// The member screen's pure parts: the setup a token comes with, the approval
// card, a name the hub will take, which agents a member has run, and the Who
// menu's choices as the filters the hub is sent.

import { beforeEach, describe, expect, it } from "vitest";
import { setMemberCount } from "../../web/author.mjs";
import { MEMBER_NAME, agentsSeen, approvalCardHTML, approveNote, memberNameProblem, memberSetup } from "../../web/members.mjs";
import { artifactsScope, setWho, whoActive, whoMenu, whoParams, whoPick, whoQuery } from "../../web/who.mjs";

const setup = memberSetup({ url: "https://hub.example.net/", member: "laptop", token: "tok-secret-123" });
const flat = (group) => setup.find((g) => g.id === group).snippets.map((s) => s.text).join("\n");

describe("the setup a new token comes with", () => {
  it("splits what goes on the machine from the two forms that go in a repository", () => {
    expect(setup.map((group) => group.title)).toEqual(["Store on that machine", "With the agent-hub binary", "Without it"]);
  });

  it("puts the token in the machine's store and nowhere in the project's", () => {
    expect(flat("store")).toContain("tok-secret-123");
    expect(flat("binary")).not.toContain("tok-secret-123");
    expect(flat("env")).not.toContain("tok-secret-123");
  });

  it("writes the credential entry for the config file", () => {
    const entry = setup[0].snippets.find((s) => s.id === "credential").text;
    expect(entry).toContain("[[credential]]");
    expect(entry).toContain('url = "https://hub.example.net"');
    expect(entry).toContain('member = "laptop"');
    expect(entry).toContain('token = "tok-secret-123"');
    expect(setup[0].snippets.find((s) => s.id === "env").text).toContain("export HUB_TOKEN=tok-secret-123");
  });

  it("names the project file's keys and never a token key", () => {
    const file = setup[1].snippets.find((s) => s.id === "project-file").text;
    expect(file).toContain('member = "laptop"');
    expect(file).toContain('url = "https://hub.example.net"');
    expect(file).not.toMatch(/token/i);
  });

  it("runs the binary for each client when it is there, and names no variable", () => {
    const texts = Object.fromEntries(setup[1].snippets.map((s) => [s.id, s.text]));
    expect(JSON.parse(texts.claude).mcpServers["agent-hub"]).toEqual({ command: "agent-hub", args: ["mcp"] });
    expect(JSON.parse(texts.opencode).mcp["agent-hub"]).toEqual({ type: "local", command: ["agent-hub", "mcp"] });
    expect(JSON.parse(texts.antigravity).mcpServers["agent-hub"]).toEqual({ command: "agent-hub", args: ["mcp"] });
    expect(flat("binary")).not.toContain("HUB_TOKEN");
  });

  it("reads the token from the environment by the syntax each client expands, with no Antigravity entry", () => {
    const texts = Object.fromEntries(setup[2].snippets.map((s) => [s.id, s.text]));
    expect(JSON.parse(texts["claude-env"]).mcpServers["agent-hub"].headers.Authorization).toBe("Bearer ${HUB_TOKEN}");
    expect(JSON.parse(texts["claude-env"]).mcpServers["agent-hub"].url).toBe("https://hub.example.net/mcp");
    expect(JSON.parse(texts["opencode-env"]).mcp["agent-hub"].headers.Authorization).toBe("Bearer {env:HUB_TOKEN}");
    expect(setup[2].snippets.map((s) => s.label).join("\n")).not.toContain("Antigravity");
  });

  it("does not claim the environment form finds its token in the store", () => {
    expect(setup[2].hint).toContain("HUB_TOKEN");
    expect(setup[2].hint).not.toMatch(/machine's store/);
    expect(setup[1].hint).toMatch(/machine's store/);
  });
});

describe("a member's name", () => {
  it("takes lower case, digits, dots and dashes, led by a letter or digit", () => {
    for (const ok of ["laptop", "work-laptop", "nas.lan", "a", "9lives"]) expect(memberNameProblem(ok)).toBe("");
    for (const bad of ["Laptop", "-laptop", "lap top", "laptop/claude", ".x"]) expect(memberNameProblem(bad)).not.toBe("");
    expect(memberNameProblem("")).toBe("Member name is required.");
    expect(MEMBER_NAME.test("x".repeat(64))).toBe(false);
    expect(MEMBER_NAME.test("x".repeat(63))).toBe(true);
  });
});

describe("the approval card", () => {
  const pending = {
    id: "desktop-7",
    display_name: "desktop-7",
    state: "pending",
    enrol: { note: "Setting up a build box <b>now</b>", source: "10.0.0.8", agent: "claude-code", os: "linux", host: "desktop-7" },
  };

  it("offers the name to rename, what the machine said about itself, and the reach of an approval", () => {
    const box = document.createElement("div");
    box.innerHTML = approvalCardHTML(pending);
    expect(box.querySelector("input[name='member']").value).toBe("desktop-7");
    const facts = box.querySelector(".member-facts").textContent;
    for (const word of ["claude-code", "linux", "desktop-7", "10.0.0.8"]) expect(facts).toContain(word);
    expect(box.querySelector(".member-approve-note").textContent).toBe("Approving covers every agent on desktop-7.");
    expect([...box.querySelectorAll("button")].map((b) => b.textContent)).toEqual(["Approve", "Refuse"]);
  });

  it("words the reach of an approval from the name it is approved under", () => {
    expect(approveNote("nas")).toBe("Approving covers every agent on nas.");
  });

  it("shows the reason as text, not markup", () => {
    const box = document.createElement("div");
    box.innerHTML = approvalCardHTML(pending);
    expect(box.querySelector(".member-enrol-note").textContent).toBe("Setting up a build box <b>now</b>");
    expect(box.querySelector(".member-enrol-note b")).toBeNull();
  });

  it("leaves out a fact the machine did not give", () => {
    const box = document.createElement("div");
    box.innerHTML = approvalCardHTML({ id: "x", state: "pending", enrol: { agent: "cli" } });
    expect(box.querySelectorAll(".member-fact")).toHaveLength(1);
    expect(box.querySelector(".member-enrol-reason")).toBeNull();
  });
});

describe("the agents a member has run", () => {
  it("collects the agent that asked to join and every agent in a session of the member's", () => {
    const member = { id: "laptop", enrol: { agent: "cli" } };
    const sessions = [
      { participants: [{ member: "laptop", agent: "opencode" }, { member: "desktop", agent: "claude-code" }] },
      { participants: [{ member: "laptop", agent: "claude-code" }, { member: "laptop", agent: "opencode" }] },
    ];
    expect(agentsSeen(member, sessions)).toEqual(["claude-code", "cli", "opencode"]);
    expect(agentsSeen({ id: "x" }, [])).toEqual([]);
  });
});

describe("the Who menu", () => {
  const dir = {
    members: [{ id: "laptop", display_name: "Work laptop" }, { id: "nas" }],
    agents: ["claude-code", "opencode"],
    sessions: [{ id: "s1", session_name: "feat-x" }],
    legacy: true,
  };

  beforeEach(() => {
    setMemberCount(2);
    setWho("t", "", "");
  });

  it("sends the hub's own filter for each kind of choice, one at a time", () => {
    expect(whoParams("t")).toBe("");
    expect(whoQuery("t")).toBe("");
    setWho("t", "member", "laptop");
    expect(whoParams("t")).toBe("member=laptop");
    expect(whoQuery("t")).toBe("&member=laptop");
    setWho("t", "agent", "claude-code");
    expect(whoParams("t")).toBe("agent=claude-code");
    setWho("t", "session", "01J/odd id");
    expect(whoParams("t")).toBe("session=01J%2Fodd%20id");
    setWho("t", "kind", "legacy");
    expect(whoParams("t")).toBe("kind=legacy");
    expect(whoActive("t")).toBe(true);
    setWho("t", "", "");
    expect(whoActive("t")).toBe(false);
    expect(whoPick("t")).toBeNull();
  });

  it("ignores a key the hub has no filter for", () => {
    setWho("t", "colour", "red");
    expect(whoActive("t")).toBe(false);
  });

  it("keeps each scope's choice to itself", () => {
    setWho("a", "member", "laptop");
    expect(whoParams("b")).toBe("");
    setWho("a", "", "");
  });

  it("gives every project's Artifacts segment and the global list a scope of their own", () => {
    expect(artifactsScope()).toBe("artifacts");
    expect(artifactsScope("x")).toBe("artifacts:x");
    setWho(artifactsScope("x"), "member", "laptop", "Work laptop");
    expect(whoActive(artifactsScope("x"))).toBe(true);
    expect(whoActive(artifactsScope("y"))).toBe(false);
    expect(whoActive(artifactsScope())).toBe(false);
    expect(whoParams(artifactsScope("y"))).toBe("");
    setWho(artifactsScope("x"), "", "");
  });

  it("keeps the name a choice was made under, so the trigger never shows an id", () => {
    setWho("g", "session", "01JABCDEFGHJKMNPQRSTVWXYZ0", "feat-x");
    expect(whoPick("g")).toEqual({ key: "session", value: "01JABCDEFGHJKMNPQRSTVWXYZ0", label: "feat-x" });
    const box = document.createElement("div");
    box.innerHTML = whoMenu("g", { members: [], agents: [], sessions: [] });
    expect(box.querySelector("[data-group-toggle]").textContent).toBe("Who: feat-x");
    setWho("g", "", "");
  });

  it("lists anyone, members, agents, sessions and the two kinds, with the choice checked", () => {
    setWho("t", "member", "nas", "nas");
    const box = document.createElement("div");
    box.innerHTML = whoMenu("t", dir);
    const items = [...box.querySelectorAll("[role='menuitemradio']")];
    expect(items.map((i) => i.textContent)).toEqual([
      "Anyone",
      "Work laptop",
      "nas",
      "claude-code",
      "opencode",
      "feat-x",
      "You",
      "Before members",
    ]);
    expect(items.filter((i) => i.getAttribute("aria-checked") === "true").map((i) => i.textContent)).toEqual(["nas"]);
    expect(box.querySelector("[data-group-toggle]").getAttribute("aria-haspopup")).toBe("menu");
    setWho("t", "", "");
  });

  it("names the choice in the trigger itself, with no pill beside it", () => {
    setWho("t", "member", "laptop", "Work laptop");
    const box = document.createElement("div");
    box.innerHTML = whoMenu("t", dir);
    expect(box.querySelector("[data-group-toggle]").textContent).toBe("Who: Work laptop");
    expect(box.querySelector(".pill")).toBeNull();
    setWho("t", "", "");
  });

  it("checks Anyone and names it Who when nothing is chosen", () => {
    const box = document.createElement("div");
    box.innerHTML = whoMenu("t", dir);
    expect(box.querySelector("[aria-checked='true']").textContent).toBe("Anyone");
    expect(box.querySelector("[data-group-toggle]").getAttribute("aria-label")).toBe("Who");
  });

  it("draws the trigger as the person glyph and the chevron when nothing is chosen", () => {
    const box = document.createElement("div");
    box.innerHTML = whoMenu("t", dir);
    const toggle = box.querySelector("[data-group-toggle]");
    expect(toggle.querySelectorAll("svg")).toHaveLength(2);
    expect(toggle.classList.contains("is-picked")).toBe(false);
    expect(toggle.textContent).toBe("");
  });

  it("offers the Members group only from two members, and Before members only where there are such rows", () => {
    const names = (d) => {
      const box = document.createElement("div");
      box.innerHTML = whoMenu("t", d);
      return [...box.querySelectorAll("[role='group']")].map((g) => g.getAttribute("aria-label"));
    };
    expect(names({ ...dir, members: [{ id: "laptop" }], legacy: false })).toEqual(["Agents", "Sessions", "Other"]);
    expect(names(dir)).toEqual(["Members", "Agents", "Sessions", "Other"]);
    const box = document.createElement("div");
    box.innerHTML = whoMenu("t", { ...dir, legacy: false });
    expect([...box.querySelectorAll("[data-key='kind']")].map((i) => i.textContent)).toEqual(["You"]);
  });

  it("names a session with no name by its agent and the tail of its id", () => {
    const box = document.createElement("div");
    box.innerHTML = whoMenu("t", {
      members: [],
      agents: [],
      sessions: [{ id: "01JAAAAAAAAAAAAAAAAAB3FA9Z", session_name: "", participants: [{ agent: "opencode" }] }],
    });
    expect(box.querySelector("[data-key='session']").textContent).toBe("opencode …B3FA9Z");
  });

  it("sets subagents in under their parent and sorts ended sessions last, marked", () => {
    const box = document.createElement("div");
    box.innerHTML = whoMenu("t", {
      members: [],
      agents: [],
      sessions: [
        { id: "old", session_name: "old-run", status: "ended" },
        { id: "k1", session_name: "feat-x/review-3fa9", parent_id: "p", status: "active" },
        { id: "p", session_name: "feat-x", status: "active" },
        { id: "k2", session_name: "feat-x/lint", parent_id: "p", status: "ended" },
      ],
    });
    const sessions = [...box.querySelectorAll("[data-key='session']")];
    expect(sessions.map((i) => i.textContent)).toEqual(["feat-x", "review-3fa9", "lint (ended)", "old-run (ended)"]);
    expect(sessions.map((i) => i.classList.contains("who-child"))).toEqual([false, true, true, false]);
    expect(sessions[1].getAttribute("aria-label")).toBe("feat-x/review-3fa9");
    expect(sessions[1].dataset.value).toBe("k1");
  });

  it("is still a menu on a hub that has not listed anyone yet", () => {
    const box = document.createElement("div");
    box.innerHTML = whoMenu("t");
    expect([...box.querySelectorAll("[role='menuitemradio']")].map((i) => i.textContent)).toEqual(["Anyone", "You"]);
  });

  it("escapes a name an agent chose", () => {
    const box = document.createElement("div");
    box.innerHTML = whoMenu("t", { members: [], agents: [], sessions: [{ id: "s", session_name: '<img src=x onerror="1">' }] });
    expect(box.querySelector("img")).toBeNull();
    expect(box.textContent).toContain('<img src=x onerror="1">');
  });
});
