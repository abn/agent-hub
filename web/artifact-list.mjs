// Artifacts: every project's artifacts in one list, in two tiers. What moved
// since the reader looked, then everything earlier, newest first, with the
// older rest folded. Each row is the stream's own row: glyph, name, the line
// under it, and the time in mono. On a wide stage the margin previews the
// focused row: its state, its comments, the event that published it and its
// versions, with the way in. The list is read off each project's own artifact
// list, so it needs nothing the hub does not already serve.

import { api } from "./api.mjs";
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

function chips(active, choices) {
  const chip = (id, label, count) =>
    `<button type="button" class="chip" data-arts="filter" data-project="${esc(id)}" aria-pressed="${active === id}">${esc(label)}${
      count === null ? "" : ` <span class="now-n">${count}</span>`
    }</button>`;
  return [chip("", "All", null), ...choices.map((c) => chip(c.id, c.name, c.count))].join("");
}

const HINTS = `<span class="now-hints arts-hints" aria-hidden="true"><span class="now-hint-row">${kbd("j")}${kbd("k")} move</span><span class="now-hint-row">${kbd("o")} open</span></span>`;

async function everyArtifact(projects) {
  const lists = await Promise.all(
    projects.map((project) =>
      api(`/api/v1/projects/${encodeURIComponent(project.id)}/artifacts`)
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

async function load() {
  if (!loaded) {
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
      )}${HINTS}</div>`,
      stageBody: `<div class="shell-pad now-pad arts-pad"><div class="arts"><div class="arts-split">
        <div class="arts-list">${list}</div>
        <aside class="arts-mar" aria-label="Focused artifact">${first ? preview(first) : ""}</aside>
      </div></div></div>`,
    }),
  );
  installShellLayout(main);
  if (first) fillPreview(first);
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
  const who = [artifact.project_name || artifact.project_id, artifact.actor, formatBytes(artifact.size_bytes)].filter(Boolean).join(" · ");
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

main.addEventListener("click", async (event) => {
  const button = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("button[data-arts]"));
  if (!button || !onList()) return;
  const verb = button.dataset.arts;
  if (verb === "filter") {
    location.hash = hrefFor(button.dataset.project || "");
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
