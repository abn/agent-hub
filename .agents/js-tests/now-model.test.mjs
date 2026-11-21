// The words Now opens and closes with, and how it splits what the hub sent into
// tiers. Held here as pure functions: a rendered page would only prove the
// markup the functions return.

import { describe, expect, it } from "vitest";
import {
  askQueue,
  canWait,
  clip,
  endingLine,
  eventSentenceHTML,
  askSentenceHTML,
  needsYou,
  needsYouCount,
  queuePlace,
  readSnoozes,
  relatedAsks,
  replySentenceHTML,
  sinceAndQuiet,
  sinceOldest,
  snooze,
  summaryHTML,
  threadReplies,
  unsnooze,
  word,
} from "../../web/now-model.mjs";

const NOW = Date.parse("2026-11-21T22:40:00Z");

const ask = (actor, summary, fields = {}) => ({
  event_id: `e-${actor}-${summary.length}`,
  actor,
  summary,
  kind: "question",
  status: "action",
  ...fields,
});

// The visible words of a line, with the markup taken off, so a check reads the
// sentence a reader reads.
const text = (html) => {
  const box = document.createElement("div");
  box.innerHTML = html;
  return box.textContent.replace(/\s+/g, " ").trim();
};

const memory = () => {
  const map = new Map();
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
  };
};

describe("the opening line", () => {
  it("says plainly that nothing waits and nothing is new", () => {
    expect(text(summaryHTML({}))).toBe("Nothing waits on you. Nothing is new since you looked.");
  });

  it("names the working agents when nothing waits", () => {
    expect(text(summaryHTML({ agents: 2 }))).toBe(
      "Nothing waits on you. Nothing is new since you looked. Two agents are working.",
    );
  });

  it("counts what changed when nothing waits", () => {
    expect(text(summaryHTML({ since: 1 }))).toBe("Nothing waits on you. One thing changed since you looked.");
    expect(text(summaryHTML({ since: 14 }))).toBe("Nothing waits on you. 14 things changed since you looked.");
  });

  it("names the one agent and its ask, and keeps the ask's own question mark", () => {
    expect(text(summaryHTML({ now: [ask("docs-writer", "Keep the CSV export?")] }))).toBe(
      "One agent needs you. docs-writer asks “Keep the CSV export?” Nothing else is new.",
    );
  });

  it("closes an ask without its own stop with a full stop", () => {
    const line = text(summaryHTML({ now: [ask("build-bot", "Ship 1.10 tonight", { kind: "approval" })] }));
    expect(line).toBe("One agent needs you. build-bot wants a decision on “Ship 1.10 tonight”. Nothing else is new.");
  });

  it("names two agents with an and between them", () => {
    const line = text(
      summaryHTML({
        now: [ask("build-bot", "Ship 1.10 tonight", { kind: "approval" }), ask("docs-writer", "Keep the CSV export?")],
        since: 5,
      }),
    );
    expect(line).toBe(
      "Two agents need you. build-bot wants a decision on “Ship 1.10 tonight”, and docs-writer asks “Keep the CSV export?” Five things changed since you looked.",
    );
  });

  it("counts asks apart from agents when one agent asks twice", () => {
    const line = text(summaryHTML({ now: [ask("build-bot", "Ship it?"), ask("build-bot", "Roll back?")] }));
    expect(line).toBe("One agent needs you, with two asks. build-bot asks “Ship it?”, and one more. Nothing else is new.");
  });

  it("names two of many and counts the rest, one per agent first", () => {
    const now = [
      ask("a", "First?"),
      ask("a", "Second?"),
      ask("b", "Third?", { kind: "idea" }),
      ask("c", "Fourth?"),
      ask("d", "Fifth?", { kind: "signal" }),
    ];
    const line = text(summaryHTML({ now, later: 1 }));
    expect(line).toBe(
      "Four agents need you, with five asks. a asks “First?”, b left an idea, “Third?”, and three more. One more can wait. Nothing else is new.",
    );
  });

  it("says what can wait when only that is left", () => {
    expect(text(summaryHTML({ later: 2 }))).toBe("Nothing waits on you. Two more can wait. Nothing else is new.");
  });

  it("escapes every value an agent wrote", () => {
    const html = summaryHTML({
      now: [ask('<img src=x onerror="1">', 'drop <b id="pwned">all</b> & "go"')],
    });
    expect(html).not.toContain("<img");
    expect(html).not.toContain('<b id="pwned">');
    expect(html).toContain("&lt;img src=x onerror=&quot;1&quot;&gt;");
    expect(html).toContain("&amp;");
    const box = document.createElement("div");
    box.innerHTML = html;
    expect(box.querySelector("img, #pwned")).toBeNull();
    expect(box.querySelectorAll("b")).toHaveLength(1);
  });

  it("cuts a long ask at a word so one ask cannot take the line", () => {
    const long = "Rotate the signing key on every build host in the fleet before the release goes out on Monday";
    const cut = clip(long);
    expect(cut.length).toBeLessThanOrEqual(73);
    expect(cut.endsWith("…")).toBe(true);
    expect(long.startsWith(cut.slice(0, -1))).toBe(true);
  });

  it("writes small counts as words and large ones as figures", () => {
    expect([word(0), word(1), word(12), word(13)]).toEqual(["no", "one", "twelve", "13"]);
  });
});

