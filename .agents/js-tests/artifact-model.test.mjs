// What the artifacts list says and how it tiers what the projects listed, and
// how a comment list becomes threads. Held as pure functions: a rendered page
// would only prove the markup the functions return.

import { describe, expect, it } from "vitest";
import {
  SEEN_KEY,
  artifactLine,
  artifactSlug,
  artifactTiers,
  changedSince,
  commentSummary,
  commentThreads,
  commentWords,
  listMeta,
  openThreadCount,
  projectChips,
  readSeen,
  seeAll,
  seeOne,
  versionSummary,
} from "../../web/artifact-model.mjs";

const NOW = Date.parse("2026-11-21T22:40:00Z");
const at = (minutesAgo) => new Date(NOW - minutesAgo * 60000).toISOString();

const artifact = (id, fields = {}) => ({
  id,
  project_id: "checks",
  actor: "docs-writer",
  title: id,
  kind: "markdown",
  version: 1,
  protected: false,
  size_bytes: 100,
  comments_count: 0,
  comments_open: 0,
  created_at: at(600),
  updated_at: at(600),
  ...fields,
});

function memory() {
  const map = new Map();
  return {
    getItem: (key) => (map.has(key) ? map.get(key) : null),
    setItem: (key, value) => map.set(key, String(value)),
  };
}

describe("what the reader has seen", () => {
  it("starts with nothing recorded, and a first look records every version", () => {
    const storage = memory();
    expect(readSeen(storage)).toBeNull();
    const state = seeAll([artifact("a", { version: 3 }), artifact("b")], NOW, storage);
    expect(state).toEqual({ at: NOW, seen: { a: 3, b: 1 } });
    expect(readSeen(storage)).toEqual(state);
  });

  it("adds what is marked to what was seen, leaving the rest as it was", () => {
    const storage = memory();
    seeAll([artifact("a", { version: 3 }), artifact("b", { project_id: "other" })], NOW, storage);
    const state = seeAll([artifact("a", { version: 5 })], NOW + 60000, storage);
    expect(state).toEqual({ at: NOW, seen: { a: 5, b: 1 } });
  });

  it("keeps the newer version when one artifact is read", () => {
    const storage = memory();
    seeAll([artifact("a", { version: 3 })], NOW, storage);
    seeOne("a", 2, NOW, storage);
    expect(readSeen(storage).seen.a).toBe(3);
    seeOne("a", 4, NOW, storage);
    expect(readSeen(storage).seen.a).toBe(4);
  });

  it("reads a damaged record as none", () => {
    const storage = memory();
    storage.setItem(SEEN_KEY, "{not json");
    expect(readSeen(storage)).toBeNull();
    storage.setItem(SEEN_KEY, JSON.stringify({ at: 1 }));
    expect(readSeen(storage)).toBeNull();
  });

  it("calls an artifact new when its version moved or it came after the record", () => {
    const state = { at: NOW - 60000, seen: { a: 2 } };
    expect(changedSince(artifact("a", { version: 3 }), state)).toBe(true);
    expect(changedSince(artifact("a", { version: 2 }), state)).toBe(false);
    expect(changedSince(artifact("b", { created_at: at(0) }), state)).toBe(true);
    expect(changedSince(artifact("c", { created_at: at(10) }), state)).toBe(false);
    expect(changedSince(artifact("a", { version: 9 }), null)).toBe(false);
  });
});

describe("the tiers", () => {
  const state = { at: NOW - 30 * 60000, seen: { old1: 1, old2: 1, old3: 1, moved: 2 } };
  const list = [
    artifact("old1", { updated_at: at(300) }),
    artifact("moved", { version: 3, updated_at: at(20) }),
    artifact("old2", { updated_at: at(100) }),
    artifact("fresh", { created_at: at(5), updated_at: at(5) }),
    artifact("old3", { updated_at: at(2000) }),
  ];

  it("puts what moved first, newest first, and the rest earlier, newest first", () => {
    const tiers = artifactTiers(list, state, { limit: 10 });
    expect(tiers.since.map((a) => a.id)).toEqual(["fresh", "moved"]);
    expect(tiers.earlier.map((a) => a.id)).toEqual(["old2", "old1", "old3"]);
    expect(tiers.more).toBe(0);
    expect(tiers.backTo).toBeNull();
  });

  it("folds the earlier tier past its limit and says how far back the rest goes", () => {
    const tiers = artifactTiers(list, state, { limit: 1 });
    expect(tiers.earlier.map((a) => a.id)).toEqual(["old2"]);
    expect(tiers.total).toBe(3);
    expect(tiers.more).toBe(2);
    expect(tiers.backTo).toBe(Date.parse(at(2000)));
    expect(artifactTiers(list, state, { limit: 1, all: true }).earlier).toHaveLength(3);
  });
});

