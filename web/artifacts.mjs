// Artifacts: the per-project index, and the artifact read in the stage beside
// it with its comments in the aside. An artifact has one surface, so the
// address a link carries, `#/artifacts/<id>`, opens its project's Artifacts
// shell with the artifact selected, and reload and the browser's Back keep it.

import { api } from "./api.mjs";
import {
  closeCommentsDrawer,
  loadComments,
  commentsPanel,
  commentsState,
  commentsToggle,
  openCommentsDrawer,
  openCompose,
  renderDesktopCards,
  startComments,
} from "./comments.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, main, paint, stale } from "./dom.mjs";
import { EMPTY_COPY, emptyStateHTML } from "./empty.mjs";
import { onArtifactLive } from "./events.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { render } from "./router.mjs";
import { asideAvailable, toggleAside } from "./shell-layout.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

const DOC_PATH = "M 7 3h7l4 4v14H7z M 14 3v4h4";
const LOCK_PATH = "M 7 11V8a5 5 0 0 1 10 0v3 M 5 11h14v10H5z";

function cardGlyph(protectedArtifact) {
  const path = protectedArtifact ? LOCK_PATH : DOC_PATH;
  const cls = protectedArtifact ? "lock" : "doc";
  return `<svg class="${cls}" aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="${path}"></path></svg>`;
}