describe("the row sentences", () => {
  it("puts the agent first in semibold and escapes the rest", () => {
    const box = document.createElement("div");
    box.innerHTML = askSentenceHTML(ask("build-bot", "Drop <i>staging</i>", { kind: "approval" }));
    expect(box.querySelector("b").textContent).toBe("build-bot");
    expect(box.querySelector("i")).toBeNull();
    expect(box.textContent).toBe("build-bot wants a decision: Drop <i>staging</i>");
  });

  it("gives a publish no second verb, because its summary already has one", () => {
    expect(text(eventSentenceHTML({ actor: "a", kind: "artifact", summary: "published Notes" }))).toBe("a: published Notes");
    expect(text(eventSentenceHTML({ actor: "a", kind: "finished", summary: "nightly done" }))).toBe("a finished: nightly done");
    expect(text(eventSentenceHTML({ actor: "a", kind: "constructor", summary: "x" }))).toBe("a: x");
  });
});

describe("the closing line", () => {
  it("says how far back the stream reaches and what is left", () => {
    const oldest = new Date(NOW);
    oldest.setHours(18, 5, 0, 0);
    const now = new Date(NOW);
    now.setHours(22, 40, 0, 0);
    expect(endingLine({ needs: 2, oldest: oldest.getTime(), now: now.getTime() })).toBe(
      "That is everything since 18:05. Two answers and the desk is clear.",
    );
    expect(endingLine({ needs: 1, oldest: null, now: now.getTime() })).toBe(
      "That is everything. One answer and the desk is clear.",
    );
    expect(endingLine({ needs: 0, oldest: null })).toBe("That is everything.");
  });
});

describe("the tiers", () => {
  it("lets an ask wait when its deadline is more than a day off or it is snoozed", () => {
    const soon = ask("a", "Soon?", { expires_at: new Date(NOW + 3 * 3600e3).toISOString() });
    const far = ask("b", "Monday?", { expires_at: new Date(NOW + 2 * 86400e3).toISOString() });
    const snoozed = ask("c", "Later?");
    const snoozes = { [snoozed.event_id]: NOW + 1000 };
    expect(canWait(soon, { now: NOW, snoozes })).toBe(false);
    expect(canWait(far, { now: NOW, snoozes })).toBe(true);
    expect(canWait(snoozed, { now: NOW, snoozes })).toBe(true);
    expect(canWait(snoozed, { now: NOW + 2000, snoozes })).toBe(false);
  });

  it("counts what needs the reader now, and the asks the response did not list", () => {
    const far = ask("b", "Monday?", { expires_at: new Date(NOW + 2 * 86400e3).toISOString() });
    const data = { waiting: 4, waiting_items: [ask("a", "Now?"), far, ask("c", "Done?", { status: "resolved" })] };
    const split = needsYou(data, { now: NOW });
    expect(split.now.map((item) => item.actor)).toEqual(["a"]);
    expect(split.later.map((item) => item.actor)).toEqual(["b"]);
    expect(split.unlisted).toBe(2);
    expect(needsYouCount(data, { now: NOW })).toBe(3);
  });

  it("groups what is new by project and folds the rest", () => {
    const event = (id, project, fields = {}) => ({
      id,
      project_id: project,
      project_display_name: project.toUpperCase(),
      actor: "a",
      kind: "signal",
      summary: id,
      created_at: new Date(NOW - Number(id.slice(1)) * 60e3).toISOString(),
      inbox_status: null,
      ...fields,
    });
    const data = {
      recent: [
        event("e1", "checks"),
        event("e2", "lineage", { kind: "approval", inbox_status: "action" }),
        event("e3", "checks"),
        event("e4", "lineage", { kind: "finished", inbox_status: "unread" }),
        event("e5", "lineage"),
      ],
      unseen: [
        { project_id: "checks", events: 1 },
        { project_id: "attic", events: 3 },
      ],
    };
    const { since, quiet, count } = sinceAndQuiet(data);
    expect(since.map((g) => [g.id, g.rows.map((e) => e.id), g.more])).toEqual([
      ["checks", ["e1"], 0],
      ["lineage", ["e4"], 0],
      ["attic", [], 3],
    ]);
    expect(quiet.map((e) => e.id)).toEqual(["e3", "e5"]);
    expect(count).toBe(5);
    expect(sinceOldest(since)).toBe(Date.parse(data.recent[3].created_at));
  });
});

