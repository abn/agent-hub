// Who wrote a row, in words. The byline is a subject and receipts, and which
// rule produced them depends on the author's kind and the row's origin, so each
// is held here on the data the hub sends rather than on a page.

import { beforeEach, describe, expect, it } from "vitest";
import {
  LEGACY_RECEIPT,
  liveCountText,
  liveCounts,
  liveWords,
  authorOf,
  askerKey,
  byline,
  bylineOf,
  bylineText,
  foldSessions,
  hasSession,
  isLive,
  isOutsider,
  isYou,
  leafName,
  participantsText,
  receiptsHTML,
  sessionCountText,
  sessionTitle,
  setMemberCount,
  stateWord,
  subjectHTML,
  verb,
} from "../../web/author.mjs";

const participant = (over = {}) => ({
  kind: "participant",
  member: "laptop",
  agent: "claude-code",
  session: { id: "s1", name: "feat-x" },
  ...over,
});

beforeEach(() => setMemberCount(2));

describe("a participant's byline", () => {
  it("is the session in the subject and agent and member in the receipts", () => {
    expect(byline(participant())).toEqual({ kind: "participant", subject: "feat-x", receipts: "claude-code · laptop" });
  });

  it("is the agent in the subject and the member in the receipts when no session was named", () => {
    expect(byline(participant({ session: null }))).toEqual({ kind: "participant", subject: "claude-code", receipts: "laptop" });
  });

  it("drops the member from the receipts while the hub has one member", () => {
    setMemberCount(1);
    expect(byline(participant()).receipts).toBe("claude-code");
    expect(byline(participant({ session: null }))).toMatchObject({ subject: "claude-code", receipts: "" });
  });

  it("keeps the member when the hub will not say how many there are", () => {
    setMemberCount(null);
    expect(byline(participant()).receipts).toBe("claude-code · laptop");
  });

  it("falls back to the member when a migrated row has no agent", () => {
    expect(byline(participant({ agent: "", session: null }))).toMatchObject({ subject: "laptop", receipts: "" });
  });

  it("takes the solo flag from the caller when it is given", () => {
    expect(byline(participant(), null, { solo: true }).receipts).toBe("claude-code");
    expect(byline(participant(), null, { solo: false }).receipts).toBe("claude-code · laptop");
  });
});

describe("the other authors", () => {
  it("reads the human as you and the hub's own act as the hub", () => {
    expect(byline({ kind: "human" })).toMatchObject({ kind: "human", subject: "you", receipts: "" });
    expect(byline({ kind: "human" }, "hub")).toMatchObject({ kind: "hub", subject: "the hub", receipts: "" });
  });

  it("names local in the receipts even on a hub with one member", () => {
    setMemberCount(1);
    expect(byline({ kind: "local", agent: "claude-code", session: { id: "s2", name: "scratch" } })).toMatchObject({
      subject: "scratch",
      receipts: "claude-code · local",
    });
    expect(byline({ kind: "local", agent: "claude-code", session: null })).toMatchObject({ subject: "claude-code", receipts: "local" });
  });

  it("keeps a legacy label as the subject, with a visible receipt that says why there are no others", () => {
    expect(byline({ kind: "legacy", label: "laptop/claude" })).toEqual({
      kind: "legacy",
      subject: "laptop/claude",
      receipts: LEGACY_RECEIPT,
    });
    expect(LEGACY_RECEIPT).toBe("before members");
  });

  it("says before members once when a legacy row has no label at all", () => {
    expect(byline({ kind: "legacy", label: null })).toMatchObject({ subject: "before members", receipts: "" });
  });

  it("reads the two labels that were never agents as the people they were", () => {
    expect(byline({ kind: "legacy", label: "human" })).toMatchObject({ kind: "human", subject: "you", receipts: LEGACY_RECEIPT });
    expect(byline({ kind: "legacy", label: "hub" })).toMatchObject({ kind: "hub", subject: "the hub", receipts: LEGACY_RECEIPT });
  });
});

describe("a row's author", () => {
  it("reads the typed author and origin off a row", () => {
    const row = { actor: "laptop", author: participant(), origin: null };
    expect(authorOf(row)).toEqual({ author: row.author, origin: null });
    expect(bylineOf({ author: { kind: "human" }, origin: "hub" }).subject).toBe("the hub");
  });

  it("reads a row with no author record as legacy under its old label", () => {
    expect(authorOf({ actor: "laptop/claude" })).toEqual({ author: { kind: "legacy", label: "laptop/claude" }, origin: null });
    // A comment from before members carried the label in `author` itself.
    expect(authorOf({ author: "laptop/claude" }).author).toEqual({ kind: "legacy", label: "laptop/claude" });
  });

  it("never shows the opaque actor key of a row that has an author", () => {
    const row = { actor: "laptop", author: participant() };
    expect(bylineText(row)).toBe("feat-x (claude-code · laptop)");
    expect(bylineText(row)).not.toBe("laptop");
  });
});

