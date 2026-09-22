// Storage: what the node holds, by kind and by project, and the prune that
// gives it back. Every figure is the hub's own; the screen adds none.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { main, paint, projectName, stale } from "./dom.mjs";
import { EMPTY_COPY, emptyState } from "./empty.mjs";
import { registerScreen } from "./keys.mjs";
import { render } from "./router.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

// The order the bar stacks its segments in and the legend lists them in. The
// two agree, and a segment ends in an edge of the card's own surface, so which
// segment is which never rests on telling two colours apart.
const KINDS = ["events", "sessions", "artifacts", "knowledge"];
const UNITS = ["B", "KB", "MB", "GB", "TB"];
// Spaced after the move command, as in the other glyphs.
const BACK_PATH = "M 15 6l-6 6 6 6";
const UNDO_NOTE = "You can undo for 30 s after pruning.";

registerScreen("storage", { rows: ".storage-row" });

// Three figures and no trailing zero: 5.6 GB, 1.46 GB, 420 MB.
export function formatBytes(bytes) {
  let value = Math.max(0, Number(bytes) || 0);
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${Number(value.toFixed(digits))} ${UNITS[unit]}`;
}

const sessionsOf = (count) => (count === 1 ? "1 session" : `${count} sessions`);

// Every string here reaches the page as text, never as markup: a project id
// is whatever the project was created with, and its name whatever a human typed.
function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

// parts: [kind, bytes] in stacking order. whole: what the full width stands
// for. A kind that holds nothing draws nothing; the text beside the bar still
// lists it.
function stackedBar(parts, whole) {
  const bar = el("div", "storage-bar");
  for (const [kind, bytes] of parts) {
    if (!(bytes > 0) || !(whole > 0)) continue;
    const segment = el("span", "storage-seg");
    segment.dataset.kind = kind;
    segment.style.width = `${Math.min(100, (bytes / whole) * 100)}%`;
    bar.appendChild(segment);
  }
  return bar;
}

function head(usage) {
  const box = el("div", "proj-head storage-head");
  const back = el("a", "proj-back");
  back.href = "#/home";
  back.setAttribute("aria-label", "Back to Home");
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  for (const [name, value] of Object.entries({
    width: "20",
    height: "20",
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    "stroke-width": "1.8",
    "stroke-linecap": "round",
    "stroke-linejoin": "round",
    "aria-hidden": "true",
  }))
    svg.setAttribute(name, value);
  const line = document.createElementNS("http://www.w3.org/2000/svg", "path");
  line.setAttribute("d", BACK_PATH);
  svg.appendChild(line);
  back.appendChild(svg);
  const titles = el("div", "grow");
  titles.appendChild(el("h1", "", "Storage"));
  const where = [usage.data_path, usage.node?.host].filter(Boolean).join(" · ");
  if (where) titles.appendChild(el("p", "storage-path mono", where));
  box.append(back, titles);
  return box;
}

// The share of the volume under which a bar stops being drawn against the
// volume, and what the card says when it does. Home keeps to the same
// threshold and opens with the same words.
export const SLIVER = 0.01;
export const SLIVER_WORDS = "Under 1% of the volume is used";
const SLIVER_NOTE = `${SLIVER_WORDS}. The bar is drawn against what is used.`;

function summary(usage) {
  const card = el("section", "card storage-summary");
  card.setAttribute("aria-label", "Used");
  const figures = el("div", "storage-figures");
  const [amount, unit] = formatBytes(usage.used_bytes).split(" ");
  const used = el("p", "storage-used");
  used.append(el("span", "storage-amount", amount), " ", el("span", "storage-unit", `${unit} used`));
  figures.appendChild(used);
  const measured = usage.capacity_bytes != null;
  if (measured) {
    figures.appendChild(el("p", "storage-capacity mono", `of ${formatBytes(usage.capacity_bytes)}`));
  }

  const parts = KINDS.map((kind) => [kind, usage.by_kind?.[kind] ?? 0]);
  const said = parts.map(([kind, bytes]) => `${kind} ${formatBytes(bytes)}`);
  // Against the volume when it can be measured, so the empty track is the
  // room that is left; against what is used when it cannot. A small hub on a
  // large volume would draw no segment at all at that scale, so under one
  // part in a hundred the bar is drawn against what is used, and says so. No
  // segment is ever widened: every bar is to the scale it names.
  const sliver = measured && usage.used_bytes > 0 && usage.used_bytes < usage.capacity_bytes * SLIVER;
  const bar = stackedBar(parts, measured && !sliver ? usage.capacity_bytes : usage.used_bytes);
  bar.setAttribute("role", "img");
  bar.setAttribute(
    "aria-label",
    `${formatBytes(usage.used_bytes)} used` +
      (measured ? ` of ${formatBytes(usage.capacity_bytes)}` : "") +
      `: ${said.join(", ")}` +
      (sliver ? `. ${SLIVER_NOTE}` : ""),
  );

  const legend = el("ul", "storage-legend");
  parts.forEach(([kind], at) => {
    const item = el("li");
    item.dataset.kind = kind;
    const swatch = el("span", "storage-swatch");
    swatch.dataset.kind = kind;
    swatch.setAttribute("aria-hidden", "true");
    item.append(swatch, said[at]);
    legend.appendChild(item);
  });
  card.append(figures, bar);
  if (sliver) card.appendChild(el("p", "storage-scale", SLIVER_NOTE));
  card.appendChild(legend);
  // The hub store is one file. A row owns the bytes of its own events; the
  // rest of the file is no project's, so the rows' events add up to less than
  // the legend's, and the card says by how much and why.
  if (usage.events_shared_bytes > 0) {
    card.appendChild(
      el(
        "p",
        "storage-shared",
        `Events includes ${formatBytes(usage.events_shared_bytes)} of hub database that every ` +
          `project shares. A row counts only its project's own events.`,
      ),
    );
  }
  return card;
}

