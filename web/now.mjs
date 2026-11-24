// Now: one stream of what the reader's sessions did, in three tiers. What needs
// the reader, each ask a sentence with its answer at the end of the line; what
// changed since they looked, by project; and the quiet rest, folded. A written
// line opens it and a calm one closes it. Everything is read off the one Home
// response, so painting the screen is one request however much it shows.

import { api } from "./api.mjs";
import { bylineOf, foldSessions, liveCountText, participantsText } from "./author.mjs";
import { esc, glyph, kindMark, kindName, main, optionsAttr, paint, projectName, questionOptions, stale } from "./dom.mjs";
import { EMPTY_COPY, emptyStateHTML } from "./empty.mjs";
import { formatEventSummary } from "./feed.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { actOnIdea, answer, answerWith, approve, declineApproval, named } from "./inbox.mjs";
import { registerScreen } from "./keys.mjs";
import {
  askSentenceHTML,
  endingLine,
  feedSentenceHTML,
  needsYou,
  placeTime,
  readSnoozes,
  sinceAndQuiet,
  sinceOldest,
  snooze,
  summaryHTML,
  unsnooze,
} from "./now-model.mjs";
import { render } from "./router.mjs";
import { installShellLayout, shellHTML, shellStageHead } from "./shell-layout.mjs";
import { deadlineWords, stamp } from "./time.mjs";
import { toast } from "./toast.mjs";

// What the stream shows: everything, or one tier. The Inbox's address is Now
// showing what needs the reader, so an old link lands on the same asks.
const SHOWS = {
  "": "All",
  needs: "Needs you",
  since: "Since you looked",
};