describe("the words", () => {
  it("names an artifact as a file", () => {
    expect(artifactSlug("Release notes 1.10", "markdown")).toBe("release-notes-1-10.md");
    expect(artifactSlug("  nightly 212 ", "html")).toBe("nightly-212.html");
    expect(artifactSlug("", "html")).toBe("artifact.html");
  });

  it("says comments in the fewest words", () => {
    expect(commentWords(0, 0)).toBe("");
    expect(commentWords(2, 0)).toBe("2 comments");
    expect(commentWords(1, 1)).toBe("1 comment, 1 open");
    expect(commentWords(3, 1)).toBe("3 comments, 1 open");
    expect(commentSummary(0, 0)).toBe("None");
    expect(commentSummary(2, 0)).toBe("2, none open");
    expect(commentSummary(2, 1)).toBe("2, 1 open");
  });

  it("writes the line under a name", () => {
    expect(artifactLine(artifact("a", { version: 3, comments_count: 2 }))).toBe("checks · docs-writer · v3 · 2 comments");
    expect(artifactLine(artifact("a", { protected: true, actor: "archivist", project_id: "attic", version: 2 }))).toBe(
      "attic · archivist · v2 · encrypted in your browser",
    );
    expect(artifactLine(artifact("a", { label: "report" }))).toBe("checks · docs-writer · v1 · report");
    expect(artifactLine(artifact("a", { live_version: 2 }))).toBe("checks · docs-writer · v1 · being written");
    expect(artifactLine(artifact("a", { project_id: "", actor: null }))).toBe("v1");
    expect(artifactLine(artifact("a", { project_name: "Checks" }))).toBe("Checks · docs-writer · v1");
  });

  it("gives a version its label, else its description, else its size", () => {
    expect(versionSummary({ label: "3 lines changed", size_bytes: 10 })).toBe("3 lines changed");
    expect(versionSummary({ label: "", description: "notes", size_bytes: 10 })).toBe("notes");
    expect(versionSummary({ label: null, description: "", size_bytes: 4300 })).toBe("4 KB");
  });

  it("counts the header and the chips", () => {
    const list = [artifact("a"), artifact("b", { project_id: "attic", size_bytes: 2048 }), artifact("c")];
    expect(listMeta(list)).toBe("3 in 2 projects · 2 KB");
    const projects = [{ id: "attic" }, { id: "empty" }, { id: "checks", display_name: "Checks" }];
    expect(projectChips(list, projects)).toEqual([
      { id: "attic", name: "attic", count: 1 },
      { id: "checks", name: "Checks", count: 2 },
    ]);
  });
});

describe("the threads", () => {
  const text = (quote) => ({ mode: "text", quote });
  const comment = (id, minutesAgo, fields = {}) => ({
    id,
    body: id,
    author: "human",
    done: false,
    anchor: null,
    anchor_version: 3,
    created_at: at(minutesAgo),
    ...fields,
  });

  it("counts an open thread once, however many replies it holds", () => {
    const list = [
      comment("reply", 1, { anchor: text("the runner") }),
      comment("root", 5, { anchor: text("the runner") }),
      comment("loose", 2),
      comment("closed", 3, { done: true }),
    ];
    expect(openThreadCount(list)).toBe(2);
  });

  it("gathers later comments on the same passage under the first", () => {
    const list = [
      comment("reply", 1, { anchor: text("the runner") }),
      comment("loose", 2),
      comment("root", 5, { anchor: text("the runner") }),
    ];
    const threads = commentThreads(list);
    expect(threads.map((t) => [t.root.id, t.replies.map((r) => r.id)])).toEqual([
      ["loose", []],
      ["root", ["reply"]],
    ]);
  });

  it("keeps a passage on another version, or a resolved comment, a thread of its own", () => {
    const list = [
      comment("a", 5, { anchor: text("q") }),
      comment("b", 4, { anchor: text("q"), anchor_version: 2 }),
      comment("c", 3, { anchor: text("q"), done: true }),
    ];
    expect(commentThreads(list).map((t) => t.replies.length)).toEqual([0, 0, 0]);
  });
});