// A row's four figures, in the order the bar stacks them. They are the hub's
// own, and the row's total is their sum.
const partsOf = (project) => [
  ["events", project.events_bytes ?? 0],
  ["sessions", project.session_bytes ?? 0],
  ["artifacts", project.artifact_bytes ?? 0],
  ["knowledge", project.kb_bytes ?? 0],
];
const totalOf = (project) => partsOf(project).reduce((sum, [, bytes]) => sum + bytes, 0);
const projectHref = (project) => `#/projects/${encodeURIComponent(project.project_id)}/sessions`;

function projectRow(project) {
  const id = project.project_id;
  const parts = partsOf(project);
  const total = totalOf(project);
  const row = el("div", "row storage-row");
  row.dataset.project = id;

  const top = el("div", "storage-row-top");
  const title = el("div", "title");
  const link = el("a", "", projectName(project));
  link.href = projectHref(project);
  title.appendChild(link);
  top.append(title, el("span", "storage-total mono", formatBytes(total)));

  // The line under it says the same thing in words, so the bar is decoration
  // to a reader who cannot see it.
  const bar = stackedBar(parts, total);
  bar.setAttribute("aria-hidden", "true");

  const foot = el("div", "storage-row-foot");
  foot.appendChild(
    el(
      "span",
      "storage-detail mono",
      parts.map(([kind, bytes]) => `${kind} ${formatBytes(bytes)}`).join(" · "),
    ),
  );
  if (project.prunable_sessions > 0) {
    const prune = el("button", "storage-prune", `Prune ${formatBytes(project.prunable_bytes)}`);
    prune.type = "button";
    prune.dataset.id = id;
    prune.appendChild(el("span", "sr-only", ` in ${projectName(project)}`));
    prune.addEventListener("click", () => pruneProject(project));
    foot.appendChild(prune);
  }
  row.append(top, bar, foot);
  return row;
}

// The hub lists every project, so that one a prune has just emptied keeps its
// row: it still holds its events. A project that never held anything would
// be a row of zeros, and a run of those is noise, so they fold under a count
// and stay one press away.
function idleFold(projects) {
  const fold = el("details", "storage-idle");
  fold.appendChild(
    el("summary", "", projects.length === 1 ? "1 project holds nothing" : `${projects.length} projects hold nothing`),
  );
  const list = el("ul", "storage-idle-list");
  for (const project of projects) {
    const item = el("li");
    const link = el("a", "", projectName(project));
    link.href = projectHref(project);
    item.appendChild(link);
    list.appendChild(item);
  }
  fold.appendChild(list);
  return fold;
}