describe("the markup", () => {
  it("sets the subject in a bold span and escapes what an agent called itself", () => {
    const html = subjectHTML({ author: participant({ session: { id: "s", name: '<img src=x onerror="1">' } }) });
    const box = document.createElement("div");
    box.innerHTML = html;
    expect(box.querySelector("img")).toBeNull();
    expect(box.querySelector("b.by-s").textContent).toBe('<img src=x onerror="1">');
  });

  it("draws a legacy row's reason as a receipt, not a hover", () => {
    const box = document.createElement("div");
    box.innerHTML = subjectHTML({ actor: "old-bot" }) + receiptsHTML({ actor: "old-bot" });
    expect(box.querySelector("b").hasAttribute("title")).toBe(false);
    expect(box.querySelector(".by-r").textContent).toBe("before members");
  });

  it("lets a long name break at its dashes and slashes, whole and never cut", () => {
    const long = "release-coordinator/for-the-edge.hosts";
    const box = document.createElement("div");
    box.innerHTML = subjectHTML({ author: participant({ session: { id: "s", name: long } }) });
    const b = box.querySelector("b");
    expect(b.textContent).toBe(long);
    expect(b.hasAttribute("title")).toBe(false);
    expect(b.querySelectorAll("wbr").length).toBe(5);
    expect(b.innerHTML.startsWith("release-<wbr>coordinator/<wbr>")).toBe(true);
  });

  it("draws receipts in mono, and nothing when there are none", () => {
    const box = document.createElement("div");
    box.innerHTML = receiptsHTML({ author: participant() });
    expect(box.querySelector(".by-r.mono").textContent).toBe("claude-code · laptop");
    expect(receiptsHTML({ author: { kind: "human" } })).toBe("");
    expect(receiptsHTML({ actor: "old-bot" })).toContain("before members");
  });
});

describe("who is the same asker", () => {
  it("is the session when there is one, whoever wrote in it", () => {
    const a = { author: participant({ agent: "claude-code" }) };
    const b = { author: participant({ agent: "opencode" }) };
    expect(askerKey(a)).toBe(askerKey(b));
  });

  it("is the agent and member when there is no session", () => {
    const a = { author: participant({ session: null, agent: "claude-code" }) };
    const b = { author: participant({ session: null, agent: "opencode" }) };
    expect(askerKey(a)).not.toBe(askerKey(b));
  });
});

describe("a session's people", () => {
  const people = [
    { member: "laptop", agent: "claude-code", left_at: null },
    { member: "laptop", agent: "opencode", left_at: null },
    { member: "desktop", agent: "claude-code", left_at: "2026-11-23T10:00:00Z" },
  ];

  it("groups agents under their member and leaves out whoever has left", () => {
    expect(participantsText(people)).toBe("claude-code, opencode · laptop");
  });

  it("lists everyone who ever took part when asked", () => {
    expect(participantsText(people, { includeLeft: true })).toBe("claude-code, opencode · laptop, claude-code · desktop");
  });

  it("names no member on a hub with one", () => {
    expect(participantsText(people, { solo: true })).toBe("claude-code, opencode");
  });

  it("says nothing for a session with nobody in it", () => {
    expect(participantsText([])).toBe("");
    expect(participantsText(undefined)).toBe("");
  });
});

describe("subagents", () => {
  it("reads a child as its last segment", () => {
    expect(leafName("feat-x/review-3fa9")).toBe("review-3fa9");
    expect(leafName("feat-x")).toBe("feat-x");
  });

  it("folds children under their parent with a count", () => {
    const rows = [
      { id: "p", session_name: "feat-x", parent_id: null },
      { id: "c1", session_name: "feat-x/a", parent_id: "p" },
      { id: "c2", session_name: "feat-x/b", parent_id: "p" },
      { id: "c3", session_name: "feat-x/c", parent_id: "p" },
      { id: "o", session_name: "other", parent_id: null },
    ];
    const folded = foldSessions(rows);
    expect(folded.map((entry) => entry.session.id)).toEqual(["p", "o"]);
    expect(folded[0].folded).toBe("+3 subagents");
    expect(folded[0].children.map((child) => child.id)).toEqual(["c1", "c2", "c3"]);
    expect(folded[1].folded).toBe("");
  });

  it("says one subagent in the singular", () => {
    const folded = foldSessions([
      { id: "p", session_name: "feat-x" },
      { id: "c", session_name: "feat-x/a", parent_id: "p" },
    ]);
    expect(folded[0].folded).toBe("+1 subagent");
  });

  it("keeps a child whose parent is not listed under its full name, and counts the ones the hub did not list", () => {
    const folded = foldSessions([
      { id: "c", session_name: "feat-x/a", parent_id: "gone" },
      { id: "p", session_name: "solo", children_count: 2 },
    ]);
    expect(folded.map((entry) => entry.session.id)).toEqual(["c", "p"]);
    expect(folded[1].folded).toBe("+2 subagents");
  });
});