function formatBytes(bytes) {
  const value = Number(bytes) || 0;
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${Math.round(value / 1024)} KB`;
  if (value < 1024 * 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MB`;
  return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

// The live band: whether a version is being written, who is writing it, and
// whether the reader is following. It rides above the document in the stage.
// A sealed artifact hides it, so it costs nothing while nothing is live.
function liveBandElement() {
  const band = document.createElement("div");
  band.className = "hub-live-band";
  band.hidden = true;
  band.setAttribute("role", "status");
  band.innerHTML =
    `<span class="hub-live-dot" aria-hidden="true"></span>` +
    `<span class="hub-live-label mono"></span>` +
    `<button type="button" class="hub-live-follow" data-action="live-follow" aria-pressed="true" hidden>Following</button>`;
  return band;
}

// Watch one artifact. The band appears while a version is live or idle; the
// document keeps up while the reader is following, and a reader who has
// scrolled away can pause so the page stops moving under them.
function wireLiveBand(id, kind, protectedArtifact, frame, band) {
  let following = true;
  let pending = 0;
  let version = null;

  const redraw = () => {
    const state = band.dataset.state || "sealed";
    band.hidden = state === "sealed";
    band.dataset.following = following ? "yes" : "no";
    const agent = band.dataset.agent || "";
    const who = agent ? ` · ${agent}` : "";
    const extra = pending ? ` · ${pending} update${pending === 1 ? "" : "s"}` : "";
    band.querySelector(".hub-live-label").textContent = `${state}${who}${extra}`;
    const follow = band.querySelector(".hub-live-follow");
    follow.hidden = band.hidden;
    follow.textContent = following ? "Following" : "Paused";
    follow.setAttribute("aria-pressed", String(following));
  };

  const refresh = async () => {
    try {
      const state = await api(`/api/v1/artifacts/${encodeURIComponent(id)}/live`);
      band.dataset.state = state.state;
      band.dataset.agent = state.actor || "";
      version = state.version || null;
      liveVersion = state.version || null;
      // A version sheet the stage mounted before this read answered already
      // labelled the row; relabel it now that the answer is known.
      if (liveVersion) {
        const row = document.querySelector(`.hub-version-row[data-version="${liveVersion}"]`);
        const primary = row && row.querySelector(".hub-version-primary");
        if (primary) primary.textContent = "Being written";
      }
    } catch {
      // A refusal leaves the band as it was: a live read is a nicety, not the
      // document, so losing it costs nothing the reader has to be told about.
    }
  };

  const apply = async (at) => {
    if (protectedArtifact || !at || !frame || !frame.contentWindow) return;
    const payload = { type: "hub:set-live", kind, version: at };
    if (kind === "markdown") {
      try {
        payload.content = await fetchRawText(id, at);
      } catch {
        return;
      }
    }
    frame.contentWindow.postMessage(payload, "*");
  };

  band.querySelector(".hub-live-follow").addEventListener("click", async () => {
    following = !following;
    if (following) {
      pending = 0;
      await apply(version);
    }
    redraw();
  });

  const off = onArtifactLive(async (event) => {
    // A band whose artifact the reader has navigated away from stops
    // listening on the next event of any kind, not only its own.
    if (!band.isConnected) {
      off();
      return;
    }
    if (!event || event.artifact_id !== id) return;
    const was = band.dataset.state;
    await refresh();
    // The event names the version that moved, so a seal (which clears the
    // pointer) still renders the settled bytes.
    const at = event.version || version;
    if (following) {
      pending = 0;
      await apply(at);
    } else {
      pending += 1;
    }
    // Sealing is the end of the live session. Rebuild the screen once so the
    // meta line, the picker and the band all settle on the sealed version
    // rather than on the numbers read when the page opened.
    if (was && was !== "sealed" && band.dataset.state === "sealed") {
      render();
      return;
    }
    redraw();
  });
  band._unwatch = off;

  refresh().then(redraw);
}

// The project the stage last showed each artifact in. An in-app link that names
// no project, a comment's "open v2" among them, is resolved from it rather than
// by asking every project for its list.
const stagedIn = new Map();

// The artifact read in the stage. The index lists, the stage shows the
// document: the title and its provenance in the header, the breadcrumb and the
// document controls in the control row, the sandboxed page in the body. The
// document is the hub's own frame route, so the app never renders agent HTML
// in its own origin. A version other than the newest is read when the address
// names one.
export async function artifactStage(id, projectId, version = null) {
  let versions = [];
  try {
    const listed = await api(`/api/v1/artifacts/${encodeURIComponent(id)}/versions`);
    versions = listed.versions || [];
  } catch {
    return null;
  }
  const newest = (versions.length && versions[versions.length - 1].version) || 1;
  const asked = Number(version) || newest;
  const current = versions.find((v) => v.version === asked) || versions.find((v) => v.version === newest) || versions[0];
  if (!current) return null;
  const shown = current.version;

  let commentsCount = 0;
  let openCommentsCount = 0;
  try {
    const list = await api(`/api/v1/artifacts/${encodeURIComponent(id)}/comments`);
    const arr = Array.isArray(list) ? list : Array.isArray(list?.comments) ? list.comments : [];
    commentsCount = arr.length;
    openCommentsCount = arr.filter((c) => !c.done).length;
  } catch {}

  const project = projectId || "";
  stagedIn.set(id, project);
  const slug =
    (current.title || "artifact")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") + (current.kind === "markdown" ? ".md" : ".html");

  // One control in two states. With no thread yet there is nothing to read, so
  // it starts one; once a thread exists it opens the threads, and the numeral
  // beside the bubble is the open count, kept live by the comments module.
  const hasThreads = commentsCount > 0;
  const comments = hasThreads
    ? `<span class="hub-glyph-count mono" aria-hidden="true">${openCommentsCount}</span>`
    : "";
  const stagePath = `${project} / artifacts / ${slug}`;
  // The More control carries every action on the artifact. The wrapper is the
  // project header's own overflow pattern, so the menu hangs from its trigger
  // in the head and cannot open over another pane.
  const actions = `
    <button type="button" class="hub-btn-glyph${hasThreads ? " hub-comments-btn" : ""}" data-action="comments-toggle" data-threads="${
      hasThreads ? "yes" : "no"
    }" aria-label="${hasThreads ? `Comments, ${openCommentsCount}` : "Start a thread"}">${glyphSvg("comments", { size: 18 })}${comments}</button>
    <span class="proj-overflow-wrap">
      <button type="button" class="hub-btn-glyph" data-action="stage-more" aria-label="More" aria-haspopup="menu" aria-expanded="false">${glyphSvg("overflow", { size: 18 })}</button>
      <div class="proj-overflow-menu" role="menu" hidden>
        <button type="button" role="menuitem" class="proj-menu-item" data-action="stage-start-thread">Start a thread</button>
        <button type="button" role="menuitem" class="proj-menu-item" data-action="stage-comments">Comments</button>
        <button type="button" role="menuitem" class="proj-menu-item" data-action="copy-raw" data-id="${esc(id)}" data-version="${shown}">Copy raw</button>
        <button type="button" role="menuitem" class="proj-menu-item" data-action="copy-path" data-path="${esc(stagePath)}">Copy path</button>
        <button type="button" role="menuitem" class="proj-menu-item" data-action="stage-copy-link">Copy link</button>
        <button type="button" role="menuitem" class="proj-menu-item" data-action="stage-share">Share</button>
        <button type="button" role="menuitem" class="proj-menu-item" data-action="stage-open-in-browser">Open in browser</button>
      </div>
    </span>`;

  const copyGlyph = glyphSvg("copy", { size: 14 });
  const rawGlyph = glyphSvg("copyRaw", { size: 14 });

  const controls = `
    <span class="shell-meta mono" title="${esc(stagePath)}">${esc(stagePath)}</span>
    <div class="grow"></div>
    <button type="button" class="hub-btn-glyph" data-action="copy-path" data-path="${esc(
      stagePath,
    )}" aria-label="Copy path ${esc(slug)}">${copyGlyph}</button>
    <button type="button" class="hub-version-toggle mono" data-action="version-toggle" aria-haspopup="true" aria-expanded="false" aria-controls="hub-version-menu" aria-label="Version ${shown}">v${shown} of ${versions.length} ${glyphSvg("chevronDown", { size: 11, strokeWidth: 2 })}</button>
    <button type="button" class="hub-btn-glyph" data-action="copy-raw" data-id="${esc(id)}" data-version="${shown}" aria-label="Copy raw">${rawGlyph}</button>`;

  const theme = document.documentElement.dataset.theme === "dark" ? "dark" : "light";
  // A plain artifact with a live share link is concealed at its id address, so
  // the frame asks for a pass and reads the page with it.
  const pass = await viewerPass(id);
  // While threads exist, a bar rides the bottom of the reader's viewport and
  // opens them, so the way to the threads is never scrolled away.
  const strip = hasThreads
    ? `<button type="button" class="hub-comments-strip" data-action="comments-strip">${glyphSvg("comments", {
        size: 16,
      })}<span class="grow">Comments</span><span class="mono hub-comments-count">${openCommentsCount}</span>${glyphSvg(
        "chevronRight",
        { size: 16 },
      )}</button>`
    : "";
  // The artifact's own host page. It carries the document and, for a protected
  // artifact, the gate and the client-side decryptor, so the app never renders
  // agent bytes in its own origin.
  const body = `<div class="hub-viewer-doc"><iframe id="hub-frame" sandbox="allow-scripts" data-pass="${esc(
    pass,
  )}" title="${esc(current.title || slug)}" src="${esc(frameSrc(id, shown, theme, pass))}"></iframe>${strip}</div>`;

  const plus = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M12 6v12M6 12h12"></path></svg>`;
  // The open count is replaced live and the version it was built with is kept,
  // so the aside, the header control and the bar always say the same number.
  const aside = `
    <div class="shell-head">
      <div class="shell-title"><span class="shell-title-line">Comments</span></div>
      <span class="shell-meta mono hub-comments-head-meta" data-version="${shown}">${openCommentsCount} · v${shown}</span>
      <button type="button" class="hub-btn-glyph" data-action="new-thread" aria-label="New thread">${plus}</button>
    </div>
    <div class="shell-controls"></div>
    <div class="shell-body"><div class="hub-comments-cards-list"></div></div>
    <div class="shell-foot mono">Select text in the document to anchor a comment</div>`;

  // A protected artifact names no author: what the reader can know about it is
  // that it is sealed.
  const who = current.protected ? "Locked · only readable with the password" : current.actor || "agent";
  return {
    aside,
    title: slug,
    meta: `${who} · v${shown} of ${versions.length} · ${formatBytes(current.size_bytes)} · ${relative(
      Date.parse(current.created_at),
    )}`,
    actions,
    controls,
    body,
    commentsCount,
    hasThreads,
    version: shown,
    versions,
    current,
    kind: current.kind,
    protected: Boolean(current.protected),
  };
}

// The stage's own wiring: the frame reports its height so it never scrolls
// inside itself, the copy controls confirm in place, and the comments control
// opens the thread for the version on screen.
export function wireArtifactStage(root, id, info) {
  const frame = root.querySelector("#hub-frame");
  if (frame) {
    const onHeight = (event) => {
      if (event.source !== frame.contentWindow) return;
      const h = event.data && event.data.hubFrameHeight;
      if (typeof h !== "number" || !isFinite(h)) return;
      frame.style.height = `${Math.max(Math.round(h), 200)}px`;
    };
    window.addEventListener("message", onHeight);
    const doc = root.querySelector(".hub-viewer-doc");
    if (doc && info) {
      const band = liveBandElement();
      doc.prepend(band);
      wireLiveBand(id, info.kind, info.protected, frame, band);
    }
  }
  if (info) startComments(id, info.version, info.protected);
  const coarse = window.matchMedia("(pointer: coarse)").matches;
  // The drawer is mounted at every width. On a fine pointer the aside is the
  // reading surface, but a text selection and Start a thread open the compose
  // sheet, and openCompose renders into this drawer. Without it the selection
  // callout posted a message nothing answered, so commenting on selected text
  // did nothing on a desktop. It is mounted in the screen, so the next paint
  // takes it with the stage rather than leaving one drawer per visit behind.
  const { toggle, badge } = commentsToggle();
  const { backdrop, drawer } = commentsPanel({ toggle, badge });
  root.append(backdrop, drawer);
  const aside = root.querySelector(".shell-aside");
  const commentsBtn = root.querySelector('.shell-stage [data-action="comments-toggle"]');
  // The threads are read in one place: the aside beside the document where the
  // layout draws one, and one bottom sheet where it does not, which is a coarse
  // pointer at any width and a fine pointer below the desktop shell. The layout
  // is asked at the moment of the press, so a window resized since the paint
  // still opens the surface it shows.
  const openThreads = () => {
    if (!asideAvailable(aside)) {
      openCommentsDrawer();
      return;
    }
    if (aside.hidden && commentsBtn) toggleAside(commentsBtn);
    renderDesktopCards();
  };
  if (!coarse) {
    const list = root.querySelector(".hub-comments-cards-list");
    if (list) commentsState.desktopContainer = list;
    loadComments();
    renderDesktopCards();
  } else if (aside) {
    aside.hidden = true;
  }
  if (commentsBtn) {
    const threads = commentsBtn.dataset.threads === "yes";
    if (threads && asideAvailable(aside)) commentsBtn.setAttribute("aria-pressed", String(!aside.hidden));
    else commentsBtn.setAttribute("aria-haspopup", "dialog");
    commentsBtn.addEventListener("click", () => {
      if (!threads) {
        openCompose(null);
      } else if (!asideAvailable(aside)) {
        if (commentsState.open && commentsState.viewMode === "list") closeCommentsDrawer();
        else openCommentsDrawer();
      } else {
        toggleAside(commentsBtn);
        if (!aside.hidden) renderDesktopCards();
      }
    });
  }
  root.querySelector('[data-action="comments-strip"]')?.addEventListener("click", openThreads);
  root.querySelector('[data-action="new-thread"]')?.addEventListener("click", () => openCompose(null));
  for (const btn of root.querySelectorAll('[data-action="copy-path"]')) {
    btn.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(btn.dataset.path);
      } catch {}
      toast("Path copied");
    });
  }
  for (const btn of root.querySelectorAll('[data-action="copy-raw"]')) {
    btn.addEventListener("click", async () => {
      try {
        const text = await fetchRawText(btn.dataset.id, Number(btn.dataset.version));
        await navigator.clipboard.writeText(text);
        toast("Raw text copied");
      } catch {
        toast("Failed to copy raw text");
      }
    });
  }
  // The stage's More menu, hung from its own trigger in the header. Comments
  // opens the surface this pointer reads threads on, and Start a thread opens
  // the composer. Copy raw and Copy path are wired by the loops above, because
  // the menu carries the same data-actions.
  const stageMore = root.querySelector('[data-action="stage-more"]');
  const stageMenu = stageMore?.closest(".proj-overflow-wrap")?.querySelector(".proj-overflow-menu");
  let share = null;
  if (info && info.current) {
    share = buildShareSheet(id, info.current, info.version, stageMore);
    root.append(share.backdrop, share.sheet);
  }
  if (stageMore && stageMenu) {
    const closeMenu = () => {
      stageMenu.hidden = true;
      stageMore.setAttribute("aria-expanded", "false");
    };
    const runs = {
      "stage-start-thread": () => openCompose(null),
      "stage-comments": openThreads,
      "stage-copy-link": async () => {
        try {
          await navigator.clipboard.writeText(location.href);
        } catch {}
        toast("Link copied");
      },
      "stage-share": () => share?.openSheet(),
      "stage-open-in-browser": () => {
        const frame = root.querySelector("#hub-frame");
        if (frame) window.open(frame.src, "_blank");
      },
    };
    stageMore.addEventListener("click", (e) => {
      e.stopPropagation();
      const open = stageMenu.hidden;
      stageMenu.hidden = !open;
      stageMore.setAttribute("aria-expanded", String(open));
      if (open) stageMenu.querySelector("button")?.focus();
    });
    for (const item of stageMenu.querySelectorAll("button")) {
      item.addEventListener("click", () => {
        closeMenu();
        const run = runs[item.dataset.action];
        if (run) run();
      });
    }
    // The project header's outside-click handler hides this menu with every
    // other, but it clears aria-expanded on `.proj-overflow-btn` alone, and this
    // trigger is a glyph button. Its state is cleared here.
    const wrap = stageMore.closest(".proj-overflow-wrap");
    document.addEventListener("click", (e) => {
      if (wrap && wrap.isConnected && !wrap.contains(e.target)) closeMenu();
    });
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Escape" || stageMenu.hidden || !stageMore.isConnected) return;
      e.stopPropagation();
      closeMenu();
      stageMore.focus();
    });
  }

  // The version control opens the version sheet; a pick reads that version in
  // this same stage.
  if (info && Array.isArray(info.versions) && info.versions.length > 0) {
    const { backdrop, sheet } = buildVersionSheet(id, info.versions, info.version, "");
    root.append(backdrop, sheet);
  }
}

