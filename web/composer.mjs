// The one control a reply or a comment is written in. Every screen that takes
// words from the reader builds this and nothing else: a question's card and the
// row it answers, an artifact's comments and their replies, and a wiki page's
// comments.
//
// It is multi-line, it keeps what was typed when the send fails, and it says
// why in the component rather than in a modal the reader has to dismiss before
// they can see their own words again. Screens build one, put it where the
// words belong, and hand it the call that sends.

import { glyphSvg } from "./glyphs.mjs";

let sequence = 0;

// The field grows a line at a time to this many lines and then scrolls, so a
// long answer stays in view without pushing the send control off a phone's
// screen above its keyboard.
export const MAX_LINES = 8;

// One keyboard rule everywhere. Enter is a line break, as it is in any other
// multi-line field and on a phone's own keyboard, and the platform's command
// key with Enter sends. A reply is a message in a mailbox, not a chat line, so
// the rule a reader learns on one composer holds on all of them.
const APPLE = /Mac|iPhone|iPad/.test(globalThis.navigator?.platform || "");
export const SEND_KEYS = APPLE ? "Cmd+Enter" : "Ctrl+Enter";
const TOUCH = () => Boolean(globalThis.matchMedia?.("(pointer: coarse)").matches);

// label: the field's accessible name, for a reader who cannot see what it sits
// under. placeholder: the resting line, which reads as the other way to answer
// when there are quick answers above it. action: the send control's name.
// empty: why the send control does nothing yet, shown while the field is
// blank. options: the answers the asker suggested, each sent as the body when
// pressed. maxLength: the hub's own limit, when it has one. send: what to call
// with the body; it rejects with the message the reader needs to see.
export function composer({
  label,
  options = [],
  placeholder = options.length ? "Or type a reply" : "Write a reply",
  action = "Send",
  empty = "Write a reply to send it.",
  maxLength = 0,
  send,
}) {
  sequence += 1;
  const hintId = `composer-hint-${sequence}`;
  const errorId = `composer-error-${sequence}`;

  const form = document.createElement("form");
  form.className = "composer";
  form.noValidate = true;

  const field = document.createElement("textarea");
  field.className = "composer-field";
  field.name = "body";
  field.rows = 1;
  field.placeholder = placeholder;
  if (maxLength) field.maxLength = maxLength;
  field.setAttribute("aria-label", label);
  field.setAttribute("aria-describedby", `${hintId} ${errorId}`);

  const button = document.createElement("button");
  button.type = "submit";
  // The primary fill, so the send keeps its ink on hover like every primary
  // button rather than taking an outline button's hover surface.
  button.className = "primary composer-send";
  button.setAttribute("aria-label", action);
  button.setAttribute("aria-describedby", hintId);
  button.setAttribute("aria-keyshortcuts", APPLE ? "Meta+Enter" : "Control+Enter");
  button.innerHTML = glyphSvg("send", { size: 18, strokeWidth: 1.8 });

  // Why the send control is dimmed, and once there is something to send, the
  // key that sends it. One line that is always there, so the field does not
  // move when it changes.
  const hint = document.createElement("p");
  hint.className = "composer-hint";
  hint.id = hintId;

  const error = document.createElement("p");
  error.className = "composer-error";
  error.id = errorId;
  error.setAttribute("role", "alert");
  error.hidden = true;

  const row = document.createElement("div");
  row.className = "composer-row";
  row.append(field, button);

  // A suggested answer is sent as it reads, through the same send as a typed
  // one, so it is refused, retried and reported the same way.
  const picks = options.map((option) => {
    const pick = document.createElement("button");
    pick.type = "button";
    pick.className = "composer-option";
    pick.textContent = option;
    pick.addEventListener("click", () => submit(option, pick));
    return pick;
  });
  if (picks.length) {
    const group = document.createElement("div");
    group.className = "composer-options";
    group.setAttribute("role", "group");
    group.setAttribute("aria-label", "Quick answers");
    group.append(...picks);
    form.append(group);
  }
  form.append(row, hint, error);

  // The field's height is its row count, so the stylesheet owns every length
  // and the script only counts lines. Counted from one row: counting from the
  // current height only ever grows, and deleting a paragraph would leave the
  // field the size the paragraph made it.
  function fit() {
    field.rows = 1;
    const style = getComputedStyle(field);
    const line = parseFloat(style.lineHeight);
    if (!line) return;
    const padding = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
    const lines = Math.round((field.scrollHeight - padding) / line);
    field.rows = Math.max(1, Math.min(MAX_LINES, lines));
  }

  function sync() {
    const blank = !field.value.trim();
    button.setAttribute("aria-disabled", String(blank));
    hint.dataset.state = blank ? "empty" : "keys";
    // A touch keyboard has no key to name, so under a coarse pointer the line
    // stays, empty, and the send control is described by nothing.
    const keys = TOUCH() ? "" : `${SEND_KEYS} to ${action.toLowerCase()}`;
    hint.textContent = blank ? empty : keys;
    fit();
  }

  function fail(message, target = field) {
    error.hidden = false;
    error.textContent = message;
    target.focus();
  }

  // The one send on its way. A question is answered once, so nothing else is
  // sent until the hub has said yes or no to it. The field is held read-only
  // rather than disabled, so it keeps focus and a phone keeps its keyboard up.
  let sending = false;

  function busy(state) {
    sending = state;
    field.readOnly = state;
    button.disabled = state;
    for (const pick of picks) pick.disabled = state;
    form.setAttribute("aria-busy", String(state));
  }

  async function submit(body, from = field) {
    if (sending) return;
    error.hidden = true;
    error.textContent = "";
    busy(true);
    try {
      await send(body);
    } catch (problem) {
      // What was typed stays in the field: it is the reader's, and the send is
      // the only thing that failed.
      busy(false);
      fail(`Nothing was sent: ${problem.message}`, from);
      return;
    }
    busy(false);
    field.value = "";
    sync();
    // A composer that outlives its send, under a list that stays open, keeps
    // the reader where they were writing.
    if (form.isConnected) field.focus();
  }

  field.addEventListener("input", sync);

  field.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.isComposing) return;
    if (!(event.ctrlKey || event.metaKey)) return;
    event.preventDefault();
    form.requestSubmit();
  });

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (sending) return;
    const body = field.value.trim();
    // An empty send is not an error: the line under the field already says
    // what is missing, so the press only puts the reader where to write it.
    if (!body) {
      field.focus();
      return;
    }
    await submit(body);
  });

  sync();
  // A field can be counted only once it is laid out; until then it is one row.
  globalThis.requestAnimationFrame?.(fit);

  return { element: form, field, focus: () => field.focus() };
}
