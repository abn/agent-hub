// Storage: what the node holds, by kind and by project, and the prune that
// gives it back. Every figure is the hub's own; the screen adds none.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { main, paint, stale } from "./dom.mjs";
import { EMPTY_COPY, emptyState } from "./empty.mjs";
import { registerScreen } from "./keys.mjs";
import { render } from "./router.mjs";
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
// is whatever the project was created with.
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
  return card;
}

function projectRow(project) {
  const id = project.project_id;
  const parts = [
    ["sessions", project.session_bytes],
    ["artifacts", project.artifact_bytes],
    ["knowledge", project.kb_bytes],
  ];
  const total = parts.reduce((sum, [, bytes]) => sum + bytes, 0);
  const row = el("div", "row storage-row");
  row.dataset.project = id;

  const top = el("div", "storage-row-top");
  const title = el("div", "title");
  const link = el("a", "", id);
  link.href = `#/projects/${encodeURIComponent(id)}/sessions`;
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
    prune.appendChild(el("span", "sr-only", ` in ${id}`));
    prune.addEventListener("click", () => pruneProject(project));
    foot.appendChild(prune);
  }
  row.append(top, bar, foot);
  return row;
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

async function pruneProject(project) {
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
    title: `Prune ${sessionsOf(count)} in ${id}?`,
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
        `${project.project_id} · ${sessionsOf(project.prunable_sessions)} · ` +
        formatBytes(project.prunable_bytes),
    ),
    note: UNDO_NOTE,
    safe: "Keep",
    danger: `Prune ${sessionsOf(count)}`,
  });
  if (!confirmed) return;
  await commit("/api/v1/storage/sessions", count, usage.prunable.bytes);
}

export async function storageScreen(gen) {
  const usage = await api("/api/v1/storage");
  paint(gen, '<div class="storage"></div>');
  if (stale(gen)) return;
  const root = main.querySelector(".storage");
  root.appendChild(head(usage));
  // The hub's own store is never empty, so "nothing stored" is the projects
  // holding nothing: no brain, no artifact, no knowledge base.
  if (!usage.total_bytes) {
    root.appendChild(emptyState(EMPTY_COPY.storage));
    return;
  }
  root.appendChild(summary(usage));
  const byProject = el("section", "storage-by");
  const label = el("h2", "section-label", "By project");
  label.id = "storage-by-title";
  byProject.setAttribute("aria-labelledby", label.id);
  const list = el("div", "storage-projects");
  for (const project of usage.projects) list.appendChild(projectRow(project));
  byProject.append(label, list);
  root.append(byProject, pruneAllCard(usage));
}
