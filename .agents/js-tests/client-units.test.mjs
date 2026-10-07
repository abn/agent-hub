// Unit tests over the client's pure functions, imported from the modules the
// browser loads. Where a browser script proved a rule by walking the rendered
// page, the rule itself is held here, and a rename no longer breaks it.

import { describe, expect, it } from "vitest";
import { SUBJECT_MAX, formatEventSummary, subjectAndMessage } from "../../web/feed.mjs";
import { displayTitle, terms } from "../../web/search.mjs";
import { anchorQuote } from "../../web/comments.mjs";
import { optionsAttr, optionsFrom, questionOptions } from "../../web/dom.mjs";
import { composer } from "../../web/composer.mjs";

// The report a finished event carries, the one the browser invariant seeded to
// hold the subject and message split. Both stages read it: the inbox detail
// title and the feed stage title.
const LONG_SUMMARY =
  "The nightly run is green. 42 checks passed, the loader rewrite is behind the flag, " +
  "and the artifact viewer now reserves its 52px chrome like every other pane.";

// The split is lossless: the two halves are disjoint and together hold every
// character of the summary, so a summary that stops being a heading has not
// lost anything. The ellipsis is the reader's sign that the subject was cut, so
// it is dropped before the halves are read back together.
const readsBack = (summary) => {
  const { subject, message } = subjectAndMessage(summary);
  return `${subject.replace(/…$/, "")} ${message}`.replace(/\s+/g, " ").trim();
};

const flat = (summary) => summary.replace(/\s+/g, " ").trim();

describe("subjectAndMessage", () => {
  it("keeps a short single-line summary as a subject with no message", () => {
    expect(subjectAndMessage("Which storage backend should the hub use?")).toEqual({
      subject: "Which storage backend should the hub use?",
      message: "",
    });
  });

  it("keeps a summary at the budget as a subject", () => {
    const exact = "x".repeat(SUBJECT_MAX);
    expect(subjectAndMessage(exact)).toEqual({ subject: exact, message: "" });
  });

  it("splits a report at its leading sentence and marks the cut", () => {
    expect(subjectAndMessage(LONG_SUMMARY)).toEqual({
      subject: "The nightly run is green.…",
      message: flat(LONG_SUMMARY).slice("The nightly run is green.".length).trim(),
    });
  });

  it("reads a long summary back whole once the split has been made", () => {
    expect(readsBack(LONG_SUMMARY)).toBe(flat(LONG_SUMMARY));
    expect(subjectAndMessage(LONG_SUMMARY).subject.length).toBeLessThan(LONG_SUMMARY.length);
  });

  it("clamps a one-line summary with no sentence end inside the budget, on a word boundary", () => {
    // No `.`, `!` or `?` in the opening, so the subject is the opening words and
    // the message picks up at the same word. A word is never cut in half.
    const summary = "one two three four five six seven eight nine ten " +
      "eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty";
    const { subject, message } = subjectAndMessage(summary);
    // The cut sits on a whole word: the subject without its ellipsis is the
    // opening of the summary, and the next character is the space that word
    // ended on rather than the inside of a word.
    expect(subject.endsWith("…")).toBe(true);
    const head = subject.slice(0, -1);
    expect(summary.startsWith(head)).toBe(true);
    expect(summary.slice(head.length, head.length + 1)).toBe(" ");
    expect(message.startsWith(summary.slice(head.length).trim())).toBe(true);
    expect(readsBack(summary)).toBe(flat(summary));
  });

  it("stops a one-line summary at the last clause mark inside the budget, keeping the punctuation", () => {
    // A report with no sentence end inside the budget is titled by the clause it
    // last pauses at, so the heading ends on a phrase rather than inside one.
    // The mark is the reader's, so the subject keeps it.
    const summary =
      "The nightly run is green, the loader rewrite is behind the flag, " +
      "and the artifact viewer now reserves its 52px chrome like every other pane in the shell.";
    const { subject, message } = subjectAndMessage(summary);
    expect(subject).toBe("The nightly run is green, the loader rewrite is behind the flag,…");
    expect(message).toBe(
      "and the artifact viewer now reserves its 52px chrome like every other pane in the shell.",
    );
    expect(readsBack(summary)).toBe(flat(summary));
  });

  it("does not read a hyphen inside a word as a clause boundary", () => {
    // `build-1` is one word. Only a dash holding a space on each side is a
    // boundary, so a report with no other punctuation falls back to the last
    // whole word inside the budget and carries on past every hyphen.
    const summary =
      "Rebuilt the adapter so that build-1 and build-2 both hold one engine and the browser reads it through one reader without the two drifting apart over a release.";
    const { subject } = subjectAndMessage(summary);
    const lastHyphen = summary.lastIndexOf("-", SUBJECT_MAX);
    expect(subject.slice(0, -1)).toContain("build-2");
    expect(subject.length).toBeGreaterThan(lastHyphen);
    expect(readsBack(summary)).toBe(flat(summary));
  });

  it("takes the first line as the subject of a multi-line summary and keeps the rest as its message", () => {
    const summary = "Nightly run is green\n\n42 checks passed, the loader rewrite is behind the flag.";
    expect(subjectAndMessage(summary)).toEqual({
      subject: "Nightly run is green",
      message: "42 checks passed, the loader rewrite is behind the flag.",
    });
  });

  it("clamps a first line that is itself over the budget", () => {
    const first = "y".repeat(SUBJECT_MAX + 40);
    const { subject, message } = subjectAndMessage(`${first}\nsecond line`);
    expect(subject.length).toBe(SUBJECT_MAX + 1);
    expect(subject.endsWith("…")).toBe(true);
    expect(message).toBe("second line");
  });

  it("holds nothing for a missing or blank summary", () => {
    for (const empty of ["", "   ", "\n\n", null, undefined]) {
      expect(subjectAndMessage(empty)).toEqual({ subject: "", message: "" });
    }
  });

  it("trims a summary rather than splitting inside its padding", () => {
    expect(subjectAndMessage("  Padded subject  ")).toEqual({ subject: "Padded subject", message: "" });
  });
});