// The artifacts index in the one shell: one row per artifact, the same shape
// as every other list, with the fixed glyph column so every title starts at
// the same x. The document itself is read in the stage.
// The artifacts index is grouped, and the group is a value the reader picks.
// Date and Kind both come from the listing; the writer is not on an artifact,
// so an Agent group would need backend data the list does not carry.
export const ARTIFACT_GROUPS = { day: "Date", kind: "Kind", agent: "Agent" };
export let artifactGrouping = "day";

export function setArtifactGrouping(group) {
  if (group in ARTIFACT_GROUPS) {
    artifactGrouping = group;
    render();
  }
}

const GROUP_CHEVRON = `<svg aria-hidden="true" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M 6 9l6 6 6-6"></path></svg>`;

export function artifactGroupMenu() {
  const item = (key, label) =>
    `<button type="button" role="menuitemradio" aria-checked="${artifactGrouping === key}" data-action="artifact-group" data-group="${key}">${label}</button>`;
  return `<div class="shell-group">
    <button type="button" class="hub-group-toggle" data-group-toggle aria-haspopup="menu" aria-expanded="false">Group${GROUP_CHEVRON}</button>
    <span class="pill shell-group-pill">${ARTIFACT_GROUPS[artifactGrouping]}</span>
    <div class="shell-group-menu" role="menu" aria-label="Group artifacts" data-group-menu hidden>
      ${item("day", "Date")}${item("kind", "Kind")}${item("agent", "Agent")}
    </div>
  </div>`;
}