function showFromHash() {
  const [path, query = ""] = location.hash.replace(/^#/, "").split("?");
  if (path.split("/")[1] === "inbox") return "needs";
  const show = new URLSearchParams(query).get("show") || "";
  return Object.hasOwn(SHOWS, show) ? show : "";
}

const hrefFor = (show) => (show ? `#/home?show=${show}` : "#/home");

// The quiet tier stays as the reader left it while they act on the asks above.
let quietOpen = false;

const clock = (size = 16) => glyphSvg("clock", { size, strokeWidth: 1.8 });

const shortId = (id) => String(id || "").slice(-6);

// A receipt reference: a mono id that is a link to what it names.
const ref = (href, id, label) =>
  `<a class="now-ref" href="${esc(href)}" aria-label="${esc(label)}">${esc(shortId(id))}</a>`;

const KIND_WORD = { approval: "Approval", question: "Question", idea: "Idea" };

// The ledger beside an ask: what it is and where, the event, the session it
// was asked in, the artifact it points at, and when it stops waiting.
function askReceipts(item, snoozedUntil) {
  const project = item.project_id;
  const lines = [
    `<span class="now-r now-r-kind">${esc(KIND_WORD[item.kind] || "Ask")} in ${esc(projectName(item))}</span>`,
  ];
  const by = bylineOf(item).receipts;
  if (by) lines.push(`<span class="now-r now-r-by">${esc(by)}</span>`);
  const where = [ref(`#/inbox?open=${encodeURIComponent(item.event_id)}`, item.event_id, `Event ${item.event_id}`)];
  if (item.session_id) {
    where.push(
      `in ${ref(
        `#/projects/${encodeURIComponent(project)}/sessions?id=${encodeURIComponent(item.session_id)}`,
        item.session_id,
        `Session ${item.session_id}`,
      )}`,
    );
  } else {
    where.push(`asked ${esc(placeTime(Date.parse(item.created_at)))}`);
  }
  lines.push(`<span class="now-r">${where.join(" ")}</span>`);
  const artifact = item.payload && typeof item.payload.artifact_id === "string" ? item.payload.artifact_id : "";
  if (artifact) {
    lines.push(
      `<span class="now-r">artifact ${ref(
        `#/artifacts/${encodeURIComponent(artifact)}?project=${encodeURIComponent(project)}`,
        artifact,
        `Artifact ${artifact}`,
      )}</span>`,
    );
  }
  const due = item.expires_at ? deadlineWords(item.expires_at, item.on_expiry) : "";
  if (due) lines.push(`<span class="now-r now-r-due">${clock(14)}${esc(due)}</span>`);
  if (snoozedUntil) {
    lines.push(`<span class="now-r now-r-due">${clock(14)}snoozed until ${esc(placeTime(snoozedUntil))}</span>`);
  }
  return lines.join("");
}

const kbd = (key) => `<kbd aria-hidden="true">${esc(key)}</kbd>`;

// The verbs at the end of an ask's line. The first is the primary, which only
// the focused row fills. Each button names the keys that press it.
function askActions(item, snoozedUntil) {
  const id = esc(item.event_id);
  const summary = esc(item.summary);
  const open = `<a class="now-sec now-read" href="#/inbox?open=${encodeURIComponent(item.event_id)}" data-open data-id="${id}">Open ${kbd("o")}</a>`;
  const button = (verb, label, keys, extra = "", primary = false) =>
    `<button type="button" class="${primary ? "now-pri" : ""}" data-now="${verb}" data-id="${id}" data-summary="${summary}" data-key="${keys}"${extra}>${esc(label)}${kbd(keys.split(" ")[0])}</button>`;
  const verbs = [];
  const foot = [];
  if (item.kind === "approval") {
    verbs.push(button("approve", "Approve", "1 a", "", true), button("decline", "Decline", "2"));
  } else if (item.kind === "question") {
    const options = questionOptions(item.payload).slice(0, 9);
    options.forEach((option, at) => {
      verbs.push(button("choose", option, String(at + 1), ` data-choice="${esc(option)}"`, at === 0));
    });
    const reply = `<button type="button" class="${options.length ? "now-sec" : "now-pri"}" data-now="reply" data-id="${id}" data-summary="${summary}" data-key="${options.length ? "r" : "1 r"}"${optionsAttr(item.payload)}>Reply${kbd(options.length ? "r" : "1")}</button>`;
    // With suggested answers Reply is a secondary, on the bottom line; with
    // none it is the one answer and stays first.
    (options.length ? foot : verbs).push(reply);
  } else if (item.kind === "idea") {
    verbs.push(button("resolve", "Resolve", "1 e", "", true), button("clear", "Clear", "2"));
  }
  // Snoozing has a key and a control: the key on a keyboard, and the button on
  // a row that is already snoozed, where bringing it back is the one thing to do.
  if (snoozedUntil) verbs.push(button("unsnooze", "Bring back", "s"));
  else verbs.push(`<button type="button" class="now-key-only" data-now="snooze" data-id="${id}" data-summary="${summary}" data-key="s" tabindex="-1" aria-hidden="true" hidden></button>`);
  foot.push(open);
  return { html: verbs.join(""), foot: foot.join("") };
}

function askRow(item, snoozes, later) {
  const until = snoozes[item.event_id] || 0;
  const acts = askActions(item, until);
  const mark = later
    ? `<span class="now-k now-k-later" aria-hidden="true">${clock()}</span><span class="sr-only">Can wait. ${esc(kindName(item.kind))}</span>`
    : `<span class="now-k">${glyph(item.kind)}</span>`;
  return `<div class="row now-row now-ask${later ? " is-later" : ""}" data-item="${esc(item.event_id)}">
    <div class="now-main">
      ${mark}
      <div class="now-body">
        <p class="now-s"><a class="now-ask-open" href="#/inbox?open=${encodeURIComponent(item.event_id)}" data-open data-id="${esc(item.event_id)}">${askSentenceHTML(item)}</a><span class="now-acts">${acts.html}</span><span class="now-foot">${acts.foot}</span></p>
      </div>
      ${stamp(item.created_at)}
    </div>
    <div class="now-mar now-rc">${askReceipts(item, until)}</div>
  </div>`;
}

const feedHref = (event) =>
  `#/projects/${encodeURIComponent(event.project_id)}/feed?event=${encodeURIComponent(event.id)}`;

// A row for something that happened: the session and what it did, in the words
// the feed uses, opening the event in its project's feed. The receipt is the
// event and, for a publish, the artifact.
export function eventRow(event, quiet = false) {
  const by = bylineOf(event).receipts;
  const receipts = [
    ...(by ? [`<span class="now-r now-r-by">${esc(by)}</span>`] : []),
    `<span class="now-r">${ref(feedHref(event), event.id, `Event ${event.id}`)}${event.inbox_status === "unread" ? " unread" : ""}</span>`,
  ];
  const artifact = event.payload && typeof event.payload.artifact_id === "string" ? event.payload.artifact_id : "";
  if (event.kind === "artifact" && artifact) {
    receipts.push(
      `<span class="now-r">artifact ${ref(
        `#/artifacts/${encodeURIComponent(artifact)}?project=${encodeURIComponent(event.project_id)}`,
        artifact,
        `Artifact ${artifact}`,
      )}</span>`,
    );
  }
  return `<div class="row now-row now-event${quiet ? " is-quiet" : ""}">
    <div class="now-main">
      <span class="now-k">${glyph(event.kind)}</span>
      <div class="now-body">
        <p class="now-s"><a class="now-open" href="${esc(feedHref(event))}" data-open data-id="${esc(event.id)}">${feedSentenceHTML(event, formatEventSummary(event))}</a></p>
      </div>
      ${stamp(event.created_at)}
    </div>
    <div class="now-mar now-rc">${receipts.join("")}</div>
  </div>`;
}

// A ledger line that is not a row: a tier's heading, a project's, the opening
// and the end. It still carries the margin, so the rule beside the stream runs
// unbroken.
const ledger = (cls, mainHTML, marHTML = "") =>
  `<div class="now-lr ${cls}"><div class="now-main">${mainHTML}</div><div class="now-mar">${marHTML}</div></div>`;

const tierGlyph = {
  need: kindMark("approval"),
  since: kindMark("signal"),
  quiet: glyphSvg("chevronDown", { size: 14, strokeWidth: 2 }),
};

function needsTier(asks, snoozes) {
  const count = asks.now.length + asks.unlisted;
  const sub = asks.later.length ? `<span class="now-sub">and ${asks.later.length} can wait</span>` : "";
  // The hub sends no id past the list, so the note opens the oldest listed ask,
  // and the unlisted ones follow it with j.
  const oldest = [...asks.now, ...asks.later].sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at))[0];
  const more = `${asks.unlisted} more ${asks.unlisted === 1 ? "ask is" : "asks are"} waiting and not listed here.`;
  const unlisted = asks.unlisted
    ? ledger(
        "now-note",
        `<p class="now-more">${oldest ? `<a href="#/inbox?open=${encodeURIComponent(oldest.event_id)}">${more}</a>` : more}</p>`,
      )
    : "";
  return `<section class="now-tier" aria-labelledby="now-needs">
    ${ledger("now-t is-need", `<h2 class="section-label now-label" id="now-needs">${tierGlyph.need}Needs you · <span class="now-n">${count}</span></h2>${sub}`)}
    ${asks.now.map((item) => askRow(item, snoozes, false)).join("")}
    ${asks.later.map((item) => askRow(item, snoozes, true)).join("")}
    ${unlisted}
  </section>`;
}

function sinceTier(since, count) {
  const projects = since.length;
  const groups = since
    .map((group) => {
      const more = group.more
        ? ledger(
            "now-note",
            `<a class="now-more" href="#/projects/${encodeURIComponent(group.id)}/feed">${group.more} more in ${esc(group.name)}</a>`,
          )
        : "";
      return `${ledger(
        "now-grp",
        `<h3 class="now-grp-name">${esc(group.name)} <span class="now-n">${group.rows.length + group.more}</span></h3>`,
      )}${group.rows.map((event) => eventRow(event)).join("")}${more}`;
    })
    .join("");
  return `<section class="now-tier" aria-labelledby="now-since">
    ${ledger(
      "now-t is-gap",
      `<h2 class="section-label now-label" id="now-since">${tierGlyph.since}Since you looked · <span class="now-n">${count}</span></h2><span class="now-sub is-where">in ${projects} ${projects === 1 ? "project" : "projects"}</span><button type="button" class="now-seen" data-now="seen" data-screen-key="E">Mark seen${kbd("E")}</button>`,
    )}
    ${groups}
  </section>`;
}

function quietTier(quiet) {
  return `<section class="now-tier" aria-labelledby="now-quiet-label">
    ${ledger(
      "now-t is-gap",
      `<h2 class="section-label now-label" id="now-quiet-label">${tierGlyph.quiet}Quiet · <span class="now-n">${quiet.length}</span></h2><span class="now-sub">seen before, folded</span><button type="button" class="now-show" data-now="quiet" data-screen-key="q" aria-expanded="${quietOpen}" aria-controls="now-quiet">${quietOpen ? "Hide" : "Show"}${kbd("q")}</button>`,
    )}
    <div class="now-quiet" id="now-quiet"${quietOpen ? "" : " hidden"}>${quiet.map((event) => eventRow(event, true)).join("")}</div>
  </section>`;
}

function chips(show, asks, sinceCount) {
  const counts = { "": "", needs: asks.now.length + asks.unlisted, since: sinceCount };
  return Object.entries(SHOWS)
    .map(
      ([key, label]) =>
        `<button type="button" class="chip" data-now="show" data-show="${key}" aria-pressed="${show === key}">${esc(label)}${
          key ? ` <span class="now-n">${counts[key]}</span>` : ""
        }</button>`,
    )
    .join("");
}

// The keys that move and open need a row to act on, so their hints go with the
// rows; the shortcut list opens from anywhere.
const HINTS = `<span class="now-hints" aria-hidden="true"><span class="now-hint-row">${kbd("j")}${kbd("k")} move</span><span class="now-hint-row">${kbd("o")} open</span><span>${kbd("?")} keys</span></span>`;

function headerMeta(oldest) {
  const date = new Date();
  const day = date.toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" });
  return [`${day}, ${placeTime(date.getTime())}`, oldest === null ? "" : `new since ${placeTime(oldest)}`]
    .filter(Boolean)
    .join(" · ");
}

const LIVE_SHOWN = 6;

// The sessions working now, across projects, children folded under their
// parent: the session in 600, then its project and who is in it as receipts.
function liveSessionsHTML(sessions) {
  const folded = foldSessions(sessions, { live: true });
  if (!folded.length) return "";
  const rows = folded.slice(0, LIVE_SHOWN).map(({ session, folded: more }) => {
    const project = session.project_display_name || session.project_id || "";
    const who = participantsText(session.participants);
    const meta = [project, who, more].filter(Boolean).join(" · ");
    const href = `#/projects/${encodeURIComponent(session.project_id)}/sessions?id=${encodeURIComponent(session.id)}`;
    return `<li class="now-live-row"><a class="now-live-name" href="${esc(href)}">${esc(session.session_name || session.id)}</a><span class="now-live-meta mono">${esc(meta)}</span></li>`;
  });
  const left = folded.length - rows.length;
  const rest = left > 0 ? `<li class="now-live-more">and ${left} more</li>` : "";
  return `<ul class="now-live" aria-label="Live sessions">${rows.join("")}${rest}</ul>`;
}

