// Artifacts: every project's artifacts in one list, in two tiers. What moved
// since the reader looked, then everything earlier, newest first, with the
// older rest folded. Each row is the stream's own row: glyph, name, the line
// under it, and the time in mono. On a wide stage the margin previews the
// focused row: its state, its comments, the event that published it and its
// versions, with the way in. The list is read off each project's own artifact
// list, so it needs nothing the hub does not already serve.

import { api } from "./api.mjs";
import { bylineOf } from "./author.mjs";
import {
  artifactLine,
  artifactSlug,
  artifactTiers,
  commentSummary,
  formatBytes,
  listMeta,
  projectChips,
  readSeen,
  seeAll,
  versionSummary,
} from "./artifact-model.mjs";
import { artifactHref } from "./artifacts.mjs";
import { esc, kindMark, main, paint, stale } from "./dom.mjs";
import { EMPTY_COPY, emptyStateHTML } from "./empty.mjs";
import { onArtifactLive } from "./events.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { registerScreen } from "./keys.mjs";
import { render } from "./router.mjs";
import { installShellLayout, shellHTML, shellStageHead } from "./shell-layout.mjs";
import { placeTime, stamp } from "./time.mjs";
import { toast } from "./toast.mjs";
import { artifactsScope, loadWho, whoMenu, whoParams } from "./who.mjs";

// How many earlier rows show before the rest folds.
const EARLIER_SHOWN = 10;

// The fold stays as the reader left it while they move about the list.
let showAll = false;

// The artifacts the last paint listed, by id, for the preview.
let listed = new Map();

// The projects and their merged artifacts, read once per visit to the screen.
// A chip, Show all or Mark seen repaints from this; a fresh entry to the
// screen or a live artifact write reads them again.
let loaded = null;

const kbd = (key) => `<kbd aria-hidden="true">${esc(key)}</kbd>`;