// The label for the day an artifact was last written, in the reader's own
// calendar. The list is ordered by that timestamp, so grouping on it keeps one
// group per day and never repeats a label. Grouping on creation instead puts
// the same label in the list twice, because a well-used artifact is written
// long after it was created.
function artifactDay(iso) {
  const at = new Date(iso);
  const midnight = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((midnight(new Date()) - midnight(at)) / 86400000);
  if (days === 0) return "TODAY";
  if (days === 1) return "YESTERDAY";
  const month = at.toLocaleString(undefined, { month: "short" }).toUpperCase();
  return `${month} ${at.getDate()}, ${at.getFullYear()}`;
}

function artifactGroups(artifacts) {
  const groups = new Map();
  for (const a of artifacts) {
    let label;
    if (artifactGrouping === "agent") {
      label = (a.actor ? String(a.actor) : "Unattributed").toUpperCase();
    } else if (artifactGrouping === "kind") {
      label = String(a.kind || "other").toUpperCase();
    } else {
      label = artifactDay(a.updated_at || a.created_at);
    }
    const bucket = groups.get(label);
    if (bucket) bucket.push(a);
    else groups.set(label, [a]);
  }
  return [...groups].map(([label, items]) => ({ label, items }));
}

export async function artifactIndex(projectId, selectedId = "") {
  const { artifacts } = await api(`/api/v1/projects/${encodeURIComponent(projectId)}/artifacts`);
  if (!artifacts || !artifacts.length) {
    return { rows: `<div class="shell-body-pad"><p class="empty">No artifacts yet.</p></div>`, artifacts: [] };
  }
  // The newest artifact is what the stage shows when the reader has not chosen
  // one, so its row is the marked one from the first paint.
  const mark = selectedId || artifacts[0].id;
  const rows = artifactGroups(artifacts)
    .map((group) => {
      const body = group.items
        .map((a) => {
          const comments = a.comments_count
            ? ` · ${a.comments_count} ${a.comments_count === 1 ? "comment" : "comments"}`
            : "";
          const href = `#/projects/${encodeURIComponent(projectId)}/artifacts?artifact=${encodeURIComponent(a.id)}`;
          const lock = a.protected ? `<span class="sr-only">Encrypted</span>` : "";
          const selected = a.id === mark;
          return `<div class="row artifact-row${selected ? " selected" : ""}" data-id="${esc(a.id)}">
            <span class="row-glyph" style="background:none;border-radius:0;color:var(--ink-3);box-shadow:none;display:grid;place-items:center;" aria-hidden="true">${cardGlyph(a.protected)}</span>
            <div class="grow">
              <a class="title" href="${esc(href)}"${
                selected ? ' aria-current="true"' : ""
              }>${esc(a.title || "artifact")}${lock}</a>
              <div class="meta mono">v${a.version || 1} · ${formatBytes(a.size_bytes)}${comments}</div>
            </div>
          </div>`;
        })
        .join("");
      return `<h2 class="section-label shell-group-label">${esc(group.label || "")}${
        group.label ? " · " : ""
      }${group.items.length}</h2><div class="inbox-rows">${body}</div>`;
    })
    .join("");
  return { rows, artifacts };
}

