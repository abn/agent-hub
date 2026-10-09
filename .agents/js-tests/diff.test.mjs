// The wiki's line diff, read back the way the history view uses it: the lines
// of the earlier version and of the page now, each said once and in order.

import { describe, expect, it } from "vitest";
import { diffStat, hunks, lineDiff, onlyFinalNewline, splitLines } from "../../web/diff.mjs";

// Reading a diff back as either side is the property that matters: dropping
// the added lines gives the earlier text, dropping the removed ones gives the
// later text. A diff that loses or invents a line fails one of the two.
const side = (lines, drop) =>
  lines
    .filter((line) => line.kind !== drop)
    .map((line) => line.text)
    .join("\n");

const holdsBothSides = (before, after) => {
  const lines = lineDiff(before, after);
  expect(side(lines, "add")).toBe(splitLines(before).join("\n"));
  expect(side(lines, "del")).toBe(splitLines(after).join("\n"));
  return lines;
};

describe("lineDiff", () => {
  it("marks nothing between two equal texts", () => {
    const lines = holdsBothSides("a\nb\nc\n", "a\nb\nc\n");
    expect(diffStat(lines)).toEqual({ added: 0, removed: 0 });
  });

  it("finds the one line an edit changed in the middle of a page", () => {
    const lines = holdsBothSides("# Deploy\n\nRun make.\n\nDone.\n", "# Deploy\n\nRun make release.\n\nDone.\n");
    expect(lines.filter((line) => line.kind !== "same")).toEqual([
      { kind: "del", text: "Run make." },
      { kind: "add", text: "Run make release." },
    ]);
  });

  it("reads a page from nothing as every line added, and back as every line removed", () => {
    expect(diffStat(holdsBothSides("", "a\nb\n"))).toEqual({ added: 2, removed: 0 });
    expect(diffStat(holdsBothSides("a\nb\n", ""))).toEqual({ added: 0, removed: 2 });
  });

  it("takes the shortest edit when lines move around", () => {
    const lines = holdsBothSides("a\nb\nc\nd\ne\n", "a\nc\nd\nb\ne\n");
    expect(diffStat(lines)).toEqual({ added: 1, removed: 1 });
  });

  it("holds both sides over many random edits", () => {
    let seed = 7;
    const random = () => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed / 2147483648;
    };
    const words = ["alpha", "beta", "gamma", "delta", ""];
    for (let round = 0; round < 200; round += 1) {
      const text = () =>
        Array.from({ length: Math.floor(random() * 12) }, () => words[Math.floor(random() * words.length)]).join("\n");
      holdsBothSides(text(), text());
    }
  });
});

describe("lineDiff past its edit budget", () => {
  it("gives no diff rather than a wrong one when two pages share nothing", () => {
    const before = Array.from({ length: 1500 }, (_, n) => `old ${n}`).join("\n");
    const after = Array.from({ length: 1500 }, (_, n) => `new ${n}`).join("\n");
    expect(lineDiff(before, after)).toBeNull();
  });

  it("still diffs a long page whose edit fits the budget", () => {
    const before = Array.from({ length: 5000 }, (_, n) => `line ${n}`).join("\n");
    const after = before.replace("line 2500", "line changed");
    expect(diffStat(holdsBothSides(before, after))).toEqual({ added: 1, removed: 1 });
  });

  // A middle that only adds or only removes is read off without a search, so
  // it is never past the budget; any other is given up on only past it.
  it("gives up only when the shortest edit is longer than the budget", () => {
    let seed = 11;
    const random = () => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed / 2147483648;
    };
    const words = ["alpha", "beta", "gamma", "delta"];
    let gaveUp = 0;
    for (let round = 0; round < 200; round += 1) {
      const text = () =>
        Array.from({ length: Math.floor(random() * 10) }, () => words[Math.floor(random() * words.length)]).join("\n");
      const before = text();
      const after = text();
      const exact = lineDiff(before, after);
      const { added, removed } = diffStat(exact);
      const budget = Math.floor(random() * 8);
      const bounded = lineDiff(before, after, budget);
      if (added + removed <= budget) expect(bounded).toEqual(exact);
      else if (bounded === null) gaveUp += 1;
      else expect(bounded).toEqual(exact);
    }
    expect(gaveUp).toBeGreaterThan(0);
  });
});

describe("hunks", () => {
  it("folds a long unchanged run to one count and keeps the context around a change", () => {
    const before = Array.from({ length: 20 }, (_, n) => `line ${n}`).join("\n");
    const after = before.replace("line 10", "line ten");
    const shown = hunks(lineDiff(before, after), 2);
    expect(shown[0]).toEqual({ kind: "skip", count: 8 });
    expect(shown.filter((line) => line.kind === "same").map((line) => line.text)).toEqual([
      "line 8",
      "line 9",
      "line 11",
      "line 12",
    ]);
    expect(shown[shown.length - 1]).toEqual({ kind: "skip", count: 7 });
  });

  it("folds the whole text when nothing changed", () => {
    expect(hunks(lineDiff("a\nb\n", "a\nb\n"))).toEqual([{ kind: "skip", count: 2 }]);
  });
});

describe("onlyFinalNewline", () => {
  it("tells a page that only gained or lost its final newline from an unchanged one", () => {
    expect(diffStat(lineDiff("a\nb", "a\nb\n"))).toEqual({ added: 0, removed: 0 });
    expect(onlyFinalNewline("a\nb", "a\nb\n")).toBe(true);
    expect(onlyFinalNewline("a\nb\n", "a\nb")).toBe(true);
  });

  it("is false for equal texts and for any other change", () => {
    expect(onlyFinalNewline("a\nb\n", "a\nb\n")).toBe(false);
    expect(onlyFinalNewline("a\nb\n", "a\nc\n")).toBe(false);
    // A second newline is a blank line, which the diff shows.
    expect(onlyFinalNewline("a\n", "a\n\n")).toBe(false);
    expect(diffStat(lineDiff("a\n", "a\n\n"))).toEqual({ added: 1, removed: 0 });
  });
});

// The length of the longest common subsequence of two line lists, by the plain
// table: the reference the edit script is held to.
const lcs = (a, b) => {
  const table = Array.from({ length: a.length + 1 }, () => new Array(b.length + 1).fill(0));
  for (let i = 1; i <= a.length; i += 1) {
    for (let j = 1; j <= b.length; j += 1) {
      table[i][j] = a[i - 1] === b[j - 1] ? table[i - 1][j - 1] + 1 : Math.max(table[i - 1][j], table[i][j - 1]);
    }
  }
  return table[a.length][b.length];
};

describe("lineDiff is a shortest edit", () => {
  it("adds and removes exactly the lines outside a longest common subsequence", () => {
    let seed = 23;
    const random = () => {
      seed = (seed * 1103515245 + 12345) % 2147483648;
      return seed / 2147483648;
    };
    const words = ["alpha", "beta", "gamma", "delta", ""];
    for (let round = 0; round < 300; round += 1) {
      const text = () =>
        Array.from({ length: Math.floor(random() * 14) }, () => words[Math.floor(random() * words.length)]).join("\n");
      const before = text();
      const after = text();
      const a = splitLines(before);
      const b = splitLines(after);
      const { added, removed } = diffStat(holdsBothSides(before, after));
      expect(added + removed).toBe(a.length + b.length - 2 * lcs(a, b));
    }
  });
});