function projectFromHash() {
  const query = location.hash.replace(/^#/, "").split("?")[1] || "";
  return new URLSearchParams(query).get("project") || "";
}

const hrefFor = (project) => (project ? `#/artifacts?project=${encodeURIComponent(project)}` : "#/artifacts");

const lockMark = () =>
  `<span class="glyph" aria-hidden="true">${glyphSvg("lock", { size: 14, strokeWidth: 2.4 })}</span>`;

function row(artifact) {
  const slug = artifactSlug(artifact.title, artifact.kind);
  const mark = artifact.protected
    ? `${lockMark()}<span class="sr-only">Protected</span>`
    : `${kindMark("artifact")}<span class="sr-only">Artifact</span>`;
  return `<div class="row now-row arts-row" data-artifact="${esc(artifact.id)}">
    <div class="now-main">
      <span class="now-k">${mark}</span>
      <div class="now-body">
        <p class="now-s arts-name"><a class="now-open" href="${esc(artifactHref(artifact.project_id, artifact.id))}" data-open data-id="${esc(artifact.id)}">${esc(slug)}</a></p>
        <p class="arts-line">${esc(artifactLine(artifact))}</p>
      </div>
      ${stamp(artifact.updated_at || artifact.created_at)}
    </div>
  </div>`;
}

const tierLine = (cls, id, label, count, extra = "") =>
  `<div class="now-lr now-t ${cls}"><div class="now-main"><h2 class="section-label now-label" id="${id}">${label} · <span class="now-n">${count}</span></h2>${extra}</div></div>`;

function sinceTier(rows) {
  return `<section class="arts-tier" aria-labelledby="arts-since">
    ${tierLine(
      "",
      "arts-since",
      `${kindMark("signal")}Since you looked`,
      rows.length,
      `<button type="button" data-arts="seen" data-screen-key="E">Mark seen${kbd("E")}</button>`,
    )}
    ${rows.map(row).join("")}
  </section>`;
}

function earlierTier(tiers, gap) {
  const back = tiers.backTo ? `, back to ${esc(placeTime(tiers.backTo))}` : "";
  const fold = tiers.more
    ? `<div class="arts-fold"><span>${tiers.more} more${back}</span><button type="button" data-arts="all">Show all</button></div>`
    : "";
  return `<section class="arts-tier" aria-labelledby="arts-earlier">
    ${tierLine(
      gap ? "is-gap" : "",
      "arts-earlier",
      `<span class="glyph" aria-hidden="true">${glyphSvg("stack", { size: 14, strokeWidth: 2.2 })}</span>Earlier`,
      tiers.total,
      `<span class="now-sub">newest first</span>`,
    )}
    ${tiers.earlier.map(row).join("")}
    ${fold}
  </section>`;
}

// Under 720px the row is a scrolling strip of every project. Wider, a hub with
// five or more projects keeps the three with the most artifacts as chips and
// folds the rest into a More menu; a folded project that is the current filter
// stays on the row as a chip so the filter is always visible. The three is a
// ceiling: fitChips folds more of them into More as the row narrows.
const CHIPS_SHOWN = 3;
const MORE_CHEVRON = `<svg aria-hidden="true" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M 6 9l6 6 6-6"></path></svg>`;

function chips(active, choices) {
  const chip = (id, label, count, extra = "", attrs = "") =>
    `<button type="button" class="chip${extra}" data-arts="filter" data-project="${esc(id)}"${attrs} aria-pressed="${active === id}"><span class="chip-label" title="${esc(label)}">${esc(label)}</span>${
      count === null ? "" : ` <span class="now-n">${count}</span>`
    }</button>`;
  if (choices.length < CHIPS_SHOWN + 2) return [chip("", "All", null), ...choices.map((c) => chip(c.id, c.name, c.count))].join("");
  const ranked = choices.map((c, i) => ({ c, i })).sort((a, b) => b.c.count - a.c.count || a.i - b.i).map(({ c }) => c);
  const top = ranked.slice(0, CHIPS_SHOWN);
  const rest = ranked.slice(CHIPS_SHOWN);
  const rank = (c) => ` data-rank="${ranked.indexOf(c)}"`;
  const item = (c) =>
    `<button type="button" role="menuitemradio" aria-checked="${active === c.id}" data-arts="filter" data-project="${esc(c.id)}"${rank(c)}${ranked.indexOf(c) < CHIPS_SHOWN ? " hidden" : ""}><span class="arts-check" aria-hidden="true">${glyphSvg("resolve", { size: 14, strokeWidth: 2.2 })}</span><span class="chip-label" title="${esc(c.name)}">${esc(c.name)}</span> <span class="mono shell-menu-n">${c.count}</span></button>`;
  return [
    chip("", "All", null),
    ...top.map((c) => chip(c.id, c.name, c.count, "", rank(c))),
    ...rest.map((c) => chip(c.id, c.name, c.count, c.id === active ? " arts-chip-folded is-current" : " arts-chip-folded", rank(c))),
    `<div class="shell-group arts-more">
      <button type="button" class="hub-group-toggle" data-group-toggle data-arts-more aria-haspopup="menu" aria-expanded="false">More${MORE_CHEVRON}</button>
      <div class="shell-group-menu" role="menu" aria-label="More projects" data-group-menu hidden>${ranked.map(item).join("")}</div>
    </div>`,
  ].join("");
}

const HINTS = `<span class="now-hints arts-hints" aria-hidden="true"><span class="now-hint-row">${kbd("j")}${kbd("k")} move</span><span class="now-hint-row">${kbd("o")} open</span></span>`;

async function everyArtifact(projects) {
  const who = whoParams(artifactsScope());
  const lists = await Promise.all(
    projects.map((project) =>
      api(`/api/v1/projects/${encodeURIComponent(project.id)}/artifacts${who ? `?${who}` : ""}`)
        .then((answer) =>
          (answer.artifacts || []).map((a) => ({
            ...a,
            project_id: a.project_id || project.id,
            project_name: project.display_name || project.id,
          })),
        )
        .catch(() => []),
    ),
  );
  return lists.flat();
}

function forget() {
  loaded = null;
  eventCache.clear();
}

// The Who choice the held lists were read under: a different one is a different
// read, so the lists are asked for again.
let loadedFor = "";

async function load() {
  const asking = whoParams(artifactsScope());
  if (loaded && loadedFor !== asking) loaded = null;
  if (!loaded) {
    loadedFor = asking;
    loaded = api("/api/v1/projects")
      .then(async ({ projects }) => ({ projects: projects || [], all: await everyArtifact(projects || []) }))
      .catch((error) => {
        loaded = null;
        throw error;
      });
  }
  return loaded;
}

export async function artifactListScreen(gen) {
  if (!onList()) forget();
  const active = projectFromHash();
  const { projects, all } = await load();
  const who = await loadWho();
  if (stale(gen)) return;
  const state = readSeen() || seeAll(all);
  const shown = active ? all.filter((a) => a.project_id === active) : all;
  const tiers = artifactTiers(shown, state, { limit: EARLIER_SHOWN, all: showAll });
  listed = new Map(shown.map((a) => [a.id, a]));

  let list;
  if (!shown.length) {
    list = emptyStateHTML(EMPTY_COPY.artifacts);
  } else {
    list =
      (tiers.since.length ? sinceTier(tiers.since) : "") +
      (tiers.total ? earlierTier(tiers, tiers.since.length > 0) : "");
  }
  const first = tiers.since[0] || tiers.earlier[0] || null;

  paint(
    gen,
    shellHTML({
      noIndex: true,
      segment: "artifact-list",
      stageHead: shellStageHead("Artifacts", all.length ? listMeta(all) : "", "", "#/more"),
      stageControls: `<div class="shell-controls arts-controls" role="toolbar" aria-label="Filter artifacts">${chips(
        active,
        projectChips(all, projects),
      )}${whoMenu(artifactsScope(), who)}${HINTS}</div>`,
      stageBody: `<div class="shell-pad now-pad arts-pad"><div class="arts"><div class="arts-split">
        <div class="arts-list">${list}</div>
        <aside class="arts-mar" aria-label="Focused artifact">${first ? preview(first) : ""}</aside>
      </div></div></div>`,
    }),
  );
  installShellLayout(main);
  keepMoreInView();
  focusFirstShown();
  watchChipFit();
  if (first) fillPreview(first);
  if (focusAfter !== null) {
    const chosen = [...main.querySelectorAll("button.chip[data-arts='filter']")].find((b) => /** @type {HTMLElement} */ (b).dataset.project === focusAfter);
    /** @type {HTMLElement | undefined} */ (chosen)?.focus({ preventScroll: true });
    focusAfter = null;
  }
}

// The project a pick just chose; its chip takes focus once the list is redrawn.
/** @type {string | null} */
let focusAfter = null;

// How many of the busiest chips fit beside More and the key hints: as many as
// the row holds up to the ceiling, and never fewer than one, so a long name is
// folded into More before it is cut to a letter. The menu lists every project
// that is not a chip on the row; the current filter stays a chip either way.
/** @type {ResizeObserver | null} */
let chipWatch = null;
/** @type {MutationObserver | null} */
let chipGuard = null;

function stopChipWatch() {
  chipWatch?.disconnect();
  chipGuard?.disconnect();
  chipWatch = null;
  chipGuard = null;
}

/** @param {HTMLElement} bar */
function fitChips(bar) {
  if (!bar.querySelector(".arts-more")) return;
  const chips = [...bar.querySelectorAll("button.chip[data-rank]")].map((el) => /** @type {HTMLElement} */ (el));
  const items = [...bar.querySelectorAll(".arts-more [data-rank]")].map((el) => /** @type {HTMLElement} */ (el));
  const apply = (/** @type {number} */ n) => {
    for (const chip of chips) {
      const folded = Number(chip.dataset.rank) >= n;
      const current = chip.getAttribute("aria-pressed") === "true";
      chip.classList.toggle("arts-chip-folded", folded);
      chip.classList.toggle("is-current", folded && current);
    }
    for (const item of items) item.hidden = Number(item.dataset.rank) < n;
  };
  // A chip is cut when its text is wider than its label's box, unless the box
  // is already at the longest name a chip shows.
  const cut = (/** @type {HTMLElement} */ chip) => {
    const label = chip.querySelector(".chip-label");
    if (!label || chip.offsetParent === null) return false;
    const range = document.createRange();
    range.selectNodeContents(label);
    const box = label.getBoundingClientRect().width;
    const text = range.getBoundingClientRect?.().width ?? 0;
    return text > box + 0.1 && box < parseFloat(getComputedStyle(label).maxWidth) - 0.5;
  };
  const wide = window.matchMedia?.("(min-width: 720px)").matches ?? true;
  let n = wide ? CHIPS_SHOWN : chips.length;
  for (; wide && n > 1; n--) {
    apply(n);
    if (bar.scrollWidth <= bar.clientWidth && !chips.some(cut)) break;
  }
  apply(n);
}

// The fit follows the toolbar it was started for and stops with it: another
// screen replacing the stage takes the toolbar out, and the guard lets go.
function watchChipFit() {
  stopChipWatch();
  const bar = /** @type {HTMLElement | null} */ (main.querySelector(".arts-controls"));
  if (!bar) return;
  fitChips(bar);
  // Plex arrives after the first measure and is wider than the fallback face.
  document.fonts?.ready.then(() => bar.isConnected && fitChips(bar));
  if (typeof ResizeObserver !== "function") return;
  chipWatch = new ResizeObserver(() => (bar.isConnected ? fitChips(bar) : stopChipWatch()));
  chipWatch.observe(bar);
  if (typeof MutationObserver === "function") {
    chipGuard = new MutationObserver(() => {
      if (!bar.isConnected) stopChipWatch();
    });
    chipGuard.observe(main, { childList: true, subtree: true });
  }
}

// The menu opens on its first item; the fit may have hidden the busiest
// projects, so focus moves on to the first one the menu still shows. The
// menu's own toggle handler stops the click, so this sits on the toggle too.
function focusFirstShown() {
  const toggle = main.querySelector("[data-arts-more]");
  toggle?.addEventListener("click", () => {
    const menu = /** @type {HTMLElement | null} */ (toggle.closest(".arts-more")?.querySelector("[data-group-menu]"));
    if (menu && !menu.hidden) /** @type {HTMLElement | null} */ (menu.querySelector("button:not([hidden])"))?.focus();
  });
}

// The Group menu places itself 200px from the right edge; More's items can be
// wider than that, so once it opens the menu is pulled back inside the window.
function keepMoreInView() {
  const toggle = main.querySelector("[data-arts-more]");
  const menu = /** @type {HTMLElement | null} */ (main.querySelector(".arts-more [data-group-menu]"));
  toggle?.addEventListener("click", () => {
    if (!menu || menu.hidden) return;
    menu.style.maxWidth = `${window.innerWidth - 16}px`;
    const left = Math.min(parseFloat(menu.style.left) || 8, window.innerWidth - menu.offsetWidth - 8);
    menu.style.left = `${Math.max(8, Math.round(left))}px`;
  });
}

// The receipts of the focused artifact, drawn at once from the list and then
// completed by the two reads only the margin needs: its versions and the event
// that published it.
function preview(artifact) {
  const slug = artifactSlug(artifact.title, artifact.kind);
  const path = `${artifact.project_id} / artifacts / ${slug}`;
  const status = artifact.live_version
    ? `<span class="arts-status">${kindMark("signal")}Being written</span>`
    : artifact.protected
      ? `<span class="arts-status">${lockMark()}Encrypted</span>`
      : `<span class="arts-status is-ok">${glyphSvg("resolve", { size: 14, strokeWidth: 2.2 })}Published</span>`;
  const by = bylineOf(artifact);
  const who = [artifact.project_name || artifact.project_id, by.subject, by.receipts, formatBytes(artifact.size_bytes)].filter(Boolean).join(" · ");
  return `<div class="arts-prev" data-for="${esc(artifact.id)}">
    <span class="section-label arts-prev-label">Focused</span>
    <h3 class="arts-prev-name">${esc(slug)}</h3>
    <p class="arts-prev-who">${esc(who)}</p>
    <dl class="arts-rc">
      <dt>Status</dt><dd>${status}</dd>
      <dt>Comments</dt><dd>${esc(commentSummary(artifact.comments_count, artifact.comments_open))}</dd>
      <dt>Event</dt><dd class="arts-event"><span class="arts-wait">…</span></dd>
      <dt>Path</dt><dd class="arts-path"><span class="arts-path-text mono">${esc(path)}</span><button type="button" class="hub-btn-glyph arts-copy" data-arts="copy" data-path="${esc(path)}" aria-label="Copy path ${esc(slug)}">${glyphSvg("copy", { size: 14 })}</button></dd>
    </dl>
    <span class="section-label arts-prev-label">Versions</span>
    <ul class="arts-vers"><li class="arts-wait">…</li></ul>
    <div class="arts-acts"><a class="arts-open" href="${esc(artifactHref(artifact.project_id, artifact.id))}">Open ${kbd("o")}</a></div>
  </div>`;
}

const versionCache = new Map();
const eventCache = new Map();

function versionsOf(artifact) {
  const key = `${artifact.id}@${artifact.version}`;
  if (!versionCache.has(key)) {
    versionCache.set(
      key,
      api(`/api/v1/artifacts/${encodeURIComponent(artifact.id)}/versions`)
        .then((answer) => answer.versions || [])
        .catch(() => null),
    );
  }
  return versionCache.get(key);
}

// The newest artifact event in the project that names this artifact. One read
// per project serves every artifact in it.
function eventsOf(project) {
  if (!eventCache.has(project)) {
    eventCache.set(
      project,
      api(`/api/v1/projects/${encodeURIComponent(project)}/feed?kinds=artifact&limit=200`)
        .then((answer) => answer.events || [])
        .catch(() => []),
    );
  }
  return eventCache.get(project);
}

function marginShown(aside) {
  return !!aside && aside.getClientRects().length > 0 && getComputedStyle(aside).display !== "none";
}

async function fillPreview(artifact) {
  const aside = main.querySelector(".arts-mar");
  if (!marginShown(aside)) return;
  const box = () => {
    const prev = /** @type {HTMLElement} */ (aside.querySelector(".arts-prev"));
    return aside.isConnected && prev && prev.dataset.for === artifact.id ? prev : null;
  };
  const [versions, events] = await Promise.all([versionsOf(artifact), eventsOf(artifact.project_id)]);
  const prev = box();
  if (!prev) return;
  const event = events.find((e) => e.payload && e.payload.artifact_id === artifact.id);
  prev.querySelector(".arts-event").innerHTML = event
    ? `<a class="now-ref" href="#/projects/${encodeURIComponent(artifact.project_id)}/feed?event=${encodeURIComponent(event.id)}" aria-label="Event ${esc(event.id)}">${esc(String(event.id).slice(-6))}</a>`
    : `<span class="arts-none">Not in the feed</span>`;
  const list = prev.querySelector(".arts-vers");
  if (!versions) {
    list.innerHTML = `<li class="arts-none">Versions could not be read.</li>`;
    return;
  }
  const newestFirst = [...versions].reverse();
  const head = newestFirst.slice(0, 5);
  const rest = newestFirst.length - head.length;
  list.innerHTML =
    head
      .map(
        (v) =>
          `<li><span class="arts-v">v${v.version}</span><span class="arts-vs">${esc(versionSummary(v))}</span><span class="arts-vw">${esc(placeTime(Date.parse(v.created_at)))}</span></li>`,
      )
      .join("") + (rest ? `<li class="arts-none">and ${rest} earlier</li>` : "");
}

function focusRow(row) {
  const artifact = listed.get(row.dataset.artifact);
  const aside = main.querySelector(".arts-mar");
  if (!artifact || !aside) return;
  if (/** @type {HTMLElement} */ (aside.querySelector(".arts-prev"))?.dataset.for === artifact.id) return;
  aside.innerHTML = preview(artifact);
  fillPreview(artifact);
}

const onList = () => !!main.querySelector('.shell[data-segment="artifact-list"]');

onArtifactLive(forget);

main.addEventListener("focusin", (event) => {
  if (!onList()) return;
  const row = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest?.(".arts-row"));
  if (row) focusRow(row);
});