describe("formatEventSummary", () => {
  const summary = (kind, text) => formatEventSummary({ kind, summary: text });

  it("leaves a signal as the agent wrote it", () => {
    expect(summary("signal", "The tailnet link came back up")).toBe("The tailnet link came back up");
  });

  it("says a question was asked, whichever form it arrived in", () => {
    expect(summary("question", "Which backend?")).toBe("asked: Which backend?");
    expect(summary("question", "Asked: Which backend?")).toBe("asked: Which backend?");
  });

  it("says an answer was given, and drops the prefix it carried", () => {
    expect(summary("answer", "The WAL engine.")).toBe("answered The WAL engine.");
    expect(summary("answer", "re: The WAL engine.")).toBe("answered The WAL engine.");
    expect(summary("answer", "Answered  The WAL engine.")).toBe("answered The WAL engine.");
  });

  it("reads a session event as the change it is", () => {
    expect(summary("session", "session build-1 started")).toBe("started build-1");
    expect(summary("session", "session build-1 ended")).toBe("ended build-1");
    expect(summary("session", "session build-1 picked up from build-0")).toBe("picked up build-1 from build-0");
    expect(summary("session", "session build-1 forked from build-0")).toBe("forked build-1 from build-0");
    expect(summary("session", "session build-1 reassigned to build-2")).toBe("reassigned build-1 to build-2");
  });

  it("lowercases a leading capitalised verb and leaves the rest of the line", () => {
    expect(summary("session", "Resumed build-1 after a compaction")).toBe("resumed build-1 after a compaction");
    expect(summary("artifact", "Published notes.md")).toBe("published notes.md");
    expect(summary("finished", "Patched the search index")).toBe("patched the search index");
    // The pattern wants a capitalised verb ending in "ed", so an irregular one
    // is left as the agent wrote it rather than guessed at.
    expect(summary("finished", "Wrote the runbook")).toBe("Wrote the runbook");
  });

  it("says an approval was decided, and reads a re: line as a decision", () => {
    expect(summary("approval", "re: Ship the WAL engine.")).toBe("decided Ship the WAL engine.");
    expect(summary("approval", "Approved the WAL engine.")).toBe("approved the WAL engine.");
  });

  it("leaves a line it has no reading for alone", () => {
    expect(summary("session", "nothing conventional here")).toBe("nothing conventional here");
    expect(summary("comment", "Asked: is this a question?")).toBe("Asked: is this a question?");
    expect(formatEventSummary({ kind: "signal" })).toBe("");
  });
});