function pruneAllCard(usage) {
  const card = el("section", "card storage-all");
  const title = el("h2", "storage-all-title", "Prune all ended sessions");
  title.id = "storage-all-title";
  card.setAttribute("aria-labelledby", title.id);
  const holding = usage.projects.filter((project) => project.prunable_sessions > 0);
  const text = el("p", "storage-all-text");
  if (usage.prunable.sessions > 0) {
    text.append(
      `${sessionsOf(usage.prunable.sessions)} across ${holding.length} ` +
        `${holding.length === 1 ? "project" : "projects"} · frees `,
      el("span", "storage-frees", formatBytes(usage.prunable.bytes)),
      ". Feed events and artifacts are never touched by this action.",
    );
  } else {
    text.textContent =
      "No session has ended, so there is nothing to prune. Feed events and artifacts are never touched by this action.";
  }
  card.append(title, text);
  if (usage.prunable.sessions > 0) {
    const review = el("button", "danger storage-review", "Review & prune…");
    review.type = "button";
    review.addEventListener("click", () => pruneAll(usage, holding));
    card.appendChild(review);
  }
  return card;
}

// One request prunes; the answer carries a token per session, and undo is
// the single-session route once for each.
async function commit(path, expected, bytes) {
  let pruned;
  try {
    pruned = await api(path, { method: "DELETE" });
  } catch (error) {
    toast(`Nothing changed: ${error.message}`);
    return;
  }
  await render();
  const tokens = pruned.sessions.map((session) => session.undo_token);
  if (!tokens.length) {
    toast("Nothing was pruned: no session had ended.");
    return;
  }
  // The byte figure was measured for the sessions the dialog named. If the
  // hub pruned a different number, the figure is no longer its own.
  const freed = tokens.length === expected ? `, ${formatBytes(bytes)} freed` : "";
  toast(`Pruned ${sessionsOf(tokens.length)}${freed}. Undo within 30 s.`, async () => {
    try {
      for (const token of tokens) {
        await api(`/api/v1/prune/undo/${encodeURIComponent(token)}`, { method: "POST" });
      }
    } catch (error) {
      toast(`Undo stopped: ${error.message}`);
    }
    await render();
  });
}

export async function pruneProject(project) {
  const id = project.project_id;
  // The listing names what goes. The count and the bytes stay the storage
  // response's, which is what the button already showed.
  let lines = [];
  try {
    const listing = await api(`/api/v1/sessions?project=${encodeURIComponent(id)}`);
    lines = listing.sessions
      .filter((session) => session.status === "ended" && !session.deleted_at)
      .map((session) =>
        [session.id, session.session_name, formatBytes(session.brain_bytes ?? 0)]
          .filter(Boolean)
          .join(" · "),
      );
  } catch {
    lines = [];
  }
  const count = project.prunable_sessions;
  const confirmed = await confirmAction({
    title: `Prune ${sessionsOf(count)} in ${projectName(project)}?`,
    body:
      `The brain files and audit logs of these ended sessions are deleted, freeing ` +
      `${formatBytes(project.prunable_bytes)}. Feed events, artifacts and the project ` +
      `knowledge base stay.`,
    list: lines,
    note: UNDO_NOTE,
    safe: "Keep",
    danger: `Prune ${sessionsOf(count)}`,
  });
  if (!confirmed) return;
  await commit(
    `/api/v1/storage/projects/${encodeURIComponent(id)}/sessions`,
    count,
    project.prunable_bytes,
  );
}

async function pruneAll(usage, holding) {
  const count = usage.prunable.sessions;
  const confirmed = await confirmAction({
    title: `Prune ${sessionsOf(count)}?`,
    body:
      `Every ended session in these projects loses its brain file and audit log, freeing ` +
      `${formatBytes(usage.prunable.bytes)}. Feed events, artifacts and project knowledge ` +
      `bases stay.`,
    list: holding.map(
      (project) =>
        `${projectName(project)} · ${sessionsOf(project.prunable_sessions)} · ` +
        formatBytes(project.prunable_bytes),
    ),
    note: UNDO_NOTE,
    safe: "Keep",
    danger: `Prune ${sessionsOf(count)}`,
  });
  if (!confirmed) return;
  await commit("/api/v1/storage/sessions", count, usage.prunable.bytes);
}

