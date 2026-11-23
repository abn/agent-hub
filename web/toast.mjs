// The line at the foot of the screen that says what an action did, and offers
// the one way back from the actions that stay reversible for a short window.
//
// The region is created once and outlives every message: a live region only
// announces what is put into it after it exists, so a region built together
// with its first toast is a region nothing is heard from. One toast is on
// screen at a time; a new one replaces whatever is there, and the replaced
// message's undo is not offered again.

import { main } from "./dom.mjs";

const UNDO_SECONDS = 30;
const PLAIN_MS = 5000;
// How far down the toast has to be dragged before the release dismisses it.
const SWIPE_PX = 40;
// Spaced after each command letter, as in the composer's glyph.
const DISMISS_PATH = "M 6 6l12 12 M 18 6l-12 12";

const region = document.createElement("div");
region.className = "toast-region";
region.setAttribute("role", "status");
region.setAttribute("aria-live", "polite");
region.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  event.preventDefault();
  dismiss();
});
document.body.appendChild(region);

let current = null;

// The controls that sit at the foot of the screen and that a toast must not
// cover: the phone's tab bar and item thumb bar, and a desktop editor's action
// row. Only the ones drawn are measured.
const BOTTOM_CHROME = ".tabbar, .item-thumb, .wiki-editor-actions";

// Lift the region above whatever bottom chrome is drawn under where the toast
// will stand, so what the reader may press next is never behind it.
function clearControls() {
  const width = Math.min(420, window.innerWidth - 32);
  const left = (window.innerWidth - width) / 2;
  const right = left + width;
  let top = window.innerHeight;
  for (const control of document.querySelectorAll(BOTTOM_CHROME)) {
    const box = control.getBoundingClientRect();
    if (!box.width || !box.height || box.bottom <= window.innerHeight / 2) continue;
    if (box.right <= left || box.left >= right) continue;
    top = Math.min(top, box.top);
  }
  const lift = top < window.innerHeight ? `${Math.ceil(window.innerHeight - top)}px` : "";
  if (region.style.getPropertyValue("--toast-lift") === lift) return;
  if (lift) region.style.setProperty("--toast-lift", lift);
  else region.style.removeProperty("--toast-lift");
}

// The chrome under a toast changes while it stands: a sheet opens or closes,
// the window is resized, the route moves on. Re-measure on each, once per
// frame, and stop when the toast goes.
function watchControls() {
  let frame = 0;
  // Checked at each call: an observer can fire after the globals are gone.
  const hasFrames = () => typeof requestAnimationFrame === "function" && typeof cancelAnimationFrame === "function";
  const cancel = () => {
    if (hasFrames()) cancelAnimationFrame(frame);
    else if (typeof clearTimeout === "function") clearTimeout(frame);
  };
  const measure = () => {
    cancel();
    if (hasFrames()) frame = requestAnimationFrame(clearControls);
    else if (typeof setTimeout === "function") frame = setTimeout(clearControls, 16);
  };
  const mutated = new MutationObserver((records) => {
    if (records.some((record) => !region.contains(record.target))) measure();
  });
  mutated.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["class", "hidden", "style"] });
  const resized = typeof ResizeObserver === "function" ? new ResizeObserver(measure) : null;
  resized?.observe(document.documentElement);
  window.addEventListener("resize", measure);
  window.addEventListener("hashchange", measure);
  return () => {
    cancel();
    mutated.disconnect();
    resized?.disconnect();
    window.removeEventListener("resize", measure);
    window.removeEventListener("hashchange", measure);
  };
}

// A subject a toast names: the item's own words, cut to a line, in the same
// curly quotes the dialogs use.
export function quoted(summary, fallback = "") {
  const text = String(summary || fallback).replace(/\s+/g, " ").trim();
  const clamped = text.length > 48 ? `${text.slice(0, 47).trimEnd()}…` : text;
  return `\u201c${clamped}\u201d`;
}