describe("displayTitle", () => {
  it("drops the knowledge base namespace from a wiki page's display path", () => {
    // The index answers with the stored path; the wiki index beside it reads the
    // same page as `notes.md`, so the search row reads it the same way.
    expect(displayTitle("/fs/notes.md", "kb")).toBe("notes.md");
    expect(displayTitle("/fs/deploy/rollout.md", "kb")).toBe("deploy/rollout.md");
    expect(displayTitle("/index.md", "kb")).toBe("index.md");
  });

  it("leaves a title that is not a wiki path alone, whatever the family", () => {
    expect(displayTitle("runbook.md", "kb")).toBe("runbook.md");
    expect(displayTitle("/fs/notes.md", "feed")).toBe("/fs/notes.md");
    expect(displayTitle("/fs/notes.md", "artifact")).toBe("/fs/notes.md");
  });

  it("reads a missing title as nothing rather than as the word undefined", () => {
    for (const empty of ["", null, undefined, 0]) {
      expect(displayTitle(empty, "kb")).toBe("");
    }
  });
});

describe("terms", () => {
  it("is the words of a query as the index matched them", () => {
    expect(terms("WAL engine?")).toEqual(["wal", "engine"]);
    expect(terms("  ")).toEqual([]);
    expect(terms(null)).toEqual([]);
  });
});

describe("anchorQuote", () => {
  // The hub caps a text anchor at 2048 bytes, so a long selection must be cut
  // client-side rather than refused.
  const bytes = (text) => new TextEncoder().encode(text).length;

  it("keeps a short quote as written", () => {
    expect(anchorQuote("Build")).toBe("Build");
  });

  it("cuts a long quote below the hub's byte budget", () => {
    const capped = anchorQuote("x".repeat(5000));
    expect(capped.length).toBeGreaterThan(0);
    expect(bytes(capped)).toBeLessThanOrEqual(1500);
  });

  it("cuts multibyte text by bytes, not characters", () => {
    const capped = anchorQuote("é".repeat(2000));
    expect(bytes(capped)).toBeLessThanOrEqual(1500);
    expect(capped.length).toBeLessThan(2000);
  });
});

describe("questionOptions", () => {
  it("reads the options a question carries, in order", () => {
    expect(questionOptions({ options: ["Keep it", "Drop it"] })).toEqual(["Keep it", "Drop it"]);
  });

  it("offers nothing for a question without options or with a malformed list", () => {
    expect(questionOptions(null)).toEqual([]);
    expect(questionOptions({ body: "plain" })).toEqual([]);
    expect(questionOptions({ options: "Yes" })).toEqual([]);
    expect(questionOptions({ options: [1, null, "  ", "Yes"] })).toEqual(["Yes"]);
  });

  it("carries the options through a Reply control's attribute and back", () => {
    const options = ['Say "yes"', "<b>no</b>"];
    const holder = document.createElement("div");
    holder.innerHTML = `<button${optionsAttr({ options })}>Reply</button>`;
    expect(optionsFrom(holder.querySelector("button").dataset.options)).toEqual(options);
    expect(optionsAttr({})).toBe("");
    expect(optionsFrom("not json")).toEqual([]);
  });
});

describe("composer quick answers", () => {
  const picks = (element) => [...element.querySelectorAll(".composer-options button")];

  it("draws no quick answers when none are offered", () => {
    const { element } = composer({ label: "Your answer", send: async () => {} });
    expect(element.querySelector(".composer-options")).toBeNull();
  });

  it("sends a picked option's text once, however often it is pressed", async () => {
    const sent = [];
    let finish;
    const { element } = composer({
      label: "Your answer",
      options: ["Keep it", "Drop it"],
      send: (body) => {
        sent.push(body);
        return new Promise((resolve) => (finish = resolve));
      },
    });
    const [keep, drop] = picks(element);
    expect(element.querySelector('[role="group"]').getAttribute("aria-label")).toBe("Quick answers");
    drop.click();
    drop.click();
    keep.click();
    element.querySelector(".composer-field").value = "typed instead";
    element.dispatchEvent(new Event("submit", { cancelable: true }));
    expect(sent).toEqual(["Drop it"]);
    expect(picks(element).every((pick) => pick.disabled)).toBe(true);
    finish();
  });

  it("says why a refused pick sent nothing and offers the options again", async () => {
    const { element } = composer({
      label: "Your answer",
      options: ["Keep it", "Drop it"],
      send: async () => {
        throw new Error("question was already answered");
      },
    });
    const [keep] = picks(element);
    keep.click();
    await new Promise((resolve) => setTimeout(resolve, 0));
    const error = element.querySelector(".composer-error");
    expect(error.hidden).toBe(false);
    expect(error.textContent).toBe("Nothing was sent: question was already answered");
    expect(picks(element).some((pick) => pick.disabled)).toBe(false);
  });
});