// The group the reader picked, applied on the next paint.
if (typeof document !== "undefined") {
  document.addEventListener("click", (event) => {
    const button = event.target.closest?.('[data-action="artifact-group"]');
    if (!button) return;
    const group = button.dataset.group;
    if (group in ARTIFACT_GROUPS) {
      artifactGrouping = group;
      render();
    }
  });
}

// The version an agent is holding live, when the band has read one. The version
// sheet labels that row so a reader knows the text is still moving.
let liveVersion = null;

// The framed page cannot remember a theme (a sandboxed frame has no store),
// so the viewer names the one it wants in the address. Relative to this
// document (the app shell), not the origin root, so it still lands on the
// artifact page under whatever prefix a proxy mounts the app on.
function frameSrc(id, version, theme, pass) {
  const params = new URLSearchParams();
  if (version) params.set("version", String(version));
  if (theme) params.set("theme", theme);
  if (pass) params.set("pass", pass);
  const query = params.toString();
  const address = `artifacts/${encodeURIComponent(id)}`;
  return query ? `${address}?${query}` : address;
}

// A shared plain artifact's page is 404 for everyone but the token holder, and
// an iframe navigation carries no bearer token, so the frame asks for a pass
// before it names the page. The hub answers an empty pass for an artifact whose
// page is public anyway, so the address a reader ends up with carries a
// credential only where one is needed.
async function viewerPass(id) {
  try {
    const minted = await api(`/api/v1/artifacts/${encodeURIComponent(id)}/viewer-pass`);
    return minted?.pass || "";
  } catch {
    return "";
  }
}