function dismiss() {
  if (!current) return;
  current.unwatch();
  clearInterval(current.tick);
  clearTimeout(current.timer);
  const held = current.el.contains(document.activeElement);
  current.el.remove();
  current = null;
  // The reader was standing on a control that has just gone. Put them back on
  // the screen rather than at the end of the document.
  if (held) main.focus({ preventScroll: true });
}

// Whether the reader is composing something. A field, a picker and an editable
// region are all places a moved caret loses them their place.
function writing() {
  const el = /** @type {HTMLElement} */ (document.activeElement);
  if (!el) return false;
  if (el.isContentEditable) return true;
  return ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName);
}

function iconButton(label, path) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "toast-close";
  button.setAttribute("aria-label", label);
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", "16");
  svg.setAttribute("height", "16");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "1.8");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("aria-hidden", "true");
  const line = document.createElementNS("http://www.w3.org/2000/svg", "path");
  line.setAttribute("d", path);
  svg.appendChild(line);
  button.appendChild(svg);
  return button;
}

// Swipe down to dismiss, as the design asks for. The dismiss control beside it
// is the tap and keyboard equivalent, so nothing is reachable only by gesture,
// and a reader who asked for less motion gets no drag to follow.
function swipeToDismiss(el) {
  const still = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  let from = null;
  el.addEventListener("pointerdown", (event) => {
    if (event.target.closest("button")) return;
    from = event.clientY;
    el.setPointerCapture(event.pointerId);
  });
  el.addEventListener("pointermove", (event) => {
    if (from === null) return;
    const moved = Math.max(0, event.clientY - from);
    if (!still) el.style.transform = `translateY(${moved}px)`;
  });
  const release = (event) => {
    if (from === null) return;
    const moved = event.clientY - from;
    from = null;
    el.style.transform = "";
    if (moved > SWIPE_PX) dismiss();
  };
  el.addEventListener("pointerup", release);
  el.addEventListener("pointercancel", release);
}

// message: what happened, in the agent's voice. undo: the call that reverses
// it, when the action is reversible; it is what makes the toast last 30 s.
export function toast(message, undo) {
  dismiss();

  const el = document.createElement("div");
  el.className = "toast";
  const text = document.createElement("span");
  text.className = "toast-text";
  text.textContent = message;
  el.appendChild(text);

  let seconds = UNDO_SECONDS;
  let undoButton = null;
  if (undo) {
    undoButton = document.createElement("button");
    undoButton.type = "button";
    undoButton.className = "toast-undo";
    // The seconds change every second. Inside a live region that would be
    // announced every second, so this subtree is held out of it; a reader who
    // moves to the button still hears the count, because it is its own name.
    undoButton.setAttribute("aria-live", "off");
    undoButton.textContent = `Undo · ${seconds}s`;
    undoButton.addEventListener("click", () => {
      dismiss();
      undo();
    });
    el.appendChild(undoButton);
  }

  const close = iconButton("Dismiss", DISMISS_PATH);
  close.addEventListener("click", dismiss);
  el.appendChild(close);

  swipeToDismiss(el);
  region.appendChild(el);
  clearControls();
  current = { el, tick: 0, timer: 0, unwatch: watchControls() };

  if (undoButton) {
    current.tick = setInterval(() => {
      seconds -= 1;
      if (seconds <= 0) {
        dismiss();
        return;
      }
      undoButton.textContent = `Undo · ${seconds}s`;
    }, 1000);
    // Undo is the only way back from what just happened, and the control that
    // started it has been repainted away, so focus moves here rather than
    // leaving the reader to hunt for it at the end of the document. Not from
    // under someone who is writing: the caret is theirs, and the live region
    // announces both the message and the way back either way.
    if (!writing()) undoButton.focus({ preventScroll: true });
  } else {
    current.timer = setTimeout(dismiss, PLAIN_MS);
  }
}