// The More menu's keys: ArrowDown on its trigger opens it, the arrows move
// through the projects, and Escape closes it with focus back on the trigger.
main.addEventListener("keydown", (event) => {
  if (!onList()) return;
  const target = /** @type {HTMLElement} */ (event.target);
  const more = target.closest?.(".arts-more");
  if (!more) return;
  const toggle = /** @type {HTMLElement} */ (more.querySelector("[data-arts-more]"));
  const menu = /** @type {HTMLElement} */ (more.querySelector("[data-group-menu]"));
  if (event.key === "ArrowDown" && target === toggle && menu.hidden) {
    event.preventDefault();
    toggle.click();
  } else if (event.key === "Escape" && !menu.hidden) {
    toggle.focus();
  } else if ((event.key === "ArrowDown" || event.key === "ArrowUp") && menu.contains(target)) {
    event.preventDefault();
    const items = /** @type {HTMLElement[]} */ ([...menu.querySelectorAll("button:not([hidden])")]);
    const at = items.indexOf(/** @type {HTMLButtonElement} */ (target));
    const step = event.key === "ArrowDown" ? 1 : -1;
    items[(at + step + items.length) % items.length].focus();
  }
});

main.addEventListener("click", async (event) => {
  const button = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("button[data-arts]"));
  if (!button || !onList()) return;
  const verb = button.dataset.arts;
  if (verb === "filter") {
    const id = button.dataset.project || "";
    if (id === projectFromHash()) {
      // Picking what is already chosen changes nothing; focus goes back to More.
      if (button.closest(".arts-more")) /** @type {HTMLElement | null} */ (main.querySelector("[data-arts-more]"))?.focus();
      return;
    }
    focusAfter = id;
    location.hash = hrefFor(id);
  } else if (verb === "all") {
    showAll = true;
    await render();
    // The first row the fold held, so the reader carries on where it opened.
    /** @type {HTMLElement} */ (main.querySelectorAll('[aria-labelledby="arts-earlier"] .arts-row')[EARLIER_SHOWN])?.focus({ preventScroll: true });
  } else if (verb === "seen") {
    seeAll([...listed.values()]);
    await render();
    toast(projectFromHash() ? "Marked these artifacts as seen." : "Marked every artifact as seen.");
  } else if (verb === "copy") {
    try {
      await navigator.clipboard.writeText(button.dataset.path);
    } catch {}
    toast("Path copied");
  }
});

registerScreen("artifacts", { rows: ".arts-row" });