async function fetchRawText(id, version) {
  const query = version ? `?version=${version}` : "";
  const headers = {};
  const token = localStorage.getItem("hub.token");
  if (token) headers.Authorization = "Bearer " + token;
  const res = await fetch(
    new URL(`api/v1/artifacts/${encodeURIComponent(id)}/raw${query}`, document.baseURI),
    { headers },
  );
  if (!res.ok) throw new Error("Failed to fetch raw text");
  const ct = res.headers.get("content-type") || "";
  if (ct.includes("application/json")) {
    const json = await res.json();
    return typeof json === "string" ? json : JSON.stringify(json, null, 2);
  }
  return await res.text();
}

export function toggleVersionMenu(button) {
  const menu = document.getElementById("hub-version-menu");
  const backdrop = document.getElementById("hub-version-backdrop");
  if (!menu) return;
  const open = menu.hidden;
  menu.hidden = !open;
  if (backdrop) backdrop.hidden = !open;
  if (button) button.setAttribute("aria-expanded", String(open));
  if (open) {
    const active = menu.querySelector('[aria-current="true"]');
    (active || menu.querySelector("button"))?.focus();
  }
}

// A version is read in the stage it was picked from. The pick replaces the
// address rather than adding to it, so Back still leaves the artifact.
export function pickVersion(id, version) {
  const parts = location.hash.replace(/^#/, "").split("?")[0].split("/");
  const project = parts[1] === "projects" ? decodeURIComponent(parts[2] || "") : "";
  location.replace(
    project
      ? artifactHref(project, id, version)
      : `#/artifacts/${encodeURIComponent(id)}?version=${encodeURIComponent(version)}`,
  );
}

// Version sheet (Screen 02): replaces inline version select
function buildVersionSheet(id, versions, shown, projectId) {
  const backdrop = document.createElement("div");
  backdrop.id = "hub-version-backdrop";
  backdrop.className = "hub-version-backdrop";
  backdrop.hidden = true;

  const sheet = document.createElement("div");
  sheet.id = "hub-version-menu";
  sheet.className = "hub-version-sheet hub-version-menu";
  sheet.hidden = true;
  sheet.setAttribute("role", "dialog");
  sheet.setAttribute("aria-modal", "true");
  sheet.setAttribute("aria-label", "Versions");

  const handle = document.createElement("div");
  handle.className = "hub-version-sheet-handle";

  const head = document.createElement("div");
  head.className = "hub-version-sheet-header";
  head.innerHTML = `
    <span class="hub-version-sheet-title">Versions</span>
    <span class="hub-version-sheet-count mono">${versions.length} · newest first</span>
  `;

  const list = document.createElement("div");
  list.className = "hub-version-sheet-list";

  for (const v of [...versions].reverse()) {
    const isCurrent = v.version === shown;
    const row = document.createElement("button");
    row.type = "button";
    row.className = `hub-version-row${isCurrent ? " current" : ""}`;
    row.dataset.action = "version-pick";
    row.dataset.id = id;
    row.dataset.version = String(v.version);
    if (isCurrent) row.setAttribute("aria-current", "true");

    const age = v.created_at ? relative(Date.parse(v.created_at)) : "";
    const actor = v.actor || "agent";
    const isLive = v.version === liveVersion;
    const primaryText = isLive ? "Being written" : isCurrent ? "Current" : actor;
    const secondaryText = isCurrent || isLive ? `${actor} · ${age}` : age;

    row.innerHTML = `
      <span class="hub-version-num mono">v${v.version}</span>
      <span class="hub-version-info">
        <span class="hub-version-primary">${esc(primaryText)}</span>
        <span class="hub-version-secondary">${esc(secondaryText)}</span>
      </span>
      <span class="hub-version-size mono">${formatBytes(v.size_bytes)}</span>
    `;
    list.appendChild(row);
  }

  const footer = document.createElement("div");
  footer.className = "hub-version-sheet-footer";
  footer.textContent = "Versions are whole saves. Nothing is compared - the hub keeps no diff.";

  sheet.append(handle, head, list, footer);

  const closeSheet = () => {
    backdrop.hidden = true;
    sheet.hidden = true;
    const toggle = main.querySelector(".hub-version-toggle");
    if (toggle) {
      toggle.setAttribute("aria-expanded", "false");
      toggle.focus();
    }
  };

  backdrop.addEventListener("click", closeSheet);
  sheet.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      closeSheet();
    }
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !sheet.hidden) {
      e.stopPropagation();
      closeSheet();
    }
  });

  return { backdrop, sheet };
}