describe("the count over live sessions", () => {
  const rows = [
    { id: "p", session_name: "feat-x" },
    { id: "c", session_name: "feat-x/a", parent_id: "p" },
    { id: "o", session_name: "other" },
  ];

  it("counts the sessions that head a row and the subagents under them", () => {
    expect(liveCounts(rows)).toEqual({ sessions: 2, subagents: 1 });
    expect(liveCountText(rows)).toBe("2 live sessions, 1 subagent");
  });

  it("does not count children the hub did not list, which may have ended", () => {
    expect(liveCounts([{ id: "p", session_name: "feat-x", children_count: 3 }])).toEqual({ sessions: 1, subagents: 0 });
    expect(liveCountText([{ id: "p", session_name: "feat-x", children_count: 3 }])).toBe("1 live session");
  });

  it("counts a live child whose parent is not listed as a subagent, as the hub does", () => {
    const orphan = { id: "c", session_name: "feat-x/a", parent_id: "idle-parent" };
    expect(liveCounts([...rows, orphan])).toEqual({ sessions: 2, subagents: 2 });
    expect(liveCountText([...rows, orphan])).toBe("2 live sessions, 2 subagents");
    expect(liveCountText([orphan])).toBe("0 live sessions, 1 subagent");
  });

  it("folds only the children a live list holds", () => {
    const folded = foldSessions(
      [
        { id: "p", session_name: "feat-x", children_count: 3 },
        { id: "q", session_name: "other", children_count: 1 },
        { id: "k", session_name: "other/a", parent_id: "q" },
      ],
      { live: true },
    );
    expect(folded.map((entry) => entry.folded)).toEqual(["", "+1 subagent"]);
  });

  it("says no live sessions when there are none", () => {
    expect(liveCountText([])).toBe("No live sessions");
  });
});

describe("you and the hub", () => {
  const human = { author: { kind: "human" }, origin: null };
  const hub = { author: { kind: "human" }, origin: "hub" };

  it("tells the human and the hub from an agent", () => {
    expect(isYou(human)).toBe(true);
    expect(isYou(hub)).toBe(false);
    expect(isOutsider(human)).toBe(true);
    expect(isOutsider(hub)).toBe(true);
    expect(isOutsider({ author: participant() })).toBe(false);
    expect(isOutsider({ actor: "human" })).toBe(true);
    expect(isOutsider({ actor: "laptop/claude" })).toBe(false);
  });

  it("conjugates for you and for anyone else", () => {
    expect(verb(human, "asks", "ask")).toBe("ask");
    expect(verb(hub, "asks", "ask")).toBe("asks");
    expect(verb({ author: participant() }, "is", "are")).toBe("is");
  });

  it("says whether a row named its session", () => {
    expect(hasSession({ author: participant() })).toBe(true);
    expect(hasSession({ author: participant({ session: null }) })).toBe(false);
    expect(hasSession({ actor: "laptop/claude" })).toBe(false);
  });
});

describe("a session that is live or idle", () => {
  it("is live when the hub says it is", () => {
    expect(isLive({ status: "active", live: true })).toBe(true);
    expect(stateWord({ status: "active", live: true })).toBe("live");
  });

  it("is idle when it is active and the hub does not say live", () => {
    expect(stateWord({ status: "active", live: false })).toBe("idle");
    expect(stateWord({ status: "active" })).toBe("idle");
  });

  it("does not guess from participants", () => {
    const seen = { member: "laptop", agent: "claude-code", left_at: null, last_seen_at: new Date().toISOString() };
    expect(isLive({ status: "active", participants: [seen] })).toBe(false);
  });

  it("is neither once it has ended, and says what it is", () => {
    expect(isLive({ status: "ended", live: true })).toBe(false);
    expect(stateWord({ status: "ended" })).toBe("ended");
  });
});

describe("live counts said one way", () => {
  it("names sessions, then subagents when there are any", () => {
    expect(liveWords(2, 1)).toBe("2 live sessions, 1 subagent");
    expect(liveWords(1, 3)).toBe("1 live session, 3 subagents");
    expect(liveWords(4, 0)).toBe("4 live sessions");
    expect(liveWords(0)).toBe("0 live sessions");
  });
});

describe("what a list of sessions holds, and what an unnamed one is called", () => {
  it("counts the rows a list shows and the subagents folded under them", () => {
    const rows = [{ id: "p", status: "active" }, { id: "k1", parent_id: "p", status: "active" }, { id: "k2", parent_id: "p", status: "active" }, { id: "q", status: "active" }];
    expect(sessionCountText(rows)).toBe("2 sessions, 2 subagents");
    expect(sessionCountText([rows[0], rows[1]])).toBe("1 session, 1 subagent");
    expect(sessionCountText([rows[3]])).toBe("1 session");
  });

  it("reads an unnamed session by its agent and the tail of its id, never the whole id", () => {
    expect(sessionTitle({ session_name: "feat-x", id: "01J0000000000000000000ABCD" })).toBe("feat-x");
    expect(sessionTitle({ session_name: "", id: "01J0000000000000000000ABCD", participants: [{ agent: "opencode" }] })).toBe("opencode …00ABCD");
    expect(sessionTitle({ id: "01J0000000000000000000ABCD" })).toBe("…00ABCD");
  });
});
