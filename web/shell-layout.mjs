// The one shell: rail, index, stage, aside. Feed, artifacts, sessions and the
// inbox are the same four zones, so the pane widths, the splitter behaviour and
// the remembered widths live here rather than in each screen.
//
// The index and the aside are resizable; the stage is what is left and never
// goes under 560. A width is remembered per pane, per device. One clamp serves
// the pointer and the keyboard alike, because a rule that holds on a drag and
// not on an arrow key is not a rule.

import { esc } from "./dom.mjs";

const DEFAULTS = { index: 300, aside: 320 };
const MIN = { index: 260, aside: 288 };
const MAX = { index: 480, aside: 440 };
const STEP = 16;
const STEP_SHIFT = 48;
const STAGE_MIN = 560;
const RAIL = 200;
const SPLITTERS = 10; // two 5px dividers

const key = (pane) => `ah-w-${pane}`;

function readWidth(pane) {
  try {
    const stored = parseInt(localStorage.getItem(key(pane)), 10);
    return Number.isFinite(stored) ? stored : DEFAULTS[pane];
  } catch {
    return DEFAULTS[pane];
  }
}

function writeWidth(pane, width) {
  try {
    localStorage.setItem(key(pane), String(width));
  } catch {
    // A device that refuses storage keeps the width for this paint only. It
    // still moves; it just does not remember.
  }
}

// The widest the pane may be without taking the stage under its floor. The
// shell's own width is what is available, so a narrow window clamps earlier
// than a wide one.
function ceiling(pane, shellWidth, otherWidth) {
  const room = shellWidth - RAIL - SPLITTERS - STAGE_MIN - otherWidth;
  return Math.min(MAX[pane], Math.max(MIN[pane], room));
}

function clamp(pane, width, shellWidth, otherWidth) {
  return Math.max(MIN[pane], Math.min(ceiling(pane, shellWidth, otherWidth), Math.round(width)));
}

// The splitter's own description, for a reader who arrives on it with a
// keyboard. It says what it resizes and where it currently sits.
function describe(splitter, pane, width) {
  splitter.setAttribute("aria-valuenow", String(width));
  splitter.setAttribute("aria-valuemin", String(MIN[pane]));
  splitter.setAttribute("aria-valuemax", String(MAX[pane]));
}

function apply(shell, pane, width, other) {
  shell.style.setProperty(`--w-${pane}`, `${width}px`);
  const splitter = shell.querySelector(`[data-split="${pane}"]`);
  if (splitter) describe(splitter, pane, width);
  writeWidth(pane, width);
  void other;
}

// The other pane's width, which the clamp needs. A pane that is not on screen
// contributes nothing, so opening the aside does not shrink the index.
function otherWidth(shell, pane) {
  const other = pane === "index" ? "aside" : "index";
  const el = shell.querySelector(`.shell-${other}`);
  if (!el || el.hidden) return 0;
  return el.getBoundingClientRect().width;
}

export function installSplitters(shell) {
  if (!shell || shell.dataset.splitters === "on") return;
  shell.dataset.splitters = "on";

  for (const pane of ["index", "aside"]) {
    const splitter = shell.querySelector(`[data-split="${pane}"]`);
    if (!splitter) continue;
    const initial = readWidth(pane);
    shell.style.setProperty(`--w-${pane}`, `${initial}px`);
    describe(splitter, pane, initial);
  }

  let drag = null;

  shell.addEventListener("pointerdown", (event) => {
    const splitter = event.target.closest("[data-split]");
    if (!splitter) return;
    event.preventDefault();
    const pane = splitter.dataset.split;
    const rect = shell.getBoundingClientRect();
    drag = {
      pane,
      startX: event.clientX,
      startWidth: shell.querySelector(`.shell-${pane}`).getBoundingClientRect().width,
      shellWidth: rect.width,
    };
    splitter.setPointerCapture?.(event.pointerId);
    document.body.classList.add("is-resizing");
  });

  shell.addEventListener("pointermove", (event) => {
    if (!drag) return;
    const delta = drag.pane === "index" ? event.clientX - drag.startX : drag.startX - event.clientX;
    const width = clamp(drag.pane, drag.startWidth + delta, drag.shellWidth, otherWidth(shell, drag.pane));
    apply(shell, drag.pane, width);
  });

  const end = () => {
    if (!drag) return;
    drag = null;
    document.body.classList.remove("is-resizing");
  };
  shell.addEventListener("pointerup", end);
  shell.addEventListener("pointercancel", end);

  shell.addEventListener("dblclick", (event) => {
    const splitter = event.target.closest("[data-split]");
    if (!splitter) return;
    const pane = splitter.dataset.split;
    apply(shell, pane, DEFAULTS[pane]);
  });

  // The keyboard path is part of the drawing, not an extra: the separator is
  // focusable and the arrows move it by the same clamp the pointer uses.
  shell.addEventListener("keydown", (event) => {
    const splitter = event.target.closest("[data-split]");
    if (!splitter) return;
    const pane = splitter.dataset.split;
    const current = shell.querySelector(`.shell-${pane}`).getBoundingClientRect().width;
    const shellWidth = shell.getBoundingClientRect().width;
    let next = null;
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      const forward = event.key === "ArrowRight";
      const step = event.shiftKey ? STEP_SHIFT : STEP;
      const delta = (pane === "index" ? forward : !forward) ? step : -step;
      next = current + delta;
    } else if (event.key === "Home" || event.key === "Enter") {
      next = DEFAULTS[pane];
    }
    if (next == null) return;
    event.preventDefault();
    apply(shell, pane, clamp(pane, next, shellWidth, otherWidth(shell, pane)));
  });
}

