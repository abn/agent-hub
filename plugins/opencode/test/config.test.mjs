import { describe, expect, it } from "vitest";
import { configPaths, credentialPath, readProjectFile, refusal, resolveConfig } from "../config.mjs";

const files = (map) => (path) => {
  if (path in map) return map[path];
  throw new Error("ENOENT");
};

const home = "/srv/u";
const user = "/srv/u/.config/agent-hub/config.toml";
const quiet = () => {};

const store = (...entries) =>
  entries.map(([url, member, token]) => `[[credential]]\nurl = "${url}"\nmember = "${member}"\ntoken = "${token}"\n`).join("\n");

describe("config paths", () => {
  it("names the user files and then the system file, or only HUB_CONFIG", () => {
    expect(configPaths({}, home)).toEqual([user, "/srv/u/.agent-hub/config.toml", "/etc/agent-hub/config.toml"]);
    expect(configPaths({ XDG_CONFIG_HOME: "/x" }, home)[0]).toBe("/x/agent-hub/config.toml");
    expect(configPaths({ HUB_CONFIG: "/one.toml" }, home)).toEqual(["/one.toml"]);
  });

  it("writes credentials to HUB_CONFIG, else the user file in use, else the first candidate", () => {
    expect(credentialPath({ HUB_CONFIG: "/one.toml" }, { home })).toBe("/one.toml");
    expect(credentialPath({}, { home, readFile: files({}) })).toBe(user);
    expect(credentialPath({}, { home, readFile: files({ "/srv/u/.agent-hub/config.toml": "" }) })).toBe("/srv/u/.agent-hub/config.toml");
  });
});

