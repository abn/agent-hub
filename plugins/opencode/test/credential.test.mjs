import { spawn } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { parseConfig, rekey, resolve, withLock, writeCredential } from "../credential.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const dir = join(here, "../../contract/fixtures");
const fixtures = readdirSync(dir)
  .filter((f) => f.startsWith("credential-") && f.endsWith(".json"))
  .map((f) => JSON.parse(readFileSync(join(dir, f), "utf8")));

describe("credential fixtures", () => {
  it("finds the family", () => expect(fixtures.length).toBeGreaterThan(20));
  for (const fixture of fixtures) {
    it(fixture.name, () => {
      const got = resolve(fixture.input);
      const { notices = [], ...rest } = fixture.expect;
      expect(got).toEqual({ ...rest, notices });
    });
  }
});

let root;
const base = join(here, "../../../target/tmp");
beforeEach(() => {
  mkdirSync(base, { recursive: true });
  root = mkdtempSync(join(base, "credential-"));
});
afterEach(() => rmSync(root, { recursive: true, force: true }));

const clock = (start = 1_000_000) => {
  let t = start;
  return { now: () => t, sleep: (ms) => (t += ms), advance: (ms) => (t += ms) };
};

describe("config.toml store", () => {
  it("appends a credential and keeps the rest of the file", () => {
    const path = join(root, "config.toml");
    writeFileSync(path, '[client]\nurl = "https://hub.example.net"\n# keep\n');
    writeCredential(path, "https://hub.example.net", "laptop", "t-1");
    writeCredential(path, "https://hub.example.net", "desktop", "t-2");
    const text = readFileSync(path, "utf8");
    expect(text).toContain("# keep");
    expect(parseConfig(text).credentials).toEqual([
      { url: "https://hub.example.net", member: "laptop", token: "t-1" },
      { url: "https://hub.example.net", member: "desktop", token: "t-2" },
    ]);
    expect(statSync(path).mode & 0o777).toBe(0o600);
    expect(existsSync(`${path}.lock`)).toBe(false);
  });

  it("replaces the token of an existing member, matching the url normalised", () => {
    const path = join(root, "config.toml");
    writeCredential(path, "https://Hub.example.net/", "laptop", "old");
    writeCredential(path, "https://hub.example.net", "laptop", "new");
    expect(parseConfig(readFileSync(path, "utf8")).credentials).toHaveLength(1);
    expect(parseConfig(readFileSync(path, "utf8")).credentials[0].token).toBe("new");
  });

  it("rekey renames the member and keeps the token", () => {
    const path = join(root, "config.toml");
    writeCredential(path, "https://hub.example.net", "laptop", "t-1");
    writeCredential(path, "https://hub.example.net", "desktop", "t-2");
    rekey(path, "https://hub.example.net", "laptop", "laptop-2");
    expect(parseConfig(readFileSync(path, "utf8")).credentials).toEqual([
      { url: "https://hub.example.net", member: "laptop-2", token: "t-1" },
      { url: "https://hub.example.net", member: "desktop", token: "t-2" },
    ]);
  });

  it("reads a credential back through the resolver", () => {
    const path = join(root, "config.toml");
    writeCredential(path, "https://hub.example.net", "laptop", "t-1");
    const got = resolve({ env: {}, project_root: "/r", config: parseConfig(readFileSync(path, "utf8")) });
    expect(got).toMatchObject({ state: "ok", member: "laptop", token: "t-1" });
  });

  it("parses client member, timeout and trust pairings", () => {
    const config = parseConfig(
      '[client]\nurl = "https://h"\nmember = "m"\ntimeout = 120\n[trust]\npairings = [ { path = "/p", url = "https://h", member = "m2" } ]\n',
    );
    expect(config.client).toEqual({ url: "https://h", member: "m", timeout: 120 });
    expect(config.trust.pairings).toEqual([{ path: "/p", url: "https://h", member: "m2" }]);
  });

  it("two processes writing 50 entries each lose none", async () => {
    const path = join(root, "config.toml");
    const mod = join(here, "../credential.mjs");
    const script = `import { writeCredential } from ${JSON.stringify(mod)}; for (let i = 0; i < 50; i++) writeCredential(${JSON.stringify(path)}, "https://hub.example.net", process.argv[1] + i, "tok-" + process.argv[1] + i);`;
    const run = (tag) =>
      new Promise((done) => {
        const child = spawn(process.execPath, ["--input-type=module", "-e", script, tag], { stdio: ["ignore", "ignore", "pipe"] });
        let err = "";
        child.stderr.on("data", (d) => (err += d));
        child.on("close", (code) => done({ code, err }));
      });
    const [a, b] = await Promise.all([run("a"), run("b")]);
    expect(a, a.err).toMatchObject({ code: 0 });
    expect(b, b.err).toMatchObject({ code: 0 });
    const got = parseConfig(readFileSync(path, "utf8")).credentials.map((c) => `${c.url} ${c.member} ${c.token}`);
    const want = ["a", "b"].flatMap((t) =>
      Array.from({ length: 50 }, (_, i) => `https://hub.example.net ${t}${i} tok-${t}${i}`),
    );
    expect(got.sort()).toEqual(want.sort());
  });

  it("never reuses a leftover temp file: it is replaced and the result is 0600", () => {
    const path = join(root, "config.toml");
    writeFileSync(`${path}.tmp.111`, "leftover secret", { mode: 0o644 });
    writeCredential(path, "https://hub.example.net", "laptop", "t-1", { pid: 111 });
    expect(statSync(path).mode & 0o777).toBe(0o600);
    expect(readFileSync(path, "utf8")).not.toContain("leftover");
    expect(existsSync(`${path}.tmp.111`)).toBe(false);
  });
});

