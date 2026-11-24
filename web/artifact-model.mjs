// What the artifacts list says, worked out from the projects' own artifact
// lists: which artifacts moved since the reader looked, which are earlier and
// how far back the folded rest reaches, the line under each name, and the
// threads in a comment list. Pure functions over plain data, so the words are
// held by unit tests rather than by a page.

import { bylineOf } from "./author.mjs";

// What the reader has looked at, on this device: the version of each artifact
// they last saw, and when the record began. An artifact is new to the reader
// when its version moved past the one they saw, or when it was published after
// the record began and they have not opened it.
export const SEEN_KEY = "hub.artifacts.seen";

function store() {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

const int = (value) => Math.max(0, Math.trunc(Number(value)) || 0);

export function readSeen(storage = store()) {
  try {
    const parsed = JSON.parse(storage?.getItem(SEEN_KEY) || "null");
    if (!parsed || typeof parsed !== "object" || typeof parsed.seen !== "object" || !parsed.seen) return null;
    return { at: int(parsed.at), seen: parsed.seen };
  } catch {
    return null;
  }
}

function writeSeen(state, storage = store()) {
  try {
    storage?.setItem(SEEN_KEY, JSON.stringify(state));
  } catch {}
}

// Everything listed counts as seen at its current version, added to what was
// seen before, so marking one project's artifacts leaves the rest as they were.
// The first look records this, so a device that has never looked opens on
// nothing new rather than on everything.
export function seeAll(artifacts, now = Date.now(), storage = store()) {
  const state = readSeen(storage) || { at: now, seen: {} };
  for (const artifact of artifacts) state.seen[artifact.id] = int(artifact.version) || 1;
  writeSeen(state, storage);
  return state;
}

// One artifact read at its newest version. A version the reader already saw
// past is kept.
export function seeOne(id, version, now = Date.now(), storage = store()) {
  const state = readSeen(storage) || { at: now, seen: {} };
  state.seen[id] = Math.max(int(state.seen[id]), int(version) || 1);
  writeSeen(state, storage);
  return state;
}

export function changedSince(artifact, state) {
  if (!state) return false;
  const saw = int(state.seen[artifact.id]);
  if (saw) return (int(artifact.version) || 1) > saw;
  const created = Date.parse(artifact.created_at || "");
  return Number.isFinite(created) && created > state.at;
}

const newest = (a, b) => Date.parse(b.updated_at || b.created_at) - Date.parse(a.updated_at || a.created_at);

// The two tiers. Each is newest first; the earlier tier shows `limit` rows and
// says how many more there are and how far back they reach.
export function artifactTiers(artifacts, state, { limit = 10, all = false } = {}) {
  const since = [];
  const rest = [];
  for (const artifact of artifacts) (changedSince(artifact, state) ? since : rest).push(artifact);
  since.sort(newest);
  rest.sort(newest);
  const earlier = all ? rest : rest.slice(0, limit);
  const folded = rest.slice(earlier.length);
  const last = folded[folded.length - 1];
  return {
    since,
    earlier,
    total: rest.length,
    more: folded.length,
    backTo: last ? Date.parse(last.updated_at || last.created_at) : null,
  };
}

// The file name a reader knows an artifact by: its title as a slug, with the
// extension its kind is served as.
export function artifactSlug(title, kind) {
  const base =
    String(title || "artifact")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "artifact";
  return base + (kind === "markdown" ? ".md" : ".html");
}

const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

// What the comments on an artifact come to, in the fewest words: how many,
// and how many of them are still open.
export function commentWords(total, open) {
  const all = int(total);
  const live = Math.min(int(open), all);
  if (!all) return "";
  return live ? `${plural(all, "comment", "comments")}, ${live} open` : plural(all, "comment", "comments");
}

// The line under an artifact's name: where it lives, who wrote it, its
// version, and the one thing about it worth knowing. The project is named as
// the rail names it when the caller knows that name.
export function artifactLine(artifact) {
  const parts = [artifact.project_name || artifact.project_id];
  const by = bylineOf(artifact).subject;
  if (artifact.author || artifact.actor) parts.push(by);
  parts.push(`v${int(artifact.version) || 1}`);
  if (artifact.live_version) parts.push("being written");
  else if (artifact.protected) parts.push("encrypted in your browser");
  const comments = commentWords(artifact.comments_count, artifact.comments_open);
  if (comments) parts.push(comments);
  else if (artifact.label && !artifact.protected) parts.push(artifact.label);
  return parts.filter(Boolean).join(" · ");
}

// What an artifact's comments say in the preview.
export function commentSummary(total, open) {
  const all = int(total);
  const live = int(open);
  if (!all) return "None";
  return live ? `${all}, ${live} open` : `${all}, none open`;
}

// A version's one line: its label, or its size when the writer gave none.
export function versionSummary(version, bytes = formatBytes) {
  const label = typeof version.label === "string" ? version.label.trim() : "";
  if (label) return label;
  const description = typeof version.description === "string" ? version.description.trim() : "";
  return description || bytes(version.size_bytes);
}

export function formatBytes(bytes) {
  const value = Number(bytes) || 0;
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${Math.round(value / 1024)} KB`;
  if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

// The header's count: how many artifacts, in how many projects, and their size.
export function listMeta(artifacts) {
  const projects = new Set(artifacts.map((a) => a.project_id)).size;
  const bytes = artifacts.reduce((sum, a) => sum + (Number(a.size_bytes) || 0), 0);
  return `${artifacts.length} in ${plural(projects, "project", "projects")} · ${formatBytes(bytes)}`;
}

// The projects the list can be narrowed to, with their counts, in the order
// the hub lists them.
export function projectChips(artifacts, projects) {
  const counts = new Map();
  for (const artifact of artifacts) counts.set(artifact.project_id, (counts.get(artifact.project_id) || 0) + 1);
  return projects
    .filter((project) => counts.has(project.id))
    .map((project) => ({ id: project.id, name: project.display_name || project.id, count: counts.get(project.id) }));
}

// The comments as threads: a comment on a passage opens a thread, and a later
// comment on the same passage of the same version is a reply in it. A comment
// with no passage is a thread of its own. Threads keep the order they are
// given; replies read oldest first.
export function commentThreads(comments) {
  const threads = [];
  const byPassage = new Map();
  const oldestFirst = [...comments].sort((a, b) => Date.parse(a.created_at) - Date.parse(b.created_at));
  for (const comment of oldestFirst) {
    const quote = comment.anchor && comment.anchor.mode === "text" ? comment.anchor.quote : "";
    const key = quote ? `${comment.anchor_version}\u0000${quote}` : "";
    const root = key && byPassage.get(key);
    if (root && !root.root.done && !comment.done) {
      root.replies.push(comment);
    } else {
      const thread = { root: comment, replies: [] };
      if (key && !comment.done) byPassage.set(key, thread);
      threads.push(thread);
    }
  }
  const order = new Map(comments.map((comment, at) => [comment.id, at]));
  return threads.sort((a, b) => order.get(a.root.id) - order.get(b.root.id));
}

// The open count every surface shows: open threads, not open comments, so a
// thread with replies counts once.
export function openThreadCount(comments) {
  return commentThreads(comments.filter((comment) => !comment.done)).length;
}