function introMargin(data, live) {
  const node = [data.node?.host, data.node?.mode].filter((part) => typeof part === "string" && part);
  // The head counts what the rows below it show. A hub that would not list its
  // sessions says so, in ink: an empty list and a failed one are not the same.
  const head = live
    ? `<p class="now-who">${esc(liveCountText(live))}</p>${liveSessionsHTML(live)}`
    : `<p class="now-who">Could not list live sessions.</p>`;
  return `<span class="section-label now-mar-label">Working now</span>
    ${head}
    ${node.length ? `<p class="now-node">${esc(node.join(" · "))}</p>` : ""}`;
}

export async function now(gen) {
  const show = showFromHash();
  const [data, live] = await Promise.all([
    api("/api/v1/home"),
    // The margin is a nicety: a hub that cannot list them leaves it to say so.
    api("/api/v1/sessions?live=1").then((answer) => answer.sessions || []).catch(() => null),
  ]);
  if (stale(gen)) return data;
  const at = Date.now();
  const snoozes = readSnoozes(at);
  const asks = needsYou(data, { now: at, snoozes });
  const { since, quiet, count: sinceCount } = sinceAndQuiet(data);
  const oldest = sinceOldest(since);
  const needs = asks.now.length + asks.unlisted;
  const anyAsk = needs + asks.later.length > 0;
  const sessions = Math.max(0, Math.trunc(Number(data.sessions_live)) || 0);
  const subagents = Math.max(0, Math.trunc(Number(data.subagents_live)) || 0);

  const summary = summaryHTML({ now: asks.now, later: asks.later.length, since: sinceCount, sessions, subagents, unlisted: asks.unlisted });
  const intro = ledger("now-intro", `<p class="now-sum">${summary}</p>`, introMargin(data, live));

  let tiers = "";
  if (show !== "since" && anyAsk) tiers += needsTier(asks, snoozes);
  if (show !== "needs" && since.length) tiers += sinceTier(since, sinceCount);
  if (!show && quiet.length) tiers += quietTier(quiet);

  let body;
  if (show === "needs" && !anyAsk) {
    body = intro + ledger("now-empty", emptyStateHTML(EMPTY_COPY.needs, {}, { href: "#/home" }));
  } else if (show === "since" && !since.length) {
    body = intro + ledger("now-empty", emptyStateHTML(EMPTY_COPY.since, {}, { href: "#/home" }));
  } else if (!tiers) {
    body = intro + ledger("now-empty", emptyStateHTML(EMPTY_COPY.now, {}, { action: "new-project" }));
  } else {
    const end = endingLine({ needs: show === "since" ? 0 : needs, oldest: show === "needs" ? null : oldest, now: at, scope: show });
    body = intro + tiers + ledger("now-end", `<p class="now-end-line"><span class="now-end-k" aria-hidden="true">${glyphSvg("resolve", { size: 16, strokeWidth: 2 })}</span>${esc(end)}</p>`);
  }

  paint(
    gen,
    shellHTML({
      noIndex: true,
      segment: "now",
      stageHead: shellStageHead("Now", headerMeta(oldest)),
      stageControls: `<div class="shell-controls now-controls" role="toolbar" aria-label="Show">${chips(show, asks, sinceCount)}${HINTS}</div>`,
      stageBody: `<div class="shell-pad now-pad"><div class="now">${body}</div></div>`,
    }),
  );
  installShellLayout(main);
  if (pendingFocus) {
    const focus = pendingFocus;
    pendingFocus = null;
    requestAnimationFrame(focus);
  }
  // The router hands this to the badge, which counts the same payload.
  return data;
}