describe("config.toml scanning", () => {
  const pairing = (p, m = "m") => `{ path = "${p}", url = "https://h", member = "${m}" }`;

  it("never counts a commented-out pairing in the inline array", () => {
    const text = `[trust]\npairings = [\n  # ${pairing("/r")},\n  ${pairing("/other")}, # ${pairing("/r")}\n]\n`;
    expect(parseConfig(text).trust.pairings).toEqual([{ path: "/other", url: "https://h", member: "m" }]);
    const got = resolve({
      env: {},
      project_root: "/r",
      project_file: { url: "https://h", member: "m" },
      config: parseConfig(`${text}[[credential]]\nurl = "https://h"\nmember = "m"\ntoken = "t"\n`),
    });
    expect(got).toMatchObject({ state: "needs_trust", token: null });
  });

  it("keeps a # inside a string and ignores a comment after the array", () => {
    const text = `[trust]\npairings = [ { path = "/r#1", url = 'https://h', member = "m" } ] # { path = "/x", url = "u" }\n`;
    expect(parseConfig(text).trust.pairings).toEqual([{ path: "/r#1", url: "https://h", member: "m" }]);
  });

  it("never counts a commented-out credential block or key", () => {
    const text = [
      "# [[credential]]",
      '# url = "https://h"',
      "[[credential]]",
      'url = "https://h"',
      '# member = "ghost"',
      'member = "real" # not "other"',
      'token = "t" # [[credential]]',
      "",
    ].join("\n");
    expect(parseConfig(text).credentials).toEqual([{ url: "https://h", member: "real", token: "t" }]);
  });

  it("a commented-out block is not a match for a write and survives it", () => {
    const path = join(root, "config.toml");
    writeFileSync(path, '# [[credential]]\n# url = "https://h"\n# member = "m"\n# token = "old"\n');
    writeCredential(path, "https://h", "m", "new");
    const text = readFileSync(path, "utf8");
    expect(text).toContain('# token = "old"');
    expect(parseConfig(text).credentials).toEqual([{ url: "https://h", member: "m", token: "new" }]);
  });

  it("reads multiline and literal strings", () => {
    const text = '[[credential]]\nurl = "https://h"\nmember = \'m\'\ntoken = """\nabc\\\n   def\n"""\n';
    expect(parseConfig(text).credentials).toEqual([{ url: "https://h", member: "m", token: "abcdef\n" }]);
  });

  describe("edits", () => {
    const entry = (token) => `[[credential]]\nurl = "https://h"\nmember = "m"\n${token}\n`;
    const edited = (text, token = "new") => {
      const path = join(root, "config.toml");
      writeFileSync(path, text);
      writeCredential(path, "https://h", "m", token);
      return readFileSync(path, "utf8");
    };

    it("replaces the whole of a multiline basic token", () => {
      const out = edited(`${entry('token = """\nline one\nline two"""  # keep')}[client]\nurl = "https://h"\n`);
      expect(out).toBe(`${entry('token = "new"  # keep')}[client]\nurl = "https://h"\n`);
    });

    it("replaces a multiline literal token and a single-line literal one", () => {
      expect(edited(entry("token = '''\na\nb'''"))).toBe(entry('token = "new"'));
      expect(edited(entry("token = 'a#b' # note"))).toBe(entry('token = "new" # note'));
    });

    it("keeps the inline comment of a replaced value and of a rekeyed member", () => {
      const path = join(root, "config.toml");
      writeFileSync(path, '[[credential]]\nurl = "https://h" # hub\nmember = "old" # who\ntoken = "t" # secret\n');
      rekey(path, "https://h", "old", "new");
      writeCredential(path, "https://h", "new", "t2");
      expect(readFileSync(path, "utf8")).toBe(
        '[[credential]]\nurl = "https://h" # hub\nmember = "new" # who\ntoken = "t2" # secret\n',
      );
    });

    it("keeps CRLF on a replace, an inserted field and an appended block", () => {
      const path = join(root, "config.toml");
      writeFileSync(path, '[[credential]]\r\nurl = "https://h"\r\nmember = "m"\r\n');
      writeCredential(path, "https://h", "m", "t1");
      writeCredential(path, "https://h", "m", "t2");
      writeCredential(path, "https://h", "other", "t3");
      const out = readFileSync(path, "utf8");
      expect(out.replace(/\r\n/gu, "")).not.toMatch(/[\r\n]/u);
      expect(parseConfig(out).credentials.map((c) => c.token)).toEqual(["t2", "t3"]);
    });

    it("keeps an LF file LF and a missing final newline missing", () => {
      const path = join(root, "config.toml");
      writeFileSync(path, '[client]\nurl = "https://h"');
      writeCredential(path, "https://h", "m", "t1");
      let out = readFileSync(path, "utf8");
      expect(out).not.toContain("\r");
      expect(out.endsWith("\n")).toBe(false);
      writeCredential(path, "https://h", "m", "t2");
      out = readFileSync(path, "utf8");
      expect(out.endsWith('token = "t2"')).toBe(true);
      expect(parseConfig(out).credentials).toEqual([{ url: "https://h", member: "m", token: "t2" }]);
    });

    it("inserts a missing token into a block at the end of a file without a newline", () => {
      const out = edited('[[credential]]\nurl = "https://h"\nmember = "m"');
      expect(out).toBe('[[credential]]\nurl = "https://h"\nmember = "m"\ntoken = "new"');
    });

    it("ends a new file with a newline", () => {
      expect(edited("").endsWith("\n")).toBe(true);
    });
  });
});

