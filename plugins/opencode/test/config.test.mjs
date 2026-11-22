import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";
import { configPaths, parseClientToml, resolveConfig } from "../config.mjs";

const realpath = (path) => path;
const files = (map) => (path) => {
  if (path in map) return map[path];
  throw new Error("ENOENT");
};

describe("client toml", () => {
  it("reads url and token from the client table only", () => {
    const text = [
      "[hub]",
      'admin_token = "nope"',
      "url = 'ignored'",
      "",
      "[client]",
      'url = "http://hub.lan:8080/" # the hub',
      "token = 'abc\"def'",
      'agent_id = "x"',
    ].join("\n");
    expect(parseClientToml(text)).toEqual({ url: "http://hub.lan:8080/", token: 'abc"def' });
  });

  it("unescapes basic strings", () => {
    expect(parseClientToml('[client]\ntoken = "a\\"b\\\\c"').token).toBe('a"b\\c');
  });
});

describe("resolveConfig", () => {
  const home = "/srv/u";
  const toml = (url, token) => `[client]\nurl = "${url}"\ntoken = "${token}"\n`;

  it("prefers the environment, then the file, then plugin options", () => {
    const readFile = files({
      "/srv/u/.config/agent-hub/config.toml": toml("http://file", "file-token"),
      "/srv/u/.config/agent-hub/token": "opt-token\n",
    });
    const options = { url: "http://opt", tokenFile: "/srv/u/.config/agent-hub/token", agentName: "oc2" };
    const quiet = () => {};
    expect(
      resolveConfig({ options, env: { HUB_URL: "http://env/", HUB_TOKEN: "env-token" }, readFile, realpath, home, warn: quiet }),
    ).toEqual({ url: "http://env", token: "env-token", agentName: "oc2" });
    expect(resolveConfig({ options, env: {}, readFile, realpath, home, warn: quiet })).toEqual({
      url: "http://file",
      token: "file-token",
      agentName: "oc2",
    });
    expect(resolveConfig({ options, env: {}, readFile: files({ "/srv/u/.config/agent-hub/token": "opt-token\n" }), realpath, home })).toEqual({
      url: "http://opt",
      token: "opt-token",
      agentName: "oc2",
    });
  });

  it("ignores a project url when a user-level url exists, and warns once if it differs", () => {
    const readFile = files({ "/srv/u/.config/agent-hub/config.toml": toml("http://file", "file-token") });
    const warnings = [];
    const warn = (line) => warnings.push(line);
    const fromFile = resolveConfig({ options: { url: "http://evil" }, env: {}, readFile, home, warn });
    expect(fromFile).toMatchObject({ url: "http://file", token: "file-token" });
    const fromEnv = resolveConfig({ options: { url: "http://evil" }, env: { HUB_URL: "http://env", HUB_TOKEN: "e" }, readFile, home, warn });
    expect(fromEnv).toMatchObject({ url: "http://env", token: "e" });
    expect(warnings).toHaveLength(2);
    expect(warnings[0]).toContain("http://evil");
    resolveConfig({ options: { url: "http://file/" }, env: {}, readFile, home, warn });
    expect(warnings).toHaveLength(2);
  });

  it("sends no token to a url from a layer that has none", () => {
    const readFile = files({});
    const env = { HUB_TOKEN: "env-token" };
    expect(resolveConfig({ options: { url: "http://project" }, env, readFile, home })).toEqual({
      url: "http://project",
      token: null,
      agentName: "opencode",
    });
    const withFile = files({ "/srv/u/.config/agent-hub/config.toml": toml("http://file", "file-token") });
    expect(resolveConfig({ options: {}, env: { ...env, HUB_URL: "http://env" }, readFile: withFile, home }).token).toBe("env-token");
    expect(resolveConfig({ options: {}, env: { HUB_URL: "http://env" }, readFile: withFile, home }).token).toBeNull();
    expect(resolveConfig({ options: {}, env: { HUB_TOKEN: "env-token" }, readFile: withFile, home })).toMatchObject({
      url: "http://file",
      token: "file-token",
    });
  });

  it("treats empty values as unset and defaults the agent name", () => {
    const readFile = files({ "/srv/u/.config/agent-hub/config.toml": toml("http://file", "t") });
    const got = resolveConfig({ options: { url: " ", agentName: "" }, env: { HUB_URL: "", HUB_TOKEN: "" }, readFile, home });
    expect(got).toEqual({ url: "http://file", token: "t", agentName: "opencode" });
  });

  it("falls back across the files the binary reads", () => {
    const readFile = files({ "/srv/u/.agent-hub/config.toml": toml("http://legacy", "legacy") });
    expect(resolveConfig({ env: {}, readFile, home })).toMatchObject({ url: "http://legacy", token: "legacy" });
  });

  it("lets HUB_CONFIG replace the layers", () => {
    expect(configPaths({ HUB_CONFIG: "/x/c.toml" }, home)).toEqual(["/x/c.toml"]);
  });

  it("reports no url when nothing is configured", () => {
    expect(resolveConfig({ env: {}, readFile: files({}), home }).url).toBeNull();
  });

  it("takes the url and the token from the same file", () => {
    const readFile = files({
      "/srv/u/.config/agent-hub/config.toml": '[client]\ntoken = "user-token"\n',
      "/srv/u/.agent-hub/config.toml": toml("http://legacy", "legacy-token"),
      "/etc/agent-hub/config.toml": toml("http://system", "system-token"),
    });
    expect(resolveConfig({ env: {}, readFile, home })).toMatchObject({ url: "http://legacy", token: "legacy-token" });
  });

  it("sends no token when the first file with a url has none", () => {
    const readFile = files({
      "/srv/u/.config/agent-hub/config.toml": '[client]\nurl = "http://user"\n',
      "/srv/u/.agent-hub/config.toml": toml("http://legacy", "legacy-token"),
    });
    expect(resolveConfig({ env: {}, readFile, home })).toMatchObject({ url: "http://user", token: null });
  });
});

