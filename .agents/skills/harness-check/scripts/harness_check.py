#!/usr/bin/env python3
"""Probe how Claude Code, OpenCode and Antigravity talk to an MCP server and its hooks.

One file, three roles:
  serve DIR [PORT]      a logging MCP endpoint (streamable HTTP) writing DIR/requests.jsonl
  hook CLIENT EVENT     a logging hook: stdin and env go to $HC_LOG/hooks.jsonl
  stdio-probe           a stdio MCP stub that logs the env it was started with
  run NAME...           drive a client non-interactively and print the evidence

Runs: claude, claude-plugin, claude-validate, opencode, agy, agy-validate, all.
Release runs, against a scratch hub built from this tree (cargo build first):
identity-setup, identity-children, identity-swap, identity-upgrade, identity-regression.
Everything lives under <repo>/target/tmp/harness-check/<run>/. No client's real
config is edited: Claude gets per-run flags, OpenCode gets XDG dirs and a project
config in the run directory, Antigravity gets HOME pointed at the run directory
(its sign-in survives that). The endpoint stands in for the hub: the checks are
about the clients, so no hub binary is built or started.
"""
import errno, fcntl, json, os, queue, re, shutil, subprocess, sys, threading, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs
import urllib.error
import urllib.request

SELF = os.path.abspath(__file__)
REPO = subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=os.path.dirname(SELF),
                      capture_output=True, text=True).stdout.strip()
ROOT = os.path.join(REPO, "target", "tmp", "harness-check")
TOKEN = "dummy-token-123"

# ---------------------------------------------------------------- endpoint
LOG = None
STATE = {"extra": False, "reject": 0, "streams": [], "lock": threading.Lock()}

ECHO = {"name": "echo", "description": "Return the arguments.",
        "inputSchema": {"type": "object", "properties": {
            "text": {"type": "string"},
            "session": {"type": "object", "properties": {
                "project_id": {"type": "string"}, "name": {"type": "string"}}}}}}
BUMP = {"name": "bump", "description": "Trigger tools/list_changed.",
        "inputSchema": {"type": "object", "properties": {}}}
EXTRA = {"name": "extra", "description": "Appears after list_changed.",
         "inputSchema": {"type": "object", "properties": {}}}


def log(rec):
    rec["ts"] = round(time.time(), 3)
    with STATE["lock"]:
        with open(LOG, "a") as f:
            f.write(json.dumps(rec) + "\n")