describe("snoozes", () => {
  it("remembers a snooze for an hour on this device and forgets it after", () => {
    const storage = memory();
    snooze("e1", NOW, storage);
    expect(readSnoozes(NOW + 1000, storage)).toEqual({ e1: NOW + 3600e3 });
    expect(readSnoozes(NOW + 3600e3 + 1, storage)).toEqual({});
    snooze("e2", NOW, storage);
    unsnooze("e2", NOW, storage);
    expect(readSnoozes(NOW, storage)).toEqual({});
  });

  it("reads a store it cannot parse as no snoozes", () => {
    const storage = memory();
    storage.setItem("hub.snooze", "{not json");
    expect(readSnoozes(NOW, storage)).toEqual({});
  });
});

describe("the opened item's queue", () => {
  const item = (id, fields = {}) => ({ event_id: id, actor: "a", summary: id, kind: "question", status: "action", project_id: "p", ...fields });
  const later = new Date(NOW + 3 * 86400e3).toISOString();
  const items = [
    item("e1"),
    item("e3", { expires_at: later }),
    item("e2", { status: "waiting" }),
    item("e4", { status: "resolved" }),
    item("e2"),
  ];

  it("walks what needs the reader in the hub's order, then what can wait, each once", () => {
    const queue = askQueue(items, { now: NOW, snoozes: {} });
    expect(queue.now.map((i) => i.event_id)).toEqual(["e2", "e1"]);
    expect(queue.later.map((i) => i.event_id)).toEqual(["e3"]);
    expect(queue.all.map((i) => i.event_id)).toEqual(["e2", "e1", "e3"]);
  });

  it("says where an item sits, what is either side and what opens after it", () => {
    const queue = askQueue(items, { now: NOW, snoozes: {} });
    const first = queuePlace(queue, "e2");
    expect(first.place).toBe("1 of 2 need you");
    expect(first.previous).toBeNull();
    expect(first.next.event_id).toBe("e1");
    expect(first.after.event_id).toBe("e1");
    const last = queuePlace(queue, "e3");
    expect(last.place).toBe("1 of 1 can wait");
    expect(last.next).toBeNull();
    expect(last.previous.event_id).toBe("e1");
    // The last ask answered hands over to the first one still waiting.
    expect(last.after.event_id).toBe("e2");
    const outside = queuePlace(queue, "e4");
    expect(outside).toMatchObject({ place: "", previous: null });
    expect(outside.next.event_id).toBe("e2");
  });

  it("hands over to nothing when the item is the only ask", () => {
    const queue = askQueue([item("e1")], { now: NOW, snoozes: {} });
    expect(queuePlace(queue, "e1")).toMatchObject({ place: "1 of 1 need you", previous: null, next: null, after: null });
  });

  it("relates the asks from the same session first, then the same project", () => {
    const queue = askQueue(
      [
        item("e1", { session_id: "s1" }),
        item("e2", { project_id: "q" }),
        item("e3"),
        item("e4", { session_id: "s1" }),
      ],
      { now: NOW, snoozes: {} },
    );
    const related = relatedAsks(queue.all.find((i) => i.event_id === "e1"), queue);
    expect(related.map((r) => [r.item.event_id, r.why])).toEqual([
      ["e4", "session"],
      ["e3", "project"],
    ]);
  });
});

describe("the opened item's thread", () => {
  it("keeps the replies to the item, oldest first", () => {
    const events = [
      { id: "e9", thread_id: "e1" },
      { id: "e5", thread_id: "e2" },
      { id: "e3", thread_id: "e1" },
      { id: "e1", thread_id: "e1" },
    ];
    expect(threadReplies("e1", events).map((e) => e.id)).toEqual(["e3", "e9"]);
  });

  it("reads an answer's body and escapes what the agent wrote", () => {
    expect(text(replySentenceHTML({ kind: "answer", actor: "human", summary: "re: x", payload: { body: "Approved" } }))).toBe(
      "human answered: Approved",
    );
    const html = replySentenceHTML({ kind: "signal", actor: "bot", summary: "<i>later</i>" });
    expect(html).not.toContain("<i>");
    expect(text(html)).toBe("bot added: <i>later</i>");
  });
});