describe("tokenFile", () => {
  const scratchRoot = fileURLToPath(new URL("../../../target/tmp/", import.meta.url));
  let scratch;
  afterEach(() => {
    if (scratch) rmSync(scratch, { recursive: true, force: true });
    scratch = undefined;
  });

  function layout() {
    mkdirSync(scratchRoot, { recursive: true });
    scratch = mkdtempSync(join(scratchRoot, "opencode-config-"));
    const home = join(scratch, "home");
    const dir = join(home, ".config", "agent-hub");
    mkdirSync(dir, { recursive: true });
    mkdirSync(join(scratch, "elsewhere"));
    writeFileSync(join(dir, "token"), "good-token\n");
    writeFileSync(join(scratch, "elsewhere", "id_key"), "secret-key\n");
    return { home, dir };
  }

  const resolve = (home, tokenFile, warnings = []) =>
    resolveConfig({ options: { url: "http://project", tokenFile }, env: { HOME: home, HUB_CONFIG: join(scratch, "none.toml") }, home, warn: (l) => warnings.push(l) });

  it("reads a token file inside the agent-hub config directory", () => {
    const { home, dir } = layout();
    expect(resolve(home, join(dir, "token")).token).toBe("good-token");
  });

  it("honours XDG_CONFIG_HOME for the trusted directory", () => {
    const { home } = layout();
    const xdg = join(scratch, "xdg");
    mkdirSync(join(xdg, "agent-hub"), { recursive: true });
    writeFileSync(join(xdg, "agent-hub", "token"), "xdg-token");
    const got = resolveConfig({
      options: { url: "http://project", tokenFile: join(xdg, "agent-hub", "token") },
      env: { HOME: home, XDG_CONFIG_HOME: xdg, HUB_CONFIG: join(scratch, "none.toml") },
      home,
    });
    expect(got.token).toBe("xdg-token");
  });

  it("refuses a token file outside it, sends no token and warns once", () => {
    const { home } = layout();
    const warnings = [];
    const got = resolve(home, join(scratch, "elsewhere", "id_key"), warnings);
    expect(got).toMatchObject({ url: "http://project", token: null });
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).not.toContain("secret-key");
  });

  it("refuses a symlink inside the directory that points outside it", () => {
    const { home, dir } = layout();
    symlinkSync(join(scratch, "elsewhere", "id_key"), join(dir, "link"));
    const warnings = [];
    expect(resolve(home, join(dir, "link"), warnings).token).toBeNull();
    expect(warnings).toHaveLength(1);
  });

  it("refuses a path that climbs out with dot segments", () => {
    const { home, dir } = layout();
    expect(resolve(home, join(dir, "..", "..", "..", "elsewhere", "id_key")).token).toBeNull();
  });
});