function desktopHead(usage) {
  const head = el("div", "storage-head");
  const textWrap = el("div", "storage-head-text");
  const title = el("h1", "", "Storage");
  const now = new Date();
  const readTime = `${String(now.getHours()).padStart(2, "0")}:${String(now.getMinutes()).padStart(2, "0")}`;
  const desc = el("p", "storage-desc");
  const pathText = usage.data_path || "/var/agent-hub";
  desc.append(
    "Everything on disk under ",
    el("span", "mono", pathText),
    `, read at ${readTime}. Pruning removes ended session brains; artifact blobs are removed only with their project.`,
  );
  textWrap.append(title, desc);
  const remeasure = el("button", "btn storage-remeasure", "Re-measure");
  remeasure.type = "button";
  remeasure.addEventListener("click", () => render());
  head.append(textWrap, remeasure);
  return head;
}

function summaryTiles(usage) {
  const tiles = el("div", "storage-tiles");

  // Tile 1: ON DISK
  const t1 = el("section", "card storage-tile");
  t1.setAttribute("aria-label", "On disk");
  t1.append(
    el("span", "storage-tile-label", "ON DISK"),
    el("span", "storage-tile-val mono", formatBytes(usage.used_bytes)),
    el("span", "storage-tile-sub", `across ${usage.projects.length} projects`),
  );

  // Tile 2: ARTIFACT BLOBS
  const t2 = el("section", "card storage-tile");
  t2.setAttribute("aria-label", "Artifact blobs");
  const blobProjects = usage.projects.filter((p) => (p.artifact_bytes || 0) > 0).length;
  t2.append(
    el("span", "storage-tile-label", "ARTIFACT BLOBS"),
    el("span", "storage-tile-val mono", formatBytes(usage.by_kind?.artifacts ?? 0)),
    el(
      "span",
      "storage-tile-sub",
      `${blobProjects} ${blobProjects === 1 ? "project" : "projects"} holding blobs`,
    ),
  );

  // Tile 3: SESSION BRAINS
  const t3 = el("section", "card storage-tile");
  t3.setAttribute("aria-label", "Session brains");
  const prunableCount = usage.prunable?.sessions || 0;
  t3.append(
    el("span", "storage-tile-label", "SESSION BRAINS"),
    el("span", "storage-tile-val mono", formatBytes(usage.by_kind?.sessions ?? 0)),
    el(
      "span",
      "storage-tile-sub",
      prunableCount > 0 ? `${prunableCount} ended` : "no ended sessions",
    ),
  );

  // Tile 4: RECLAIMABLE
  const t4 = el("section", "card storage-tile reclaimable");
  t4.setAttribute("aria-label", "Reclaimable");
  const foot4 = el("div", "storage-tile-foot");
  foot4.appendChild(
    el(
      "span",
      "storage-tile-sub",
      prunableCount > 0
        ? `${prunableCount} ended session ${prunableCount === 1 ? "brain" : "brains"}`
        : "no ended session brains",
    ),
  );
  if (prunableCount > 0) {
    const pruneAllBtn = el("button", "btn storage-prune-all-btn storage-review", "Prune all");
    pruneAllBtn.type = "button";
    const holding = usage.projects.filter((p) => p.prunable_sessions > 0);
    pruneAllBtn.addEventListener("click", () => pruneAll(usage, holding));
    foot4.appendChild(pruneAllBtn);
  }
  t4.append(
    el("span", "storage-tile-label", "RECLAIMABLE"),
    el("span", "storage-tile-val mono", formatBytes(usage.prunable?.bytes ?? 0)),
    foot4,
  );

  tiles.append(t1, t2, t3, t4);
  return tiles;
}