// The share URL is long and its tail is the part that differs, so the middle is
// elided for display; the full value stays in the title and is what the copy
// control takes.
function middleTruncate(value, max = 38) {
  const text = String(value || "");
  if (text.length <= max) return text;
  const head = Math.ceil((max - 1) / 2);
  const tail = max - 1 - head;
  return `${text.slice(0, head)}…${text.slice(text.length - tail)}`;
}

// Share sheet: opens from overflow menu
function buildShareSheet(id, current, shown, moreBtn) {
  const backdrop = document.createElement("div");
  backdrop.className = "hub-share-backdrop";
  backdrop.hidden = true;

  const sheet = document.createElement("div");
  sheet.className = "hub-share-sheet";
  sheet.hidden = true;
  sheet.setAttribute("role", "dialog");
  sheet.setAttribute("aria-modal", "true");
  sheet.setAttribute("aria-label", "Share this artifact");

  const handle = document.createElement("div");
  handle.className = "hub-share-handle";

  const header = document.createElement("div");
  header.className = "hub-share-header";
  header.innerHTML = `
    <span class="hub-share-title">Share this artifact</span>
    <span class="hub-share-subline">${esc(current.title || "")} · <span class="mono">v${shown}</span></span>
  `;

  const bodyWrap = document.createElement("div");
  bodyWrap.className = "hub-share-body";

  const copySvg = glyphSvg("copy", { size: 16 });

  async function copyToClipboard(url) {
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(url);
      }
    } catch {}
    toast("Link copied");
  }

  async function renderContent() {
    bodyWrap.innerHTML = "";

    if (current.protected) {
      // Encrypted artifact: the link, then delete. Encryption was chosen when
      // the document was published, so the sheet cannot lock or unlock it.
      const shareUrl = new URL(`artifacts/${encodeURIComponent(id)}`, document.baseURI).href;

      const linkRow = document.createElement("div");
      linkRow.className = "hub-share-link-row";
      linkRow.innerHTML = `
        <span class="hub-share-url mono" title="${esc(shareUrl)}">${esc(middleTruncate(shareUrl))}</span>
        <button type="button" class="hub-share-copy-btn" aria-label="Copy share link" data-action="copy-link">
          ${copySvg}
        </button>
      `;
      linkRow.querySelector('button[data-action="copy-link"]').addEventListener("click", () => {
        copyToClipboard(shareUrl);
      });

      const helper = document.createElement("div");
      helper.className = "hub-share-helper";
      helper.textContent = "Whoever published this set its password. The hub cannot reset or recover it.";

      const actions = document.createElement("div");
      actions.className = "hub-share-actions";
      const delBtn = document.createElement("button");
      delBtn.type = "button";
      delBtn.className = "hub-share-primary is-danger";
      delBtn.textContent = "Delete artifact";
      delBtn.addEventListener("click", async () => {
        const ok = await confirmAction({
          title: `Delete ${current.title || "this artifact"}?`,
          body: "Every version and every link to it stop working. This cannot be undone.",
          safe: "Cancel",
          danger: "Delete artifact",
          tone: "danger",
        });
        if (ok) {
          try {
            await api(`/api/v1/artifacts/${encodeURIComponent(id)}`, { method: "DELETE" });
            toast("Artifact deleted");
            closeSheet();
            window.location.hash = `#/projects/${encodeURIComponent(current.project_id || "")}`;
          } catch {
            toast("Failed to delete artifact");
          }
        }
      });
      actions.appendChild(delBtn);

      bodyWrap.append(linkRow, helper, actions);
    } else {
      // Plain artifact: fetch share status
      let activeShare = null;
      try {
        activeShare = await api(`/api/v1/artifacts/${encodeURIComponent(id)}/share`);
      } catch {
        activeShare = null;
      }

      if (activeShare && activeShare.url) {
        const shareUrl = new URL(activeShare.url, document.baseURI).href;

        const linkRow = document.createElement("div");
        linkRow.className = "hub-share-link-row";
        linkRow.innerHTML = `
          <span class="hub-share-url mono" title="${esc(shareUrl)}">${esc(middleTruncate(shareUrl))}</span>
          <button type="button" class="hub-share-copy-btn" aria-label="Copy share link" data-action="copy-link">
            ${copySvg}
          </button>
        `;
        linkRow.querySelector('button[data-action="copy-link"]').addEventListener("click", () => {
          copyToClipboard(shareUrl);
        });

        const helper = document.createElement("div");
        helper.className = "hub-share-helper";
        helper.textContent = `This link opens version ${activeShare.version}. It is the only way in.`;

        const actions = document.createElement("div");
        actions.className = "hub-share-actions";
        const revokeBtn = document.createElement("button");
        revokeBtn.type = "button";
        revokeBtn.className = "hub-share-primary is-revoking";
        revokeBtn.textContent = "Revoke link";
        revokeBtn.addEventListener("click", async () => {
          const ok = await confirmAction({
            title: "Revoke this link?",
            body: "Anyone holding it loses access. Sharing again makes a new link.",
            safe: "Cancel",
            danger: "Revoke link",
            tone: "primary",
          });
          if (ok) {
            try {
              await api(`/api/v1/artifacts/${encodeURIComponent(id)}/share`, { method: "DELETE" });
              toast("Link revoked");
              await renderContent();
            } catch {
              toast("Failed to revoke link");
            }
          }
        });
        actions.appendChild(revokeBtn);

        bodyWrap.append(linkRow, helper, actions);
      } else {
        const helper = document.createElement("div");
        helper.className = "hub-share-helper";
        helper.textContent = "Sharing creates one link to this version.";

        const actions = document.createElement("div");
        actions.className = "hub-share-actions";
        const createBtn = document.createElement("button");
        createBtn.type = "button";
        createBtn.className = "hub-share-primary";
        createBtn.textContent = "Make link";
        createBtn.addEventListener("click", async () => {
          try {
            createBtn.disabled = true;
            await api(`/api/v1/artifacts/${encodeURIComponent(id)}/share`, {
              method: "POST",
              headers: { "Content-Type": "application/json" },
              body: JSON.stringify({ version: shown }),
            });
            toast("Link created");
            await renderContent();
          } catch {
            createBtn.disabled = false;
            toast("Failed to create link");
          }
        });
        actions.appendChild(createBtn);

        bodyWrap.append(helper, actions);
      }
    }
  }

  sheet.append(handle, header, bodyWrap);

  const closeSheet = () => {
    backdrop.hidden = true;
    sheet.hidden = true;
    if (moreBtn) {
      moreBtn.setAttribute("aria-expanded", "false");
      moreBtn.focus();
    }
  };

  const openSheet = async () => {
    backdrop.hidden = false;
    sheet.hidden = false;
    await renderContent();
    const btn = sheet.querySelector(".hub-share-primary, .hub-share-copy-btn");
    if (btn) btn.focus();
  };

  backdrop.addEventListener("click", closeSheet);
  sheet.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      closeSheet();
    }
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && !sheet.hidden) {
      e.stopPropagation();
      closeSheet();
    }
  });

  return { backdrop, sheet, openSheet, closeSheet };
}

