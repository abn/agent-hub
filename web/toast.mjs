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

function dismiss() {
  if (!current) return;
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
  const el = document.activeElement;
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
  current = { el, tick: 0, timer: 0 };

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