function desktopTable(usage) {
  const wrap = el("div", "storage-table-wrap");
  const table = el("table", "storage-table");

  const thead = el("thead");
  const trHead = el("tr");
  const thCols = [
    ["PROJECT", "th-project"],
    ["SHARE", "th-share"],
    ["TOTAL", "th-total num"],
    ["BLOBS", "th-blobs num"],
    ["BRAINS", "th-brains num"],
    ["RECLAIMABLE", "th-reclaimable num"],
    ["LAST WRITE", "th-lastwrite num"],
    ["", "th-action"],
  ];
  for (const [title, className] of thCols) {
    const th = el("th", className);
    th.setAttribute("scope", "col");
    if (title) th.textContent = title;
    else th.appendChild(el("span", "sr-only", "Actions"));
    trHead.appendChild(th);
  }
  thead.appendChild(trHead);
  table.appendChild(thead);

  const tbody = el("tbody");
  let sumTotal = 0;
  let sumBlobs = 0;
  let sumBrains = 0;
  let sumReclaimable = 0;

  usage.projects.forEach((project, index) => {
    const total = totalOf(project);
    sumTotal += total;
    sumBlobs += project.artifact_bytes || 0;
    sumBrains += project.session_bytes || 0;
    const prunableBytes = project.prunable_bytes || 0;
    const prunableSessions = project.prunable_sessions || 0;
    if (prunableSessions > 0) sumReclaimable += prunableBytes;

    const row = el("tr", "storage-row storage-table-row");
    row.dataset.project = project.project_id;
    row.tabIndex = 0;

    // 1. PROJECT
    const tdProject = el("td", "cell-project");
    const isLive = (project.agents_active || 0) > 0;
    const dot = el("span", `rail-dot ${isLive ? "live" : "idle"}`);
    dot.setAttribute("aria-hidden", "true");
    const link = el("a", "storage-proj-link", projectName(project));
    link.href = projectHref(project);
    tdProject.append(dot, link);
    row.appendChild(tdProject);

    // 2. SHARE
    const tdShare = el("td", "cell-share");
    const blobBytes = project.artifact_bytes || 0;
    const brainBytes = project.session_bytes || 0;
    const shareBar = el("div", "storage-share-bar");
    shareBar.setAttribute("role", "img");
    shareBar.setAttribute(
      "aria-label",
      `Share: ${formatBytes(blobBytes)} blobs, ${formatBytes(brainBytes)} brains of ${formatBytes(total)} total`,
    );
    if (total > 0) {
      if (blobBytes > 0) {
        const segBlobs = el("span", "storage-share-seg blobs");
        segBlobs.style.width = `${Math.min(100, (blobBytes / total) * 100)}%`;
        segBlobs.setAttribute("title", `Blobs: ${formatBytes(blobBytes)}`);
        shareBar.appendChild(segBlobs);
      }
      if (brainBytes > 0) {
        const segBrains = el("span", "storage-share-seg brains");
        segBrains.style.width = `${Math.min(100, (brainBytes / total) * 100)}%`;
        segBrains.setAttribute("title", `Brains: ${formatBytes(brainBytes)}`);
        shareBar.appendChild(segBrains);
      }
    }
    tdShare.appendChild(shareBar);
    row.appendChild(tdShare);

    // 3. TOTAL
    const tdTotal = el("td", "cell-total num");
    tdTotal.appendChild(el("span", "mono bold", formatBytes(total)));
    row.appendChild(tdTotal);

    // 4. BLOBS
    const tdBlobs = el("td", "cell-blobs num");
    tdBlobs.appendChild(el("span", "mono", formatBytes(blobBytes)));
    row.appendChild(tdBlobs);

    // 5. BRAINS
    const tdBrains = el("td", "cell-brains num");
    tdBrains.appendChild(el("span", "mono", formatBytes(brainBytes)));
    row.appendChild(tdBrains);

    // 6. RECLAIMABLE
    const tdReclaimable = el("td", "cell-reclaimable num");
    if (prunableSessions > 0 && prunableBytes > 0) {
      tdReclaimable.appendChild(el("span", "mono action bold", formatBytes(prunableBytes)));
    } else {
      tdReclaimable.appendChild(el("span", "dash", "\u2014"));
    }
    row.appendChild(tdReclaimable);

    // 7. LAST WRITE
    const tdLastWrite = el("td", "cell-lastwrite num");
    if (project.last_write) {
      const timeNode = el("time", "ts", relative(project.last_write));
      timeNode.setAttribute("datetime", project.last_write);
      tdLastWrite.appendChild(timeNode);
    } else {
      tdLastWrite.appendChild(el("span", "dash", "\u2014"));
    }
    row.appendChild(tdLastWrite);

    // 8. ACTION
    const tdAction = el("td", "cell-action");
    if (prunableSessions > 0 && prunableBytes > 0) {
      const pruneBtn = el("button", "btn storage-prune", "Prune");
      pruneBtn.type = "button";
      pruneBtn.dataset.id = project.project_id;
      pruneBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        pruneProject(project);
      });
      tdAction.appendChild(pruneBtn);
    }
    row.appendChild(tdAction);

    tbody.appendChild(row);
  });
  table.appendChild(tbody);

  // tfoot (Totals row)
  const tfoot = el("tfoot");
  const trFoot = el("tr", "storage-totals-row");
  const tfProject = el("td", "cell-project");
  tfProject.appendChild(
    el("span", "totals-label", `${usage.projects.length} ${usage.projects.length === 1 ? "project" : "projects"}`),
  );
  trFoot.appendChild(tfProject);
  trFoot.appendChild(el("td", "cell-share"));

  const tfTotal = el("td", "cell-total num");
  tfTotal.appendChild(el("span", "mono bold", formatBytes(sumTotal)));
  trFoot.appendChild(tfTotal);

  const tfBlobs = el("td", "cell-blobs num");
  tfBlobs.appendChild(el("span", "mono", formatBytes(usage.by_kind?.artifacts ?? sumBlobs)));
  trFoot.appendChild(tfBlobs);

  const tfBrains = el("td", "cell-brains num");
  tfBrains.appendChild(el("span", "mono", formatBytes(usage.by_kind?.sessions ?? sumBrains)));
  trFoot.appendChild(tfBrains);

  const tfReclaimable = el("td", "cell-reclaimable num");
  tfReclaimable.appendChild(el("span", "mono action bold", formatBytes(usage.prunable?.bytes ?? sumReclaimable)));
  trFoot.appendChild(tfReclaimable);

  trFoot.appendChild(el("td", "cell-lastwrite"));
  trFoot.appendChild(el("td", "cell-action"));
  tfoot.appendChild(trFoot);
  table.appendChild(tfoot);

  wrap.appendChild(table);

  const container = document.createDocumentFragment();
  container.append(wrap);
  container.appendChild(
    el(
      "p",
      "storage-footnote mono",
      "A dash is a project with no ended sessions. Free space on the volume is not shown \u2014 the hub cannot read it.",
    ),
  );
  return container;
}