// Where focus goes once the repaint an action caused has landed.
let pendingFocus = null;

const askRows = () => [...main.querySelectorAll(".now-ask")];

// After an ask is answered it leaves the list, so the row now at its place is
// the next one. Focus goes there, or to the last ask, or to the stream.
function focusAskAt(index) {
  const rows = askRows();
  const row = /** @type {HTMLElement} */ (rows[Math.min(index, rows.length - 1)]);
  if (row) row.focus({ preventScroll: false });
  else /** @type {HTMLElement} */ (main.querySelector(".now-row, .now-sum"))?.focus?.({ preventScroll: true });
}

const failed = (error) => toast(`Nothing changed: ${error.message}`);

async function markSeen(button) {
  button.disabled = true;
  try {
    await markEverythingSeen();
  } finally {
    button.disabled = false;
  }
  toast("Marked everything since you looked as seen.");
}

async function markEverythingSeen() {
  const data = await api("/api/v1/home");
  const { since } = sinceAndQuiet(data);
  const marks = [];
  for (const group of since) {
    let newest = group.rows[0]?.id || "";
    if (!newest) {
      const page = await api(`/api/v1/projects/${encodeURIComponent(group.id)}/feed?limit=1`);
      newest = page.events?.[0]?.id || "";
    }
    if (newest) {
      marks.push(
        api(`/api/v1/projects/${encodeURIComponent(group.id)}/feed/seen`, {
          method: "POST",
          body: JSON.stringify({ event_id: newest }),
        }),
      );
    }
  }
  marks.push(api("/api/v1/inbox/read-all", { method: "POST" }));
  await Promise.all(marks);
  await render();
}