describe("resolveConfig", () => {
  it("lets the environment name the hub, with its own token", () => {
    const readFile = files({ [user]: `[client]\nurl = "http://file"\n${store(["http://file", "laptop", "file-token"])}` });
    const got = resolveConfig({ env: { HUB_URL: "http://env/", HUB_TOKEN: "env-token" }, readFile, home });
    expect(got).toMatchObject({ url: "http://env", token: "env-token", state: "ok", source: { url: "env", token: "env" } });
  });

  it("takes the url, member and token of the stored credential the client table names", () => {
    const readFile = files({
      [user]: `[client]\nurl = "http://file"\nmember = "laptop"\n${store(["http://file", "laptop", "t-1"], ["http://file", "desktop", "t-2"])}`,
    });
    expect(resolveConfig({ env: {}, readFile, home })).toMatchObject({
      url: "http://file",
      member: "laptop",
      token: "t-1",
      state: "ok",
    });
  });

  it("reads a legacy client token as the credential of a hub with no member chosen", () => {
    const readFile = files({ [user]: '[client]\nurl = "http://file"\ntoken = "legacy"\n' });
    expect(resolveConfig({ env: {}, readFile, home })).toMatchObject({ url: "http://file", member: null, token: "legacy", state: "ok" });
  });

  it("asks for a choice when several members hold credentials and nothing picks one", () => {
    const readFile = files({ [user]: store(["http://file", "laptop", "t-1"], ["http://file", "desktop", "t-2"]) });
    const got = resolveConfig({ env: {}, readFile, home });
    expect(got).toMatchObject({ url: "http://file", state: "several_members", token: null });
    expect(refusal(got)).toBe("Several members for http://file; set member in .agent-hub.toml or HUB_MEMBER.");
    expect(resolveConfig({ env: { HUB_MEMBER: "desktop" }, readFile, home })).toMatchObject({ member: "desktop", token: "t-2", state: "ok" });
  });

  it("holds a token back when the project file picks a pairing the config does not trust", () => {
    const readFile = files({
      [user]: store(["http://file", "laptop", "t-1"], ["http://file", "secret", "t-3"]),
      "/work/repo/.agent-hub.toml": 'member = "secret"\nurl = "http://file"\n',
    });
    const got = resolveConfig({ env: {}, readFile, home, topLevel: "/work/repo" });
    expect(got).toMatchObject({ member: "secret", token: null, state: "needs_trust" });
    expect(refusal(got)).toBe(
      "This project asks to use member secret on http://file. Allow it with agent-hub trust or under [trust] in config.toml.",
    );
    const trusted = files({
      [user]: `${store(["http://file", "secret", "t-3"])}\n[trust]\npairings = [ { path = "/work/repo", url = "http://file", member = "secret" } ]\n`,
      "/work/repo/.agent-hub.toml": 'member = "secret"\nurl = "http://file"\n',
    });
    expect(resolveConfig({ env: {}, readFile: trusted, home, topLevel: "/work/repo" })).toMatchObject({ token: "t-3", state: "ok" });
  });

  it("connects without a token to a hub it holds none for", () => {
    const got = resolveConfig({ env: { HUB_URL: "http://new" }, readFile: files({}), home });
    expect(got).toMatchObject({ url: "http://new", token: null, state: "tokenless" });
    expect(refusal(got)).toBeNull();
  });

  it("uses the plugin option url only when nothing else names a hub, and warns when it is not used", () => {
    const readFile = files({ [user]: '[client]\nurl = "http://file"\ntoken = "file-token"\n' });
    const warnings = [];
    const warn = (line) => warnings.push(line);
    expect(resolveConfig({ options: { url: "http://evil" }, env: {}, readFile, home, warn })).toMatchObject({ url: "http://file", token: "file-token" });
    expect(resolveConfig({ options: { url: "http://evil" }, env: { HUB_URL: "http://env", HUB_TOKEN: "e" }, readFile, home, warn })).toMatchObject({
      url: "http://env",
      token: "e",
    });
    expect(warnings).toHaveLength(2);
    expect(warnings[0]).toContain("http://evil");
    resolveConfig({ options: { url: "http://file/" }, env: {}, readFile, home, warn });
    expect(warnings).toHaveLength(2);
    expect(resolveConfig({ options: { url: "http://project" }, env: { HUB_TOKEN: "env-token" }, readFile: files({}), home })).toMatchObject({
      url: "http://project",
      token: null,
      state: "tokenless",
    });
  });

  it("reports no url when nothing is configured", () => {
    expect(resolveConfig({ env: {}, readFile: files({}), home })).toMatchObject({ url: null, state: "no_hub" });
  });

  it("does not read the options a project config could carry as a token source", () => {
    const got = resolveConfig({ options: { url: "http://project", tokenFile: "/etc/passwd", agentName: "x" }, env: {}, readFile: files({}), home });
    expect(got.token).toBeNull();
    expect(got).not.toHaveProperty("agentName");
  });

  it("falls back across the files the binary reads, the user's first and then the system's", () => {
    const readFile = files({ "/etc/agent-hub/config.toml": '[client]\nurl = "http://system"\ntoken = "sys"\n' });
    expect(resolveConfig({ env: {}, readFile, home })).toMatchObject({ url: "http://system", token: "sys" });
  });

  it("lets HUB_CONFIG replace the layers", () => {
    const readFile = files({
      "/one.toml": '[client]\nurl = "http://one"\ntoken = "t"\n',
      [user]: '[client]\nurl = "http://file"\n',
    });
    expect(resolveConfig({ env: { HUB_CONFIG: "/one.toml" }, readFile, home })).toMatchObject({ url: "http://one", token: "t" });
  });
});

describe("the project file", () => {
  it("reads project, url and member from before the first table", () => {
    const readFile = files({ "/w/.agent-hub.toml": 'project = "p"\nurl = "http://h"\nmember = "m"\n[other]\nproject = "no"\n' });
    expect(readProjectFile("/w", readFile)).toEqual({ file: { project: "p", url: "http://h", member: "m" }, notices: [] });
  });

  it("refuses a token, since the file is shared", () => {
    const readFile = files({ "/w/.agent-hub.toml": 'token = "x"\n' });
    expect(readProjectFile("/w", readFile)).toEqual({ file: null, notices: [".agent-hub.toml must not hold a token; it is shared"] });
  });

  it("says a legacy .agent-hub file is no longer read", () => {
    const readFile = files({ "/w/.agent-hub": "old-pin\nmore\n" });
    expect(readProjectFile("/w", readFile)).toEqual({
      file: null,
      notices: ['found .agent-hub; this release reads .agent-hub.toml: project = "old-pin"'],
    });
  });

  it("has nothing outside a repository", () => {
    expect(readProjectFile(null)).toEqual({ file: null, notices: [] });
  });
});