function renderMobile(root, usage) {
  root.appendChild(head(usage));
  root.appendChild(summary(usage));
  const byProject = el("section", "storage-by");
  const label = el("h2", "section-label", "By project");
  label.id = "storage-by-title";
  byProject.setAttribute("aria-labelledby", label.id);
  const list = el("div", "storage-projects");
  const holding = usage.projects.filter((project) => totalOf(project) > 0 || project.prunable_sessions > 0);
  const idle = usage.projects.filter((project) => !holding.includes(project));
  for (const project of holding) list.appendChild(projectRow(project));
  byProject.append(label, list);
  if (idle.length) byProject.appendChild(idleFold(idle));
  root.append(byProject, pruneAllCard(usage));
}

async function renderDesktop(root, usage) {
  root.classList.add("storage-desktop-view");
  root.appendChild(desktopHead(usage));
  root.appendChild(summaryTiles(usage));

  // No second round of requests. This asked `/stats` once per project purely
  // to learn whether each one had a live agent, so a hub with six projects
  // made six calls before the table could be drawn, and the screen sat with a
  // heading and nothing under it for as long as that took. The count travels
  // with the usage payload now, in one grouped query.
  root.appendChild(desktopTable(usage));
}

if (typeof window !== "undefined" && window.matchMedia) {
  const mq = window.matchMedia("(min-width: 720px)");
  if (mq.addEventListener) {
    mq.addEventListener("change", () => {
      if (location.hash.startsWith("#/storage")) render();
    });
  }
}

export async function storageScreen(gen) {
  const usage = await api("/api/v1/storage");
  paint(gen, '<div class="storage"></div>');
  if (stale(gen)) return;
  const root = main.querySelector(".storage");
  const isDesktop = window.matchMedia("(min-width: 720px)").matches;

  // The hub store is never empty, so nothing stored means projects hold nothing.
  //
  // The heading goes in before that check rather than after it. A screen with
  // nothing on it is still a screen the reader arrived at, and the router
  // moves focus to a heading to announce it; with the empty state returning
  // first there was no heading to move to, so arriving at an empty Storage
  // announced nothing and left focus on the region.
  if (!usage.total_bytes && !usage.projects.some((project) => totalOf(project) > 0)) {
    root.appendChild(isDesktop ? desktopHead(usage) : head(usage));
    root.appendChild(emptyState(EMPTY_COPY.storage));
    return;
  }

  if (isDesktop) {
    await renderDesktop(root, usage);
  } else {
    renderMobile(root, usage);
  }
}