def push_list_changed():
    STATE["extra"] = True
    msg = json.dumps({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
    for q in list(STATE["streams"]):
        q.put(msg)
    log({"kind": "server-push", "method": "notifications/tools/list_changed",
         "streams": len(STATE["streams"])})


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _send(self, code, body=b"", ctype="application/json", extra=None):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        u = urlparse(self.path)
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode() if n else ""
        if u.path == "/_ctl/ready":
            return self._send(200, b"{}")
        if u.path == "/_ctl/list_changed":
            push_list_changed()
            return self._send(200, b"{}")
        if u.path == "/_ctl/reject":
            STATE["reject"] = int(parse_qs(u.query).get("n", ["1"])[0])
            return self._send(200, b"{}")
        try:
            body = json.loads(raw) if raw else None
        except ValueError:
            body = raw
        rec = {"kind": "post", "path": u.path, "headers": dict(self.headers), "body": body}
        if STATE["reject"] > 0:
            STATE["reject"] -= 1
            rec["answered"] = 401
            log(rec)
            return self._send(401, b'{"error":"unauthorized"}',
                              extra={"WWW-Authenticate": 'Bearer realm="harness"'})
        msgs = body if isinstance(body, list) else [body]
        out = []
        for m in msgs:
            if not isinstance(m, dict) or "id" not in m:
                continue
            meth, p = m.get("method"), m.get("params") or {}
            if meth == "initialize":
                res = {"protocolVersion": p.get("protocolVersion", "2025-06-18"),
                       "capabilities": {"tools": {"listChanged": True}},
                       "serverInfo": {"name": "harness-logger", "version": "0"}}
            elif meth == "tools/list":
                res = {"tools": [ECHO, BUMP] + ([EXTRA] if STATE["extra"] else [])}
            elif meth == "tools/call":
                if p.get("name") == "bump":
                    push_list_changed()
                res = {"content": [{"type": "text",
                                    "text": json.dumps(p.get("arguments") or {})}]}
            elif meth == "ping":
                res = {}
            else:
                out.append({"jsonrpc": "2.0", "id": m["id"],
                            "error": {"code": -32601, "message": "no such method"}})
                continue
            out.append({"jsonrpc": "2.0", "id": m["id"], "result": res})
        log(rec)
        if not out:
            return self._send(202)
        data = json.dumps(out[0] if not isinstance(body, list) else out).encode()
        extra = {"Mcp-Session-Id": "harness-session"} if (
            isinstance(body, dict) and body.get("method") == "initialize") else {}
        self._send(200, data, extra=extra)

    def do_DELETE(self):
        log({"kind": "delete", "path": self.path, "headers": dict(self.headers)})
        self._send(200, b"{}")

    def do_GET(self):
        u = urlparse(self.path)
        if u.path == "/_ctl/ready":
            return self._send(200, b"{}")
        log({"kind": "get-stream", "path": u.path, "headers": dict(self.headers)})
        if STATE["reject"] > 0:
            return self._send(401, b"{}", extra={"WWW-Authenticate": 'Bearer realm="harness"'})
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        q = queue.Queue()
        STATE["streams"].append(q)
        try:
            self.wfile.write(b": open\n\n")
            self.wfile.flush()
            while True:
                try:
                    msg = q.get(timeout=15)
                    self.wfile.write(f"event: message\ndata: {msg}\n\n".encode())
                except queue.Empty:
                    self.wfile.write(b": ka\n\n")
                self.wfile.flush()
        except OSError:
            pass
        finally:
            STATE["streams"].remove(q)




def serve(log_dir, port=0):
    global LOG
    os.makedirs(log_dir, exist_ok=True)
    LOG = os.path.join(log_dir, "requests.jsonl")
    srv = ThreadingHTTPServer(("127.0.0.1", port), H)
    srv.daemon_threads = True
    return srv


# ------------------------------------------------------------------- hooks
def hook(client, event):
    """Log stdin. For Claude PreToolUse on echo, $HC_PRETOOL_MODE picks the reply:
    none, updated (updatedInput only) or allow (updatedInput plus permissionDecision allow)."""
    raw = sys.stdin.read()
    try:
        payload = json.loads(raw)
    except ValueError:
        payload = {"_raw": raw}
    env = {k: os.environ.get(k) for k in ("CLAUDE_PLUGIN_DATA", "CLAUDE_PLUGIN_ROOT", "CLAUDE_PROJECT_DIR",
                                           "CLAUDE_ENV_FILE", "PLUGIN_ROOT", "PLUGIN_DATA")}
    env["plugin_keys"] = sorted(k for k in os.environ if "PLUGIN" in k)
    with open(os.path.join(os.environ["HC_LOG"], "hooks.jsonl"), "a") as f:
        f.write(json.dumps({"ts": round(time.time(), 3), "client": client, "event": event,
                            "payload": payload, "env": env}) + "\n")
    mode = os.environ.get("HC_PRETOOL_MODE", "none")
    if client == "claude" and event == "PreToolUse" and mode != "none":
        ti = payload.get("tool_input") or {}
        if str(payload.get("tool_name", "")).endswith("__echo") and "session" not in ti:
            o = {"hookEventName": "PreToolUse",
                 "updatedInput": dict(ti, session={"project_id": "P", "name": "S-injected"})}
            if mode == "allow":
                o["permissionDecision"] = "allow"
            print(json.dumps({"hookSpecificOutput": o}))


def stdio_probe():
    with open(os.path.join(os.environ["HC_LOG"], "stdio-env.jsonl"), "a") as f:
        f.write(json.dumps({"argv": sys.argv[2:], "T": os.environ.get("T"), "T2": os.environ.get("T2"),
                            "HUB_TOKEN": os.environ.get("HUB_TOKEN")}) + "\n")
    for line in sys.stdin:
        try:
            m = json.loads(line)
        except ValueError:
            continue
        if "id" not in m:
            continue
        if m.get("method") == "initialize":
            res = {"result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "stdio-env", "version": "0"}}}
        elif m.get("method") == "tools/list":
            res = {"result": {"tools": []}}
        else:
            res = {"error": {"code": -32601, "message": "no such method"}}
        print(json.dumps(dict({"jsonrpc": "2.0", "id": m["id"]}, **res)), flush=True)


# ---------------------------------------------------------------- OpenCode
OC_PLUGIN = r'''// OpenCode v2 probe plugin. Logs ctx shape, hooks and events to $HC_LOG/oc.jsonl.
// $HC_OC_MUTATE=1       tool.execute.before sets input.session on the echo tool
// $HC_OC_DELETE=1      remove a child session (one with parentID) when it finishes, to provoke session.deleted
// $HC_OC_RETRANSFORM=1  after the first echo call, re-run ctx.mcp.transform with a new header
import { appendFileSync } from "node:fs";
import { join } from "node:path";

const out = () => join(process.env.HC_LOG, "oc.jsonl");
const log = (kind, data) => {
  try {
    appendFileSync(out(), JSON.stringify({ ts: Date.now() / 1000, kind, data }, (k, v) => (typeof v === "function" ? "[fn]" : v)) + "\n");
  } catch {}
};
const shape = (o) => (o && typeof o === "object" ? Object.fromEntries(Object.entries(o).map(([k, v]) => [k, typeof v === "object" && v ? Object.keys(v) : typeof v])) : typeof o);

export default {
  id: "hc-logger",
  async setup(ctx) {
    log("setup", { ctx: shape(ctx), options: ctx.options ?? null });
    const url = process.env.HC_MCP_URL;
    const entry = (gen) => ({ type: "remote", url, headers: { Authorization: `Bearer gen-${gen}`, "X-Gen": String(gen) } });
    if (process.env.HC_OC_TRANSFORM === "1" && typeof ctx.mcp?.transform === "function") {
      await ctx.mcp.transform((editor) => {
        log("mcp.transform", { editorKeys: Object.keys(editor), has: !!editor.get("hcp") });
        editor.set("hcp", entry(1));
      });
    }
    let retransformed = false;
    if (typeof ctx.tool?.hook === "function") {
      await ctx.tool.hook("execute.before", async (event) => {
        log("tool.execute.before", event);
        const name = String(event.name ?? event.tool ?? "");
        if (process.env.HC_OC_MUTATE === "1" && name.endsWith("echo")) {
          const target = event.input ?? event.args;
          if (target && !target.session) target.session = { project_id: "P", name: "S-injected" };
          log("mutated", { name, target });
        }
        if (process.env.HC_OC_RETRANSFORM === "1" && !retransformed) {
          retransformed = true;
          try {
            await ctx.mcp.transform((editor) => editor.set("hcp", entry(2)));
            log("retransform", "done");
          } catch (e) {
            log("retransform", "error " + e);
          }
        }
      });
      await ctx.tool.hook("execute.after", (event) => log("tool.execute.after", event));
    } else {
      log("no ctx.tool.hook", {});
    }
    const ac = new AbortController();
    const kids = new Set();
    if (typeof ctx.event?.subscribe === "function") {
      (async () => {
        try {
          for await (const ev of ctx.event.subscribe({ types: ["session.created", "session.deleted", "session.idle", "session.execution.started", "session.execution.succeeded"], signal: ac.signal })) { log("event", ev); if (process.env.HC_OC_DELETE === "1" && ev.type === "session.created" && ev.data?.parentID) kids.add(ev.data.sessionID); if (process.env.HC_OC_DELETE === "1" && ev.type === "session.execution.succeeded" && kids.has(ev.data?.sessionID)) { try { await ctx.session.remove({ sessionID: ev.data.sessionID }); log("session.remove", "called"); } catch (e) { log("session.remove", "error " + e); } } }
        } catch (e) {
          log("event.error", String(e));
        }
      })();
    }
  },
};
'''


# ----------------------------------------------------------------- drivers
def jl(path):
    try:
        return [json.loads(x) for x in open(path) if x.strip()]
    except OSError:
        return []


def clean(name):
    d = os.path.join(ROOT, name)
    shutil.rmtree(d, ignore_errors=True)
    os.makedirs(os.path.join(d, "log"))
    return d


def start_server(d):
    p = subprocess.Popen([sys.executable, SELF, "serve", os.path.join(d, "log")],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    port_file = os.path.join(d, "log", "port")
    for _ in range(50):
        if os.path.exists(port_file) and open(port_file).read().strip():
            return p, int(open(port_file).read())
        time.sleep(0.1)
    p.kill()
    raise SystemExit("endpoint did not start")


def ctl(port, path):
    import urllib.request
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{port}/_ctl/{path}", method="POST"))


def sh(cmd, env=None, cwd=None, timeout=120):
    e = dict(os.environ, **(env or {}))
    if cwd:
        e["PWD"] = cwd  # OpenCode takes the project from $PWD, not from the process cwd
    r = subprocess.run(cmd, env=e, cwd=cwd, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=timeout)
    return (r.stdout + r.stderr).strip()


def show_requests(d, keys=("Authorization", "X-Static", "X-Agent-Id", "X-Gen")):
    for r in jl(os.path.join(d, "log", "requests.jsonl")):
        b = r.get("body") if isinstance(r.get("body"), dict) else {}
        m = b.get("method") or r.get("method")
        extra = (b.get("params") or {}).get("clientInfo") or (b.get("params") or {}).get("arguments") or (b.get("params") or {}).get("name") or ""
        h = {k: v for k, v in (r.get("headers") or {}).items() if k.title() in keys}
        print(f"  {r['kind']:<10} {r.get('path') or ''} {m or ''} {json.dumps(extra)[:140]} {json.dumps(h) if h else ''} {r.get('answered') or ''}")


def show_hooks(d, drop=("transcript_path", "agent_transcript_path", "cwd", "prompt", "last_assistant_message", "tool_response", "background_tasks", "transcriptPath", "workspacePaths", "artifactDirectoryPath")):
    for r in jl(os.path.join(d, "log", "hooks.jsonl")):
        p = {k: v for k, v in r["payload"].items() if k not in drop}
        print(f"  {r['client']} {r['event']}: {json.dumps(p)[:420]}")


CLAUDE_ARGS = ["--model", "haiku", "--setting-sources", "project", "--no-session-persistence", "--output-format", "json"]


def claude_run(d, port, prompt, mode="none", extra=(), mcp=True, name="p"):
    ev = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "SubagentStart", "SubagentStop", "SessionEnd", "Stop"]
    hooks = {e: [{"hooks": [{"type": "command", "command": f"python3 {SELF} hook claude {e}", "timeout": 10}]}] for e in ev}
    hooks["PreToolUse"][0]["matcher"] = hooks["PostToolUse"][0]["matcher"] = "mcp__.*"
    json.dump({"hooks": hooks}, open(os.path.join(d, "settings.json"), "w"))
    json.dump({"mcpServers": {"hc": {"type": "http", "url": f"http://127.0.0.1:{port}/mcp",
               "headers": {"Authorization": "Bearer ${HUB_TOKEN}", "X-Static": "plain"}}}}, open(os.path.join(d, "mcp.json"), "w"))
    proj = os.path.join(d, "proj")
    os.makedirs(proj, exist_ok=True)
    cmd = ["claude", "-p", prompt] + CLAUDE_ARGS + ["--settings", os.path.join(d, "settings.json"),
           "--mcp-config", os.path.join(d, "mcp.json"), "--strict-mcp-config"] + list(extra)
    out = sh(cmd, env={"HC_LOG": os.path.join(d, "log"), "HUB_TOKEN": TOKEN, "HC_PRETOOL_MODE": mode}, cwd=proj, timeout=240)
    try:
        j = json.loads(out)
        print("  denials:", [(x["tool_name"], x["tool_input"]) for x in j.get("permission_denials", [])])
        print("  result:", j["result"][:200].replace("\n", " "))
    except ValueError:
        print("  raw:", out[:300])


def run_claude():
    """clientInfo, hook payloads, updatedInput, subagent agent_id, header expansion and list_changed for Claude Code."""
    print("claude", sh(["claude", "--version"]))
    sub = "Use the mcp tool echo from server hc with text hi. Then use the Agent tool to spawn a subagent whose only job is to call the same echo tool with text sub. Report both raw results."
    for mode, extra in (("none", []), ("updated", []), ("updated", ["--allowedTools", "mcp__hc__echo"]), ("allow", [])):
        name = f"claude-{mode}{'-allowlisted' if extra else ''}"
        d = clean(name)
        p, port = start_server(d)
        try:
            print(f"\n== {name}")
            claude_run(d, port, sub if mode == "allow" else "Use the mcp tool echo from server hc with text hi. Report the raw result.", mode, extra)
            show_requests(d)
            show_hooks(d)
        finally:
            p.kill()
    d = clean("claude-listchanged")
    p, port = start_server(d)
    try:
        print("\n== claude-listchanged")
        claude_run(d, port, "Call the bump tool of server hc, then the extra tool of server hc. Report both.", extra=["--allowedTools", "mcp__hc__bump", "mcp__hc__extra"])
        show_requests(d)
    finally:
        p.kill()


PLUGIN_MCP_WORKING = {"mcpServers": {"hcp": {"type": "http", "url": "${user_config.hub_url}/mcp",
    "headersHelper": "${CLAUDE_PLUGIN_ROOT}/bin/helper.py --headers --agent claude-code --data ${CLAUDE_PLUGIN_DATA}"}}}
PLUGIN_MCP_DRAFT = {"mcpServers": {"hcp": {"type": "http", "url": "${user_config.hub_url}/mcp",
    "headersHelper": "${CLAUDE_PLUGIN_ROOT}/bin/helper.py --headers --agent claude-code --url ${user_config.hub_url}"}}}
HELPER = """#!/usr/bin/env python3
import json, os, sys, time
d = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
cnt = os.path.join(d, "helper.count")
try:
    n = int(open(cnt).read()) + 1
except (OSError, ValueError):
    n = 1
open(cnt, "w").write(str(n))
with open(os.path.join(d, "helper.jsonl"), "a") as f:
    f.write(json.dumps({"ts": round(time.time(), 3), "run": n, "argv": sys.argv[1:],
        "server_name_env": os.environ.get("CLAUDE_CODE_MCP_SERVER_NAME"), "server_url_env": os.environ.get("CLAUDE_CODE_MCP_SERVER_URL"), "plugin_data_env": os.environ.get("CLAUDE_PLUGIN_DATA"),
        "plugin_root_env": os.environ.get("CLAUDE_PLUGIN_ROOT"),
        "env_keys": sorted(k for k in os.environ if k.startswith(("CLAUDE_PLUGIN", "CLAUDE_CODE_MCP", "HUB")))}) + "\\n")
print(json.dumps({"Authorization": f"Bearer helper-{n}", "X-Agent-Id": "claude-code"}))
"""


def make_plugin(d, mcp):
    pl = os.path.join(d, "plugin")
    os.makedirs(os.path.join(pl, ".claude-plugin"), exist_ok=True)
    os.makedirs(os.path.join(pl, "bin"), exist_ok=True)
    os.makedirs(os.path.join(pl, "hooks"), exist_ok=True)
    json.dump({"name": "hcplug", "version": "0.0.1", "description": "Harness probe plugin", "author": {"name": "probe"},
               "userConfig": {"hub_url": {"type": "string", "title": "Hub URL", "description": "Base URL", "required": True}}},
              open(os.path.join(pl, ".claude-plugin", "plugin.json"), "w"))
    json.dump(mcp, open(os.path.join(pl, ".mcp.json"), "w"))
    open(os.path.join(pl, "bin", "helper.py"), "w").write(HELPER)
    os.chmod(os.path.join(pl, "bin", "helper.py"), 0o755)
    json.dump({"hooks": {e: [dict({"hooks": [{"type": "command", "command": f'python3 "{SELF}" hook claude-plugin {e}'}]}, **({"matcher": "mcp__.*"} if e == "PreToolUse" else {}))]
                         for e in ("SessionStart", "PreToolUse")}}, open(os.path.join(pl, "hooks", "hooks.json"), "w"))
    return pl


def run_claude_plugin():
    """headersHelper and CLAUDE_PLUGIN_DATA for Claude Code, for the draft .mcp.json form and the working one."""
    print("claude", sh(["claude", "--version"]))
    for label, mcp in (("draft-form", PLUGIN_MCP_DRAFT), ("working-form", PLUGIN_MCP_WORKING)):
        d = clean(f"claude-plugin-{label}")
        pl = make_plugin(d, mcp)
        print(f"\n== {label}: claude plugin validate")
        print("  " + "\n  ".join(l for l in sh(["claude", "plugin", "validate", pl]).splitlines() if "\u2718" in l or "\u2714" in l or "headersHelper" in l))
        p, port = start_server(d)
        try:
            # a 401 on the first connect attempts, then a 401 on a tool call, to see when the helper re-runs
            if label == "working-form":
                ctl(port, "reject?n=4")
            json.dump({"pluginConfigs": {"hcplug@inline": {"options": {"hub_url": f"http://127.0.0.1:{port}"}}}}, open(os.path.join(d, "settings.json"), "w"))
            json.dump({"mcpServers": {}}, open(os.path.join(d, "empty.json"), "w"))
            proj = os.path.join(d, "proj")
            os.makedirs(proj, exist_ok=True)
            out = sh(["claude", "-p", "Call the echo tool of the hcp server of plugin hcplug with text one. Report raw result or error.",
                      "--model", "haiku", "--setting-sources", "project", "--settings", os.path.join(d, "settings.json"),
                      "--plugin-dir", pl, "--allowedTools", "mcp__plugin_hcplug_hcp__echo", "--no-session-persistence",
                      "--output-format", "json"], env={"HC_LOG": os.path.join(d, "log")}, cwd=proj, timeout=240)
            try:
                print("  result:", json.loads(out)["result"][:200].replace("\n", " "))
            except ValueError:
                print("  raw:", out[:300])
            print("  helper runs:")
            for r in jl(os.path.join(pl, "helper.jsonl")):
                print("   ", json.dumps(r)[:420].replace(REPO, "<repo>").replace(os.path.expanduser("~"), "~"))
            show_requests(d)
            show_hooks(d)
            for r in jl(os.path.join(d, "log", "hooks.jsonl"))[:1]:
                print("  hook env:", json.dumps(r["env"]).replace(REPO, "<repo>").replace(os.path.expanduser("~"), "~"))
        finally:
            p.kill()
            data = os.path.expanduser("~/.claude/plugins/data/hcplug-inline")  # claude creates it for --plugin-dir
            if os.path.isdir(data) and not os.listdir(data):
                os.rmdir(data)


def run_claude_validate():
    """`claude plugin validate` on a draft copy of the Claude plugin."""
    d = clean("claude-validate")
    dr = os.path.join(d, "draft")
    shutil.copytree(os.path.join(REPO, "plugins", "claude"), dr)
    os.makedirs(os.path.join(dr, "bin"))
    open(os.path.join(dr, "bin", "agent-hub-launch"), "w").write("#!/bin/sh\nexit 0\n")
    os.chmod(os.path.join(dr, "bin", "agent-hub-launch"), 0o755)
    pj = os.path.join(dr, ".claude-plugin", "plugin.json")
    m = json.load(open(pj))
    m["userConfig"] = {"hub_url": m["userConfig"]["hub_url"]}
    json.dump(m, open(pj, "w"))
    L = '${CLAUDE_PLUGIN_ROOT}/bin/agent-hub-launch'
    def h(ev, **kw):
        return [{"hooks": [dict({"type": "command", "command": f"{L} hook {ev} --harness claude"}, **kw)]}]
    hooks = {"SessionStart": h("session-start"), "UserPromptSubmit": h("prompt"), "PreCompact": h("pre-compact"),
             "PreToolUse": [{"matcher": "mcp__.*agent-hub.*__.*", "hooks": [{"type": "command", "command": f"{L} hook pre-tool-use --harness claude", "timeout": 5}]}],
             "SubagentStart": h("subagent-start"), "SubagentStop": h("subagent-stop"), "SessionEnd": h("session-end", timeout=5),
             "Stop": [{"hooks": [{"type": "command", "command": f"{L} watch --harness claude --max-seconds 3300", "asyncRewake": True, "timeout": 3600}]}]}
    json.dump({"hooks": hooks}, open(os.path.join(dr, "hooks", "hooks.json"), "w"))
    for label, mcp in (("draft .mcp.json", PLUGIN_MCP_DRAFT), ("headersHelper without user_config", {"mcpServers": {"agent-hub": {"type": "http", "url": "${user_config.hub_url}/mcp", "headersHelper": f"{L} token --headers --agent claude-code"}}})):
        json.dump(mcp, open(os.path.join(dr, ".mcp.json"), "w"))
        out = sh(["claude", "plugin", "validate", dr])
        print(f"== {label}")
        print("  " + "\n  ".join(x for x in out.splitlines() if x.strip().startswith(("\u2718", "\u2714", "\u276f mcpServers", "\u276f hooks.PreToolUse")))[:900])


def run_opencode():
    """clientInfo, hooks, env expansion, arg mutation and list_changed for OpenCode (standalone server, isolated XDG dirs)."""
    print("opencode", sh(["opencode", "--version"]))
    plans = (
        ("opencode-mutate", dict(HC_OC_MUTATE="1", HC_OC_DELETE="1"),
         "Call the echo tool of the hc MCP server with text hi. Then use the subagent tool to spawn a subagent that calls the echo tool of hc with text sub. Then say done."),
        ("opencode-transform", dict(HC_OC_TRANSFORM="1", HC_OC_RETRANSFORM="1"),
         "First call the bump tool of the hcp MCP server. Then call the extra tool of the hcp server. Then say done."),
    )
    for name, flags, prompt in plans:
        d = clean(name)
        p, port = start_server(d)
        plug = os.path.join(d, "plugin")
        os.makedirs(plug)
        open(os.path.join(plug, "index.mjs"), "w").write(OC_PLUGIN)
        json.dump({"name": "hc-opencode-logger", "version": "0.0.0", "private": True, "type": "module", "exports": {".": "./index.mjs"}}, open(os.path.join(plug, "package.json"), "w"))
        proj = os.path.join(d, "proj")
        os.makedirs(proj)
        json.dump({"mcp": {"hc": {"type": "remote", "url": f"http://127.0.0.1:{port}/mcp", "headers": {"Authorization": "Bearer {env:HUB_TOKEN}", "X-Static": "plain"}}},
                   "plugins": [plug]}, open(os.path.join(proj, "opencode.jsonc"), "w"))
        xdg = {f"XDG_{k}_HOME": os.path.join(d, "xdg", k.lower()) for k in ("CONFIG", "DATA", "STATE", "CACHE")}
        env = dict(xdg, HC_LOG=os.path.join(d, "log"), HUB_TOKEN=TOKEN, HC_MCP_URL=f"http://127.0.0.1:{port}/mcp", **flags)
        print(f"\n== {name}")
        try:
            # never `opencode run` without --standalone: it would use the user's background service
            out = sh(["opencode", "run", "--standalone", "--auto", "-m", "opencode/deepseek-v4.1-flash-free", prompt], env=env, cwd=proj, timeout=180)
            print("  result:", out[-200:].replace("\n", " "))
        finally:
            p.kill()
        show_requests(d)
        for r in jl(os.path.join(d, "log", "oc.jsonl")):
            k, dd = r["kind"], r["data"]
            if k == "setup":
                print("  setup ctx keys:", json.dumps(dd["ctx"])[:300])
            elif k == "event" and dd["type"] in ("session.created", "session.deleted"):
                print(f"  event {dd['type']}:", json.dumps(dd["data"])[:240].replace(REPO, "<repo>"))
            elif k in ("retransform", "mcp.transform", "session.remove", "mutated") or (k == "tool.execute.before" and "echo" in json.dumps(dd)):
                print(f"  {k}:", json.dumps(dd)[:300].replace(REPO, "<repo>"))


AGY_HOOKS = ("SessionStart", "PreInvocation", "PostInvocation", "Stop")


def run_agy():
    """clientInfo, hooks and config syntax for Antigravity (HOME pointed at the run dir)."""
    print("agy", sh(["agy", "--version"]))
    d = clean("agy")
    p, port = start_server(d)
    home = os.path.join(d, "home")
    cfg = os.path.join(home, ".gemini", "config")
    os.makedirs(cfg)
    proj = os.path.join(d, "proj")
    os.makedirs(proj)
    hd = {"Authorization": "Bearer ${HUB_TOKEN}", "X-B": "$HUB_TOKEN", "X-C": "${env:HUB_TOKEN}", "X-D": "{env:HUB_TOKEN}", "X-E": "%HUB_TOKEN%", "X-Static": "plain"}
    json.dump({"mcpServers": {
        "s1": {"url": f"http://127.0.0.1:{port}/s1", "headers": hd},
        "s2": {"serverUrl": f"http://127.0.0.1:{port}/s2", "headers": hd},
        "s3": {"command": sys.executable, "args": [SELF, "stdio-probe", "env-block"], "env": {"T": "${HUB_TOKEN}", "T2": "literal"}},
        "s4": {"command": sys.executable, "args": [SELF, "stdio-probe", "inherit"]},
        "s5": {"url": f"http://127.0.0.1:${{HC_PORT}}/s5"},
        "s6": {"url": f"http://127.0.0.1:{port}/s6", "headers": {"Authorization": "Bearer ${HUB_TOKEN}"},
               "variables": [{"name": "HUB_TOKEN", "title": "Hub token", "description": "probe"}]}}}, open(os.path.join(cfg, "mcp_config.json"), "w"))
    hooks = {"hc": dict({e: [{"type": "command", "command": f'{sys.executable} "{SELF}" hook agy {e}'}] for e in AGY_HOOKS},
                        PreToolUse=[{"matcher": "*", "hooks": [{"type": "command", "command": f'{sys.executable} "{SELF}" hook agy PreToolUse'}]}])}
    json.dump(hooks, open(os.path.join(cfg, "hooks.json"), "w"))
    try:
        out = sh(["agy", "-p", "Call the echo tool of the MCP server s1 with text hi and report the raw result.", "--model", "gemini-3.8-flash-low",
                  "--settings", json.dumps({"permissions": {"allow": ["mcp(*)"]}})],
                 env={"HOME": home, "HC_LOG": os.path.join(d, "log"), "HUB_TOKEN": TOKEN, "HC_PORT": str(port)}, cwd=proj, timeout=100)
        print("  result:", out[:300].replace("\n", " "))
    finally:
        p.kill()
    show_requests(d, keys=("Authorization", "X-B", "X-C", "X-D", "X-E", "X-Static"))
    print("  stdio env:", jl(os.path.join(d, "log", "stdio-env.jsonl")))
    show_hooks(d)


def run_agy_validate():
    """`agy plugin validate` on a draft copy of the Antigravity plugin."""
    d = clean("agy-validate")
    dr = os.path.join(d, "draft")
    shutil.copytree(os.path.join(REPO, "plugins", "antigravity"), dr)
    os.makedirs(os.path.join(dr, "bin"))
    open(os.path.join(dr, "bin", "agent-hub-launch"), "w").write("#!/bin/sh\nexit 0\n")
    os.chmod(os.path.join(dr, "bin", "agent-hub-launch"), 0o755)
    json.dump({"mcpServers": {"agent-hub": {"command": "agent-hub", "args": ["mcp"]}}}, open(os.path.join(dr, "mcp_config.json"), "w"))
    json.dump({"agent-hub": {"PreInvocation": [{"type": "command", "command": "./bin/agent-hub-launch hook prompt --harness antigravity", "timeout": 20}]}}, open(os.path.join(dr, "hooks.json"), "w"))
    for flag in ([], ["--strict"]):
        print("== agy plugin validate", *flag)
        print("  " + sh(["agy", "plugin", "validate", dr, *flag]).replace(REPO, "<repo>").replace("\n", "\n  "))



# ------------------------------------------------------- release runs (live)
# These drive real clients against a scratch hub started from this checkout's
# build, with its data directory under target/tmp. They never touch a hub the
# user runs, its data directory or a client's real config. HC_DRY=1 prints the
# client commands instead of running them (no model calls).
HUB_BIN = os.path.join(REPO, "target", "debug", "agent-hub")
ADMIN = "identity-live-admin"
DRY = bool(os.environ.get("HC_DRY"))
LIVE_PROJECT = "homelab"
SECRETS = []
FAILED = []


def redact(text):
    text = str(text).replace(REPO, "<repo>").replace(os.path.expanduser("~"), "~")
    for secret in SECRETS:
        text = text.replace(secret, "<token>")
    return text


def scratch_env(data, **extra):
    """The environment of a scratch hub: the caller's, minus every HUB_* setting that could point it elsewhere."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("HUB_")}
    env.update(HUB_DATA_DIR=data, HUB_BIND="127.0.0.1:0", HUB_ADMIN_TOKEN=ADMIN, **extra)
    return env


def configured_data_dirs():
    """Every data directory the user's own setup would serve: the environment, the user config files, the platform default."""
    import tomllib
    found = [os.environ.get("HUB_DATA_DIR")]
    base = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    found.append(os.path.join(base, "agent-hub"))
    cfg = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
    for path in (os.path.join(cfg, "agent-hub", "config.toml"), os.path.expanduser("~/.agent-hub/config.toml"),
                 "/etc/agent-hub/config.toml", os.environ.get("HUB_CONFIG")):
        try:
            with open(path, "rb") as f:
                found.append((tomllib.load(f).get("hub") or {}).get("data_dir"))
        except (OSError, TypeError, ValueError):
            pass
    return [os.path.realpath(p) for p in found if p and os.path.isabs(p)]


def held_by_a_hub(data):
    """True when a running hub holds this store: its hub.lock is locked by another process."""
    try:
        fd = os.open(os.path.join(data, "hub.lock"), os.O_RDONLY)
    except OSError:
        return False
    try:
        fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
    except OSError as e:
        return e.errno in (errno.EWOULDBLOCK, errno.EAGAIN)
    finally:
        os.close(fd)
    return False


def start_hub(d, data=None):
    if not os.path.exists(HUB_BIN):
        raise SystemExit(f"build the hub first: cargo build ({HUB_BIN} is missing)")
    data = data or os.path.join(d, "data")
    os.makedirs(data, exist_ok=True)
    env = scratch_env(data, RUST_LOG="error,agent_hub::app=info")
    proc = subprocess.Popen([HUB_BIN, "serve"], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.PIPE, text=True, start_new_session=True)
    port = None
    for line in proc.stderr:
        m = re.search(r"hub listening.*?bind=\S*?:(\d+)", re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", line))
        if m:
            port = int(m.group(1))
            break
    if port is None:
        proc.kill()
        raise SystemExit("the scratch hub did not start")
    threading.Thread(target=lambda: [None for _ in proc.stderr], daemon=True).start()
    return proc, port


def rest(port, method, path, body=None, token=ADMIN):
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", method=method,
                                 data=None if body is None else json.dumps(body).encode(),
                                 headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            code, raw = r.status, r.read()
    except urllib.error.HTTPError as e:
        code, raw = e.code, e.read()
    try:
        return code, json.loads(raw or b"null")
    except ValueError:
        return code, raw.decode("utf-8", "replace")


def seed_hub(port, member="livecheck"):
    code, body = rest(port, "POST", "/api/v1/projects", {"id": LIVE_PROJECT, "display_name": "Homelab"})
    if code not in (200, 201):
        raise SystemExit(f"could not create the project: {code} {body}")
    code, body = rest(port, "POST", "/api/v1/members", {"id": member, "display_name": member})
    if code != 201:
        raise SystemExit(f"could not create the member: {code} {body}")
    code, body = rest(port, "POST", f"/api/v1/members/{member}/token")
    if code != 201:
        raise SystemExit(f"could not issue a token: {code} {body}")
    SECRETS.append(body["token"])
    return body["token"]


def admin_tool(port, name, arguments):
    """One admin MCP tool call, for reading a brain by session id."""
    session = {"id": ""}
    def post(payload):
        headers = {"Authorization": f"Bearer {ADMIN}", "Content-Type": "application/json",
                   "Accept": "application/json, text/event-stream"}
        if session["id"]:
            headers["mcp-session-id"] = session["id"]
        req = urllib.request.Request(f"http://127.0.0.1:{port}/mcp", method="POST", data=json.dumps(payload).encode(), headers=headers)
        with urllib.request.urlopen(req, timeout=30) as r:
            session["id"] = session["id"] or r.headers.get("mcp-session-id", "")
            raw = r.read().decode()
        for line in raw.splitlines():
            if line.startswith("data:") and line[5:].strip():
                return json.loads(line[5:])
        return json.loads(raw) if raw.strip() else {}
    post({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "harness-check", "version": "0"}}})
    post({"jsonrpc": "2.0", "method": "notifications/initialized"})
    return post({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": name, "arguments": arguments}}).get("result", {}).get("structuredContent", {})


def show_sessions(port):
    code, body = rest(port, "GET", f"/api/v1/sessions?project={LIVE_PROJECT}")
    rows = body.get("sessions", []) if isinstance(body, dict) else []
    for r in rows:
        who = ", ".join(f"{p.get('member')}/{p.get('agent')}" for p in r.get("participants", []))
        print(f"  session {r.get('session_name')} [{r.get('status')}] parent={r.get('parent_id')} participants: {who}")
    return rows


def verdict(name, ok, why=""):
    print(f"  VERDICT {name}: {'holds' if ok else 'FAILS'}{' - ' + why if why else ''}")
    if not ok and not DRY:
        FAILED.append(name)
    return ok


def client(cmd, **kw):
    """Run a client command, or print it under HC_DRY."""
    if DRY:
        print("  dry:", redact(" ".join(cmd)))
        return ""
    return redact(sh(cmd, **kw))


def write_prompt(name):
    return ("Call the whoami tool of the agent-hub MCP server and report its raw JSON. Then call brain_put of the "
            f"agent-hub MCP server with store \"session\", path \"/kv/live\", content \"ok\" and session "
            f"{{\"project_id\": \"{LIVE_PROJECT}\", \"name\": \"{name}\"}}. Report both raw results.")


def run_identity_setup():
    """No-binary setup: each client reaches the hub with a bearer from the environment alone."""
    d = clean("identity-setup")
    hub, port = start_hub(d)
    try:
        token = seed_hub(port)
        url = f"http://127.0.0.1:{port}"
        print(f"claude {sh(['claude', '--version'])} | opencode {sh(['opencode', '--version'])} | agy {sh(['agy', '--version'])}")
        proj = os.path.join(d, "proj")
        os.makedirs(proj, exist_ok=True)

        print("\n== claude: Authorization Bearer ${HUB_TOKEN} in --mcp-config")
        json.dump({"mcpServers": {"agent-hub": {"type": "http", "url": f"{url}/mcp", "headers": {"Authorization": "Bearer ${HUB_TOKEN}"}}}},
                  open(os.path.join(d, "mcp.json"), "w"))
        print(" ", client(["claude", "-p", write_prompt("live-claude"), "--model", "haiku", "--setting-sources", "project",
                           "--no-session-persistence", "--mcp-config", os.path.join(d, "mcp.json"), "--strict-mcp-config",
                           "--allowedTools", "mcp__agent-hub__whoami", "mcp__agent-hub__brain_put"],
                          env={"HUB_TOKEN": token}, cwd=proj, timeout=240)[-400:])

        print("\n== opencode: Authorization Bearer {env:HUB_TOKEN} in opencode.jsonc")
        json.dump({"mcp": {"agent-hub": {"type": "remote", "url": f"{url}/mcp", "headers": {"Authorization": "Bearer {env:HUB_TOKEN}"}}}},
                  open(os.path.join(proj, "opencode.jsonc"), "w"))
        xdg = {f"XDG_{k}_HOME": os.path.join(d, "xdg", k.lower()) for k in ("CONFIG", "DATA", "STATE", "CACHE")}
        print(" ", client(["opencode", "run", "--standalone", "--auto", "-m", "opencode/deepseek-v4.1-flash-free", write_prompt("live-opencode")],
                          env=dict(xdg, HUB_TOKEN=token), cwd=proj, timeout=180)[-400:])

        print("\n== antigravity: stdio `agent-hub mcp`, no env block (a stdio server inherits the parent env)")
        home = os.path.join(d, "home")
        cfg = os.path.join(home, ".gemini", "config")
        os.makedirs(cfg, exist_ok=True)
        json.dump({"mcpServers": {"agent-hub": {"command": HUB_BIN, "args": ["mcp"]}}}, open(os.path.join(cfg, "mcp_config.json"), "w"))
        print(" ", client(["agy", "-p", write_prompt("live-antigravity").replace("agent-hub MCP server", "MCP server agent-hub"), "--model", "gemini-3.8-flash-low",
                           "--settings", json.dumps({"permissions": {"allow": ["mcp(*)"]}})],
                          env={"HOME": home, "HUB_URL": url, "HUB_TOKEN": token}, cwd=proj, timeout=120)[-400:])

        print("\n== the hub's view")
        rows = {r.get("session_name"): r for r in show_sessions(port)}
        for name, agent in (("live-claude", "claude-code"), ("live-opencode", "opencode"), ("live-antigravity", "antigravity")):
            seen = [(p.get("member"), p.get("agent")) for p in rows.get(name, {}).get("participants", [])]
            verdict(name, ("livecheck", agent) in seen, f"participants {seen}")
    finally:
        hub.kill()


def run_identity_children():
    """Child isolation: a Claude subagent and an OpenCode child write in <parent>/<task>."""
    d = clean("identity-children")
    hub, port = start_hub(d)
    try:
        token = seed_hub(port)
        url = f"http://127.0.0.1:{port}"
        proj = os.path.join(d, "proj")
        os.makedirs(os.path.join(proj, ".git"), exist_ok=True)
        config = os.path.join(d, "config.toml")
        open(config, "w").write(f'[client]\nurl = "{url}"\nmember = "livecheck"\n\n[[credential]]\nurl = "{url}"\nmember = "livecheck"\ntoken = "{token}"\n')
        os.chmod(config, 0o600)
        env = {"HUB_CONFIG": config, "HUB_PROJECT": LIVE_PROJECT, "PATH": f"{os.path.dirname(HUB_BIN)}:{os.environ['PATH']}"}
        prompt = ("Write a note with the hub's brain_put tool: store \"session\", path \"/kv/parent\", content \"parent\". "
                  "Then use the Agent tool to spawn a subagent whose only job is to call brain_put with store \"session\", "
                  "path \"/kv/child\", content \"child\". Then say done.")

        print("== claude: the plugin from this checkout, a subagent")
        json.dump({"pluginConfigs": {"agent-hub@inline": {"options": {"hub_url": url}}}}, open(os.path.join(d, "settings.json"), "w"))
        print(" ", client(["claude", "-p", prompt, "--model", "haiku", "--setting-sources", "project", "--no-session-persistence",
                           "--settings", os.path.join(d, "settings.json"), "--plugin-dir", os.path.join(REPO, "plugins", "claude"),
                           "--allowedTools", "mcp__plugin_agent-hub_agent-hub", "Agent"],
                          env=dict(env, HUB_SESSION="live-claude"), cwd=proj, timeout=300)[-300:])

        print("\n== opencode: the plugin from this checkout, a child session")
        json.dump({"plugins": [os.path.join(REPO, "plugins", "opencode")]}, open(os.path.join(proj, "opencode.jsonc"), "w"))
        xdg = {f"XDG_{k}_HOME": os.path.join(d, "xdg", k.lower()) for k in ("CONFIG", "DATA", "STATE", "CACHE")}
        print(" ", client(["opencode", "run", "--standalone", "--auto", "-m", "opencode/deepseek-v4.1-flash-free", prompt],
                          env=dict(env, **xdg, HUB_SESSION="live-opencode", HUB_URL=url, HUB_TOKEN=token), cwd=proj, timeout=240)[-300:])

        print("\n== the hub's view")
        rows = show_sessions(port)
        by_name = {r["session_name"]: r for r in rows}
        for root in ("live-claude", "live-opencode"):
            kids = [r for n, r in by_name.items() if n.startswith(root + "/")]
            verdict(f"{root} child session exists", bool(kids), ", ".join(k["session_name"] for k in kids))
            if root in by_name and kids:
                mine = json.dumps(admin_tool(port, "brain_list", {"store": "session", "session": {"session_id": by_name[root]["id"]}}))
                theirs = json.dumps(admin_tool(port, "brain_list", {"store": "session", "session": {"session_id": kids[0]["id"]}}))
                verdict(f"{root} brain untouched by the child", "/kv/child" not in mine and "/kv/parent" in mine, mine[:160])
                verdict(f"{root} child brain holds only its own page", "/kv/child" in theirs and "/kv/parent" not in theirs, theirs[:160])
    finally:
        hub.kill()


def run_identity_swap():
    """Approval after a credential exists but is pending: what each client does next, in one run."""
    d = clean("identity-swap")
    hub, port = start_hub(d)
    try:
        seed_hub(port)
        url = f"http://127.0.0.1:{port}"
        proj = os.path.join(d, "proj")
        os.makedirs(os.path.join(proj, ".git"), exist_ok=True)
        prompt = ("Call the whoami tool of the agent-hub MCP server and report its raw JSON. Then run the shell command `sleep 25`. "
                  "Then call whoami again and report its raw JSON. Say which tools the server offers each time.")

        def pending(name):
            code, body = rest(port, "POST", "/api/v1/enrol", {"member": name, "display_name": name, "why": "live gate", "agent": "cli"}, token="")
            if code != 202:
                raise SystemExit(f"enrol failed: {code} {body}")
            SECRETS.append(body["token"])
            return body["token"]

        def approve_later(name, renamed, after=10):
            def go():
                time.sleep(after)
                print(f"  [approve {name} as {renamed}]", rest(port, "POST", f"/api/v1/enrol/{name}/approve", {"member": renamed})[0], flush=True)
            threading.Thread(target=go, daemon=True).start()

        print("== claude: the plugin's headersHelper reads a pending credential, approval lands as another name")
        t = pending("swap-claude")
        config = os.path.join(d, "claude.toml")
        open(config, "w").write(f'[client]\nurl = "{url}"\n\n[[credential]]\nurl = "{url}"\nmember = "swap-claude"\ntoken = "{t}"\n')
        os.chmod(config, 0o600)
        json.dump({"pluginConfigs": {"agent-hub@inline": {"options": {"hub_url": url}}}}, open(os.path.join(d, "settings.json"), "w"))
        approve_later("swap-claude", "swapped-claude")
        print(" ", client(["claude", "-p", prompt, "--model", "haiku", "--setting-sources", "project", "--no-session-persistence",
                           "--settings", os.path.join(d, "settings.json"), "--plugin-dir", os.path.join(REPO, "plugins", "claude"),
                           "--allowedTools", "mcp__plugin_agent-hub_agent-hub", "Bash"],
                          env={"HUB_CONFIG": config, "PATH": f"{os.path.dirname(HUB_BIN)}:{os.environ['PATH']}"}, cwd=proj, timeout=300)[-700:])
        print("  record: does the second whoami show the approved member without a reconnect?")

        print("\n== opencode: the plugin reads the same kind of credential")
        t = pending("swap-opencode")
        config = os.path.join(d, "opencode.toml")
        open(config, "w").write(f'[client]\nurl = "{url}"\n\n[[credential]]\nurl = "{url}"\nmember = "swap-opencode"\ntoken = "{t}"\n')
        os.chmod(config, 0o600)
        json.dump({"plugins": [os.path.join(REPO, "plugins", "opencode")]}, open(os.path.join(proj, "opencode.jsonc"), "w"))
        xdg = {f"XDG_{k}_HOME": os.path.join(d, "xdg", k.lower()) for k in ("CONFIG", "DATA", "STATE", "CACHE")}
        approve_later("swap-opencode", "swapped-opencode")
        print(" ", client(["opencode", "run", "--standalone", "--auto", "-m", "opencode/deepseek-v4.1-flash-free", prompt],
                          env=dict(xdg, HUB_CONFIG=config, HUB_URL=url, HUB_PROJECT=LIVE_PROJECT), cwd=proj, timeout=240)[-700:])
        print("  record: the swap, or the restart line?")

        print("\n== antigravity: the stdio proxy hot-swaps in the same process")
        t = pending("swap-agy")
        config = os.path.join(d, "agy.toml")
        open(config, "w").write(f'[client]\nurl = "{url}"\n\n[[credential]]\nurl = "{url}"\nmember = "swap-agy"\ntoken = "{t}"\n')
        os.chmod(config, 0o600)
        home = os.path.join(d, "home")
        cfg = os.path.join(home, ".gemini", "config")
        os.makedirs(cfg, exist_ok=True)
        json.dump({"mcpServers": {"agent-hub": {"command": HUB_BIN, "args": ["mcp"]}}}, open(os.path.join(cfg, "mcp_config.json"), "w"))
        approve_later("swap-agy", "swapped-agy")
        out = client(["agy", "-p", prompt.replace("agent-hub MCP server", "MCP server agent-hub"), "--model", "gemini-3.8-flash-low",
                      "--settings", json.dumps({"permissions": {"allow": ["mcp(*)"]}})],
                     env={"HOME": home, "HUB_URL": url, "HUB_CONFIG": config}, cwd=proj, timeout=180)
        print(" ", out[-700:])
        verdict("antigravity swap", DRY or "swapped-agy" in out, "the second whoami names the approved member")
    finally:
        hub.kill()


def run_identity_upgrade():
    """Copied-store upgrade. HC_UPGRADE_COPY names a copy of a data directory (never the live one)."""
    src = os.environ.get("HC_UPGRADE_COPY")
    if not src or not os.path.isdir(src):
        raise SystemExit("set HC_UPGRADE_COPY to a copy of a data directory")
    if os.path.realpath(src) in configured_data_dirs() or held_by_a_hub(src):
        raise SystemExit("HC_UPGRADE_COPY is a live data directory (configured, or held by a running hub); copy it first")
    d = clean("identity-upgrade")
    work = os.path.join(d, "data")
    shutil.copytree(src, work, symlinks=True)
    print("== migrate --check on the copy (read only)")
    out = subprocess.run([HUB_BIN, "migrate", "--check", "--data-dir", work], env=scratch_env(work), capture_output=True, text=True, timeout=600)
    print(redact(out.stdout[-1500:]), redact(out.stderr[-500:]), f"exit {out.returncode} (0 clean, 78 stops)")
    if out.returncode != 0:
        verdict("migrate --check", False, "the plan stops; nothing was upgraded")
        return
    print("\n== serve the copy and read it back")
    hub, port = start_hub(d, data=work)
    try:
        for path in ("/api/v1/projects", "/api/v1/members"):
            code, body = rest(port, "GET", path)
            print(f"  GET {path}: {code}, {len(body.get('members', body.get('projects', []))) if isinstance(body, dict) else '?'} rows")
        before = show_sessions(port)
        shots = os.path.join(d, "shots")
        os.makedirs(shots, exist_ok=True)
        script = ("import { chromium } from '@playwright/test'; const [url, token, dir] = process.argv.slice(1);"
                  "const b = await chromium.launch(); const p = await b.newPage({ viewport: { width: 1200, height: 800 } });"
                  "await p.addInitScript((t) => localStorage.setItem('hub.token', t), token);"
                  "for (const [name, hash] of [['now', '#/home'], ['members', '#/access'], ['sessions', '#/sessions']]) {"
                  " await p.goto(url + '/' + hash); await p.waitForTimeout(1500); await p.screenshot({ path: dir + '/' + name + '.png' }); }"
                  "await b.close();")
        shot = subprocess.run(["node", "--input-type=module", "-e", script, f"http://127.0.0.1:{port}", ADMIN, shots], cwd=REPO, capture_output=True, text=True, timeout=120)
        print(f"  PWA screenshots in {redact(shots)}: exit {shot.returncode} {redact(shot.stderr[-200:])}")
    finally:
        hub.kill()
        hub.wait()
    print("\n== kill during the upgrade, restart, and compare")
    for delay in (0.05, 0.2, 0.5, 1.0):
        again = os.path.join(d, f"kill-{delay}")
        shutil.copytree(src, again, symlinks=True)
        victim = subprocess.Popen([HUB_BIN, "serve"], env=scratch_env(again), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        time.sleep(delay)
        victim.kill()
        victim.wait()
        hub, port = start_hub(d, data=again)
        try:
            rows = show_sessions(port)
            verdict(f"restart after a kill at {delay}s", len(rows) == len(before), f"{len(rows)} sessions vs {len(before)}")
        finally:
            hub.kill()
            hub.wait()
    print("\n== back up the upgraded copy, restore it and check")
    backup, restored = os.path.join(d, "backup"), os.path.join(d, "restored")
    for cmd in ([HUB_BIN, "backup", "--out", backup, "--data-dir", work], [HUB_BIN, "restore", "--from", backup, "--data-dir", restored],
                [HUB_BIN, "check", "--data-dir", restored]):
        r = subprocess.run(cmd, env=scratch_env(work), capture_output=True, text=True, timeout=600)
        verdict(" ".join(cmd[1:2]), r.returncode == 0, redact((r.stdout + r.stderr)[-200:]))


def regression_rows():
    """The rows of the recorded matrix that a fresh run's logs decide, each against what it recorded."""
    def posts(run):
        return [r for r in jl(os.path.join(ROOT, run, "log", "requests.jsonl")) if r.get("kind") == "post" and isinstance(r.get("body"), dict)]

    def calls(run, tool):
        return [(r["body"].get("params") or {}).get("arguments") or {} for r in posts(run)
                if r["body"].get("method") == "tools/call" and (r["body"].get("params") or {}).get("name") == tool]

    def client_name(run):
        for r in posts(run):
            if r["body"].get("method") == "initialize":
                return ((r["body"].get("params") or {}).get("clientInfo") or {}).get("name")
        return None

    def auth(run):
        return {r["headers"].get("Authorization") for r in posts(run) if r.get("headers")}

    def hooks(run):
        return jl(os.path.join(ROOT, run, "log", "hooks.jsonl"))

    def injected(run):
        return any((a.get("session") or {}).get("name") == "S-injected" for a in calls(run, "echo"))

    sub_start = [h["payload"].get("agent_id") for h in hooks("claude-allow") if h["event"] == "SubagentStart"]
    sub_tool = [h["payload"].get("agent_id") for h in hooks("claude-allow") if h["event"] == "PreToolUse" and h["payload"].get("agent_id")]
    pushed = any(r.get("kind") == "server-push" for r in jl(os.path.join(ROOT, "claude-listchanged", "log", "requests.jsonl")))
    return [
        ("claude clientInfo", client_name("claude-none") == "claude-code", client_name("claude-none")),
        ("opencode clientInfo", client_name("opencode-mutate") == "opencode", client_name("opencode-mutate")),
        ("antigravity clientInfo", client_name("agy") == "antigravity-client", client_name("agy")),
        ("claude hooks", {"SessionStart", "UserPromptSubmit", "PreToolUse", "SubagentStart", "SubagentStop", "SessionEnd"} <= {h["event"] for h in hooks("claude-allow")}, ""),
        ("updatedInput reaches an allowlisted tool", injected("claude-updated-allowlisted"), ""),
        ("updatedInput alone changes no permission", not calls("claude-updated", "echo"), ""),
        ("claude agent_id", bool(sub_start) and bool(sub_tool) and sub_start[0] == sub_tool[0], f"{sub_start[:1]} {sub_tool[:1]}"),
        ("claude ${HUB_TOKEN}", "Bearer dummy-token-123" in auth("claude-none"), ""),
        ("opencode {env:HUB_TOKEN}", "Bearer dummy-token-123" in auth("opencode-mutate"), ""),
        ("antigravity does not expand (recorded: fails)", "Bearer dummy-token-123" not in auth("agy"), ""),
        ("opencode arg mutation", injected("opencode-mutate"), ""),
        ("opencode transform re-run", any(a and "gen-2" in a for a in auth("opencode-transform")), ""),
        ("claude list_changed", pushed and bool(calls("claude-listchanged", "extra")), ""),
        ("headersHelper (working form)", any(a and "helper-" in a for a in auth("claude-plugin-working-form")), ""),
    ]


def run_identity_regression():
    """Re-run the client checks against this checkout and read what regressed."""
    print("claude", sh(["claude", "--version"]), "| opencode", sh(["opencode", "--version"]), "| agy", sh(["agy", "--version"]))
    for name in ("claude", "claude-plugin", "opencode", "agy"):
        print(f"\n######## {name}")
        RUNS[name]()
    print("\n######## the checkout's own plugins")
    failed = []
    for label, cmd in (("claude plugin validate plugins/claude", ["claude", "plugin", "validate", os.path.join(REPO, "plugins", "claude")]),
                       ("claude plugin validate .", ["claude", "plugin", "validate", REPO]),
                       ("agy plugin validate --strict plugins/antigravity", ["agy", "plugin", "validate", "--strict", os.path.join(REPO, "plugins", "antigravity")])):
        r = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
        print(f"== {label}: exit {r.returncode}\n  " + redact((r.stdout + r.stderr)[-500:]).replace("\n", "\n  "))
        if r.returncode != 0:
            failed.append(label)
    print("\n######## matrix rows read from the logs")
    for row, ok, detail in regression_rows():
        print(f"  {'holds    ' if ok else 'REGRESSED'} {row} {detail or ''}")
        if not ok:
            failed.append(row)
    print("\nregression:", "none" if not failed else f"{len(failed)} row(s): " + "; ".join(failed))
    FAILED.extend(failed)


RUNS = {"claude": run_claude, "claude-plugin": run_claude_plugin, "claude-validate": run_claude_validate,
        "opencode": run_opencode, "agy": run_agy, "agy-validate": run_agy_validate,
        "identity-setup": run_identity_setup, "identity-children": run_identity_children, "identity-swap": run_identity_swap,
        "identity-upgrade": run_identity_upgrade, "identity-regression": run_identity_regression}


def main(argv):
    """Returns the process exit code: 1 when any identity verdict failed or a regression row no longer holds."""
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0
    cmd = argv[0]
    if cmd == "serve":
        s = serve(argv[1], int(argv[2]) if len(argv) > 2 else 0)
        open(os.path.join(argv[1], "port"), "w").write(str(s.server_address[1]))
        s.serve_forever()
    elif cmd == "hook":
        hook(argv[1], argv[2])
    elif cmd == "stdio-probe":
        stdio_probe()
    elif cmd == "run":
        names = [n for n in RUNS if not n.startswith("identity-")] if argv[1:] == ["all"] else argv[1:]
        for n in names:
            print(f"\n######## {n}")
            RUNS[n]()
    else:
        raise SystemExit(f"unknown command {cmd}")
    if FAILED:
        print(f"\nFAILED: {len(FAILED)} check(s): " + "; ".join(FAILED))
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