main.addEventListener("click", (event) => {
  const button = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("button[data-now]"));
  if (!button || !main.querySelector('.shell[data-segment="now"]')) return;
  const { now: verb, id, summary } = button.dataset;
  const row = button.closest(".now-ask");
  const index = row ? askRows().indexOf(row) : -1;
  const next = () => {
    pendingFocus = () => focusAskAt(index);
  };
  let work = null;
  if (verb === "show") {
    const target = button.dataset.show || "";
    pendingFocus = () => /** @type {HTMLElement} */ (main.querySelector(`[data-now="show"][data-show="${target}"]`))?.focus({ preventScroll: true });
    location.hash = hrefFor(target);
    return;
  }
  if (verb === "quiet") {
    quietOpen = !quietOpen;
    const fold = /** @type {HTMLElement} */ (main.querySelector("#now-quiet"));
    if (fold) fold.hidden = !quietOpen;
    button.setAttribute("aria-expanded", String(quietOpen));
    button.firstChild.textContent = quietOpen ? "Hide" : "Show";
    return;
  }
  if (verb === "seen") work = markSeen(button);
  else if (verb === "approve") work = approve(id, summary).then(() => focusAskAt(index));
  else if (verb === "decline") work = declineApproval(id, summary).then(() => focusAskAt(index));
  else if (verb === "choose") {
    next();
    work = answerWith(id, button.dataset.choice).then(async (queued) => {
      await render();
      toast(queued ? "Queued, will send when online." : `Answered ${named(summary, id)} with \u201c${button.dataset.choice}\u201d, recorded on the feed.`);
    });
  } else if (verb === "reply") {
    work = answer(id, button, { onSent: () => focusAskAt(index) });
  } else if (verb === "resolve" || verb === "clear") {
    next();
    work = actOnIdea(id, verb === "clear" ? "cleared" : "resolved", summary);
  } else if (verb === "snooze") {
    snooze(id);
    pendingFocus = () => focusAskAt(index);
    work = render().then(() =>
      toast(`Snoozed ${named(summary, id)} for 1 hour, on this device.`, () => {
        unsnooze(id);
        render().catch(failed);
      }),
    );
  } else if (verb === "unsnooze") {
    unsnooze(id);
    pendingFocus = () => /** @type {HTMLElement} */ (main.querySelector(`.now-ask [data-id="${CSS.escape(id)}"]`)?.closest(".now-ask"))?.focus();
    work = render().then(() => toast(`Brought ${named(summary, id)} back.`));
  }
  if (work) work.catch(failed);
});

// The rows the selection moves through: every ask and event row the reader can
// see, so a folded quiet tier is skipped.
registerScreen("home", { rows: ".now-row:not(.now-quiet[hidden] .now-row)" });
registerScreen("inbox", { rows: ".now-row:not(.now-quiet[hidden] .now-row)" });