describe("member comparison", () => {
  const config = (member) => ({
    client: {},
    credentials: [{ url: "https://h", member, token: "t" }],
    trust: { pairings: [{ path: "/r", url: "https://h", member: " m " }] },
  });

  it("trims in inference and trust", () => {
    const got = resolve({ env: {}, project_root: "/r", project_file: { url: "https://h", member: "m" }, config: config("  m ") });
    expect(got).toMatchObject({ state: "ok", member: "m", token: "t", source: { member: "project", token: "credential" } });
    expect(resolve({ env: {}, project_root: "/r", config: config("  m ") })).toMatchObject({ member: "m", token: "t" });
  });

  it("trims in lookup, write and rekey", () => {
    const path = join(root, "config.toml");
    writeFileSync(path, '[[credential]]\nurl = "https://h"\nmember = " m "\ntoken = "old"\n');
    writeCredential(path, "https://h", "m", "new");
    expect(parseConfig(readFileSync(path, "utf8")).credentials).toEqual([{ url: "https://h", member: " m ", token: "new" }]);
    rekey(path, "https://h", "m", "n");
    expect(parseConfig(readFileSync(path, "utf8")).credentials).toEqual([{ url: "https://h", member: "n", token: "new" }]);
  });
});

describe("lock-dir protocol", () => {
  const target = () => join(root, "config.toml");
  const lockDir = () => `${target()}.lock`;
  const plant = (line) => {
    mkdirSync(lockDir());
    if (line !== null) writeFileSync(join(lockDir(), "owner"), `${line}\n`);
  };
  const opts = (c, extra = {}) => ({ now: c.now, sleep: c.sleep, host: "h1", pid: 111, ...extra });

  it("writes an owner line while held and removes the lock after", () => {
    const c = clock();
    let seen;
    withLock(target(), () => (seen = readFileSync(join(lockDir(), "owner"), "utf8")), opts(c));
    expect(seen).toMatch(/^111 h1 \d+\n$/u);
    expect(existsSync(lockDir())).toBe(false);
  });

  it("busy: waits out the budget then names the holder", () => {
    const c = clock();
    plant(`${process.pid} h1 ${c.now()}`);
    expect(() => withLock(target(), () => {}, opts(c, { budgetMs: 500, isAlive: () => true }))).toThrow(
      `config.toml is locked by pid ${process.pid} on h1`,
    );
    expect(existsSync(lockDir())).toBe(true);
  });

  it("busy: acquires when the holder releases mid-wait", () => {
    const c = clock();
    plant(`999 h1 ${c.now()}`);
    let calls = 0;
    const sleep = (ms) => {
      c.advance(ms);
      if (++calls === 3) rmSync(lockDir(), { recursive: true });
    };
    expect(withLock(target(), () => "ran", opts(c, { sleep, isAlive: () => true }))).toBe("ran");
  });

  it("stale by dead pid is broken", () => {
    const c = clock();
    plant(`999 h1 ${c.now() - 60_000}`);
    expect(withLock(target(), () => "ran", opts(c, { isAlive: () => false }))).toBe("ran");
    expect(readdirSync(root).filter((f) => f.includes("stale"))).toEqual([]);
  });

  it("an old lock with a live pid on this host is not broken", () => {
    const c = clock();
    plant(`999 h1 ${c.now() - 60_000}`);
    expect(() => withLock(target(), () => {}, opts(c, { budgetMs: 300, isAlive: () => true }))).toThrow(/locked by pid 999/u);
  });

  it("a young lock with a dead pid is not broken", () => {
    const c = clock();
    plant(`999 h1 ${c.now() - 1000}`);
    expect(() => withLock(target(), () => {}, opts(c, { budgetMs: 300, isAlive: () => false }))).toThrow(/locked/u);
  });

  it("an old lock from another host is broken", () => {
    const c = clock();
    plant(`999 other ${c.now() - 60_000}`);
    expect(withLock(target(), () => "ran", opts(c, { isAlive: () => true }))).toBe("ran");
  });

  it("a missing owner is broken by the directory age", () => {
    const path = target();
    plant(null);
    const old = new Date(Date.now() - 120_000);
    utimesSync(lockDir(), old, old);
    expect(withLock(path, () => "ran", { host: "h1", pid: 111 })).toBe("ran");
  });

  it("a missing owner on a fresh directory is not broken", () => {
    const c = clock(Date.now());
    plant(null);
    expect(() => withLock(target(), () => {}, opts(c, { budgetMs: 200 }))).toThrow(/locked/u);
  });

  it("two breakers race: exactly one runs, the lock is clean after", () => {
    const c = clock();
    plant(`999 h1 ${c.now() - 60_000}`);
    const ran = [];
    // the second breaker starts from inside the first's critical section
    withLock(
      target(),
      () => {
        ran.push("first");
        expect(() => withLock(target(), () => ran.push("second"), opts(c, { budgetMs: 100, pid: 222, isAlive: () => true }))).toThrow(
          /locked/u,
        );
      },
      opts(c, { isAlive: () => false }),
    );
    expect(ran).toEqual(["first"]);
    expect(existsSync(lockDir())).toBe(false);
  });

  it("a breaker paused after reading the stale owner cannot remove the lock another took", () => {
    const c = clock();
    plant(`999 h1 ${c.now() - 60_000}`);
    let swapped = false;
    const isAlive = () => {
      if (!swapped) {
        swapped = true;
        // breaker B breaks the stale lock and takes it while A is paused
        rmSync(lockDir(), { recursive: true });
        plant(`222 h2 ${c.now()}`);
      }
      return false;
    };
    expect(() => withLock(target(), () => {}, opts(c, { budgetMs: 300, isAlive }))).toThrow("config.toml is locked by pid 222 on h2");
    expect(readFileSync(join(lockDir(), "owner"), "utf8")).toMatch(/^222 h2 /u);
    expect(existsSync(`${lockDir()}.break`)).toBe(false);
    expect(readdirSync(root).filter((f) => f.includes("stale"))).toEqual([]);
  });

  it("a held break gate stops another breaker; an old one is cleared", () => {
    const c = clock(Date.now());
    plant(`999 h1 ${c.now() - 60_000}`);
    mkdirSync(`${lockDir()}.break`);
    expect(() => withLock(target(), () => {}, opts(c, { budgetMs: 200, isAlive: () => false }))).toThrow(/locked/u);
    expect(existsSync(lockDir())).toBe(true);
    const old = new Date(Date.now() - 60_000);
    utimesSync(`${lockDir()}.break`, old, old);
    expect(withLock(target(), () => "ran", opts(c, { isAlive: () => false }))).toBe("ran");
    expect(existsSync(`${lockDir()}.break`)).toBe(false);
  });

  it("release leaves a lock another holder took over", () => {
    const c = clock();
    withLock(target(), () => writeFileSync(join(lockDir(), "owner"), `222 h2 ${c.now()}\n`), opts(c));
    expect(readFileSync(join(lockDir(), "owner"), "utf8")).toContain("222 h2");
  });
});