// The aside toggle is absent, not disabled, where nothing is read against the
// stage. Screens that have no aside simply do not draw the control.
export function toggleAside(button) {
  const shell = button.closest(".shell");
  if (!shell) return;
  const aside = shell.querySelector(".shell-aside");
  if (!aside) return;
  const open = aside.hidden;
  aside.hidden = !open;
  button.setAttribute("aria-pressed", String(open));
}

const BACK_CHEVRON = `<svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M 15 6l-6 6 6 6"></path></svg>`;

// The reserved chrome is the rule, so the shell draws both rows whether or not
// a screen has anything for the second one. On a phone the shell shows one
// zone at a time: the index, or the stage with a way back to the list.
export function shellHTML({
  segment,
  indexHead,
  indexControls,
  indexBody,
  stageHead,
  stageControls,
  stageBody,
  aside = "",
  hasSelection = false,
  noIndex = false,
}) {
  // A screen with no index is rail and stage only: a short form is not a list
  // of items opened one at a time.
  if (noIndex) {
    return `<div class="shell shell-no-index"${segment ? ` data-segment="${esc(segment)}"` : ""}>
      <div class="shell-stage">
        ${stageHead}
        ${stageControls}
        <div class="shell-body">${stageBody}</div>
      </div>
    </div>`;
  }
  return `<div class="shell${hasSelection ? " has-selection" : ""}"${
    segment ? ` data-segment="${esc(segment)}"` : ""
  }>
    <div class="shell-index">
      ${indexHead}
      ${indexControls}
      <div class="shell-body" id="ah-index" role="tabpanel" tabindex="-1">${indexBody}</div>
    </div>
    <div class="shell-split" data-split="index" role="separator" aria-orientation="vertical" aria-label="Resize list" tabindex="0"></div>
    <div class="shell-stage">
      ${stageHead}
      ${stageControls}
      <div class="shell-body">${stageBody}</div>
    </div>
    <div class="shell-split" data-split="aside" role="separator" aria-orientation="vertical" aria-label="Resize panel" tabindex="0"></div>
    <div class="shell-aside"${aside ? "" : " hidden"}>${aside}</div>
  </div>`;
}

export function shellStageHead(title, meta, actions = "", backHref = "") {
  const leading = backHref
    ? `<a class="shell-leading" href="${esc(backHref)}" aria-label="Back to list">${BACK_CHEVRON}</a>`
    : "";
  return `<div class="shell-head">
    ${leading}
    <div class="shell-title">
      <h1 class="shell-title-line">${esc(title)}</h1>
      ${meta ? `<span class="shell-meta">${esc(meta)}</span>` : ""}
    </div>
    ${actions}
  </div>`;
}

export function shellIndexControls(placeholder) {
  return `<div class="shell-controls">
    <div class="shell-filter">
      <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="7"></circle><path d="M16 16l4 4"></path></svg>
      <input type="search" data-index-filter placeholder="${esc(placeholder)}" aria-label="${esc(placeholder)}">
    </div>
  </div>`;
}

// The index filter: a pill field that filters the list in place. It replaces
// the group headers with one mono count line while a query is live, and
// restores them when it is cleared. It never navigates.
export function installIndexFilter(shell) {
  const field = shell.querySelector("[data-index-filter]");
  const body = shell.querySelector(".shell-index .shell-body");
  if (!field || !body || field.dataset.wired === "on") return;
  field.dataset.wired = "on";

  const count = document.createElement("div");
  count.className = "shell-count";
  count.hidden = true;
  body.prepend(count);

  const rows = () => [...body.querySelectorAll(".row")];
  const apply = () => {
    const q = field.value.trim().toLowerCase();
    const all = rows();
    if (!q) {
      for (const row of all) row.hidden = false;
      for (const head of body.querySelectorAll(".day, .hub-group-header, .inbox-group")) head.hidden = false;
      count.hidden = true;
      return;
    }
    let shown = 0;
    for (const row of all) {
      const match = row.textContent.toLowerCase().includes(q);
      row.hidden = !match;
      if (match) shown++;
    }
    // A group header stays only while it still heads a visible row.
    for (const head of body.querySelectorAll(".day, .hub-group-header, .inbox-group")) {
      let node = head.nextElementSibling;
      let any = false;
      while (node && !node.matches(".day, .hub-group-header, .inbox-group")) {
        if (node.classList?.contains("row") && !node.hidden) { any = true; break; }
        if (node.querySelector?.(".row:not([hidden])")) { any = true; break; }
        node = node.nextElementSibling;
      }
      head.hidden = !any;
    }
    count.textContent = `${shown} of ${all.length} match "${field.value.trim()}"`;
    count.hidden = false;
  };
  field.addEventListener("input", apply);
}

export function installShellLayout(root = document) {
  for (const shell of root.querySelectorAll(".shell")) {
    installSplitters(shell);
    installIndexFilter(shell);
  }
}