// The address an artifact is read at: its project's Artifacts segment with it
// selected, and the version when the reader asked for one.
export function artifactHref(projectId, id, version = null) {
  const query = new URLSearchParams({ artifact: id });
  if (version) query.set("version", String(version));
  return `#/projects/${encodeURIComponent(projectId)}/artifacts?${query}`;
}

async function listsArtifact(projectId, id) {
  try {
    const { artifacts } = await api(`/api/v1/projects/${encodeURIComponent(projectId)}/artifacts`);
    return (artifacts || []).some((a) => a.id === id);
  } catch {
    return false;
  }
}

// The project an artifact belongs to, for a link that does not say. The hub's
// artifact routes do not name it, so each project's own list is asked.
async function projectOf(id) {
  const staged = stagedIn.get(id);
  if (staged && (await listsArtifact(staged, id))) return staged;
  const { projects } = await api("/api/v1/projects");
  for (const project of projects || []) {
    if (await listsArtifact(project.id, id)) return project.id;
  }
  return "";
}

// `#/artifacts/<id>` is the address a feed row, a search hit, a comment's
// "open v2" and an older saved link carry. An artifact has one surface, its
// project's Artifacts shell with it selected, so this address is replaced by
// that one rather than painted: Back then leaves the artifact instead of
// stepping onto a hop that forwards again.
export async function artifactRoute(params, gen, path) {
  const id = decodeURIComponent((path || "").split("/")[2] || "");
  if (!id) {
    location.replace("#/projects");
    return;
  }
  // An artifact the hub does not hold is said here, in the hub's own words,
  // rather than as an empty shell.
  try {
    await api(`/api/v1/artifacts/${encodeURIComponent(id)}/versions`);
  } catch (error) {
    paint(gen, `<h1>Artifact</h1><p class="error">${esc(error.message)}</p>`);
    return;
  }
  // A project the address names is taken only when its own list holds the
  // artifact, so a stale or mistyped name cannot open the wrong project.
  const named = params.get("project");
  const project = (named && (await listsArtifact(named, id)) ? named : "") || (await projectOf(id));
  if (stale(gen)) return;
  if (!project) {
    paint(gen, `<h1>Artifact</h1><p class="error">No project on this hub lists this artifact.</p>`);
    return;
  }
  location.replace(artifactHref(project, id, params.get("version")));
}
