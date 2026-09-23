// One keyboard map for the whole app: a search key, a roving selection through
// the rows of the screen you are on, and the two verbs the inbox owns. It
// reads the markup the screens already paint rather than asking them to
// describe themselves, so a screen joins the map by having rows.

import { main } from "./dom.mjs";
import { prefs } from "./prefs.mjs";

// What a key does to a row, found in the markup the screens already paint: the
// row's own link is what Enter follows, and the two verbs are the buttons the
// delegated click handler already owns.
const BEHAVIOUR = {
  open: (row) => row.querySelector("a[href]"),
  approve: (row) => row.querySelector('[data-action="approve"]'),
  reply: (row) => row.querySelector('[data-action="answer"]'),
};

// The screens the selection moves through. The other screens are fields and
// cards rather than a list, so a selection there would have nothing to mean.
const screens = {
  inbox: { rows: ".row" },
  feed: { rows: ".row" },
  sessions: { rows: ".row" },
  search: { rows: ".row" },
};

// The list of shortcuts lives here rather than in Settings: the map is the
// thing that knows them, the panel opens from any screen, and Settings belongs
// to the screen that owns it.
const SHORTCUTS = [
  ["/", "Focus the list's filter, or search"],
  ["j", "Next row"],
  ["k", "Previous row"],
  ["Enter", "Open the selected row"],
  ["a", "Approve the selected row"],
  ["r", "Reply on the selected row"],
  ["c", "Toggle the comments aside"],
  ["Esc", "Close what is on top"],
  ["?", "This list"],
];
// The panel is where the keys are discovered, so it is also where the reader
// is told they are theirs to switch off.
const OFF_SWITCH = "Turn these off under Single-key shortcuts in Settings. Esc and Tab stay.";

// The screen the selection belongs to, and which of its rows is selected.
let current = "";
let selected = "";
// Whether the reader's own focus is on the selection, so a repaint knows
// whether to take focus back or only to remember where it was.
let following = false;
let wantSearch = false;
let installed = false;
let panel = null;
const panes = [];

// A screen that wants rows the default selector does not find registers its
// own entry. Anything else about the map stays the map's.
export function registerScreen(name, config) {
  screens[name] = { rows: ".row", ...config };
}

// What Esc closes. The newest registered pane is the one on top; the returned
// function takes it back off.
export function registerPane(close) {
  panes.push(close);
  return () => {
    const at = panes.indexOf(close);
    if (at >= 0) panes.splice(at, 1);
  };
}

function screenName() {
  return location.hash.replace(/^#/, "").split("?")[0].split("/")[1] || "home";
}

function config() {
  return screens[screenName()] || null;
}

function rows() {
  const set = config();
  return set ? [...main.querySelectorAll(set.rows)] : [];
}

// What makes a row the same row after a repaint. The id an action button
// carries is the truest one; a row without an action falls back to its title.
function identify(row, index) {
  return (
    row.querySelector("[data-id]")?.dataset.id ||
    row.querySelector(".title")?.textContent ||
    `#${index}`
  );
}

function chosen() {
  return rows().find((row) => row.tabIndex === 0) || null;
}

// A real roving tabindex: the selected row is the one the tab order reaches,
// and moving the selection moves focus, so the ring shows it and a screen
// reader follows rather than being told about it.
function place(list, index, focus) {
  list.forEach((row, at) => {
    row.tabIndex = at === index ? 0 : -1;
  });
  current = screenName();
  selected = identify(list[index], index);
  if (!focus) return;
  following = true;
  list[index].focus();
}

function move(step) {
  const list = rows();
  if (!list.length) return;
  const at = list.findIndex((row) => row.tabIndex === 0);
  const next =
    at < 0
      ? step > 0
        ? 0
        : list.length - 1
      : Math.min(list.length - 1, Math.max(0, at + step));
  place(list, next, true);
}

function focusSearch(event) {
  event.preventDefault();
  // The design's `/` goes to the list's own filter field when the screen has
  // one, and only falls through to the Search screen when it does not.
  const filter = document.querySelector("[data-index-filter]");
  if (filter) {
    filter.focus();
    filter.select?.();
    return;
  }
  const field = document.getElementById("q");
  if (field) {
    field.focus();
    field.select?.();
    return;
  }
  wantSearch = true;
  location.hash = "#/search";
}

function openRow(event) {
  // A control handles its own Enter. Only a row that has focus as a row is the
  // map's to open.
  if (event.target.closest?.('a[href], button, [role="button"], summary')) return;
  const row = chosen();
  const target = row && BEHAVIOUR.open(row);
  if (!target) return;
  event.preventDefault();
  target.click();
}

function act(verb, event) {
  const row = chosen();
  const target = row && BEHAVIOUR[verb](row);
  if (!target) return;
  event.preventDefault();
  target.click();
}

// `c` toggles the aside, which is the stage's own comment control. A screen
// with no such control has no aside to toggle, so the key does nothing there.
function toggleAsideKey(event) {
  const control = document.querySelector('.shell-stage [data-action="comments-toggle"]');
  if (!control) return;
  event.preventDefault();
  control.click();
}

function buildPanel() {
  const dialog = document.createElement("dialog");
  dialog.className = "keymap";
  dialog.setAttribute("aria-labelledby", "keymap-title");
  const title = document.createElement("h2");
  title.id = "keymap-title";
  title.textContent = "Keyboard";
  const list = document.createElement("dl");
  for (const [key, what] of SHORTCUTS) {
    const term = document.createElement("dt");
    const mark = document.createElement("kbd");
    mark.textContent = key;
    term.appendChild(mark);
    const said = document.createElement("dd");
    said.textContent = what;
    list.append(term, said);
  }
  const note = document.createElement("p");
  note.className = "keymap-note";
  note.textContent = OFF_SWITCH;
  const close = document.createElement("button");
  close.type = "button";
  close.textContent = "Close";
  close.addEventListener("click", () => dialog.close());
  dialog.append(title, list, note, close);
  document.body.appendChild(dialog);
  return dialog;
}

// A native dialog, so the focus trap and Esc are the browser's rather than
// this module's, and nothing here waits on the app's own dialog.
function help(event) {
  event.preventDefault();
  if (!panel) panel = buildPanel();
  if (!panel.open) panel.showModal();
}

// Esc in a field that holds text is the field's: closing what is under a
// half-written answer would throw the answer away.
function closeTop(event) {
  if (document.querySelector("dialog[open]")) return;
  if (event.target.matches?.("textarea, input:not([type=checkbox]):not([type=radio])") && event.target.value) return;
  const close = panes[panes.length - 1];
  if (!close) return;
  event.preventDefault();
  close();
}

function typing(target) {
  if (!target) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
}

// Never when the reader has turned them off, never while they are writing,
// never as half of a browser or system shortcut, and never while a dialog owns
// the keyboard. An open dialog is the one thing that holds the map, asked of
// the document rather than told to the map, so every dialog holds it: the
// app's own, this module's shortcut panel, and any the browser puts in the top
// layer.
function blocked(event) {
  return (
    prefs.shortcuts !== "on" ||
    event.altKey ||
    event.ctrlKey ||
    event.metaKey ||
    typing(event.target) ||
    !!document.querySelector("dialog[open]") ||
    !!event.target.closest?.('[aria-modal="true"], [role="dialog"]')
  );
}

function onKey(event) {
  if (event.key === "Escape") {
    closeTop(event);
    return;
  }
  if (blocked(event)) return;
  if (event.key === "/") focusSearch(event);
  else if (event.key === "j") {
    event.preventDefault();
    move(1);
  } else if (event.key === "k") {
    event.preventDefault();
    move(-1);
  } else if (event.key === "Enter") openRow(event);
  else if (event.key === "c") toggleAsideKey(event);
  else if (event.key === "a") act("approve", event);
  else if (event.key === "r") act("reply", event);
  else if (event.key === "?") help(event);
}

// A screen repaints as one innerHTML write, which throws the selected element
// away. The selection is remembered by what the row is rather than by where it
// sat, so a refetch of the same screen leaves the reader where they were.
function repainted() {
  if (wantSearch) {
    const field = document.getElementById("q");
    if (field) {
      wantSearch = false;
      requestAnimationFrame(() => field.focus());
    }
  }
  const name = screenName();
  if (name !== current) {
    current = name;
    selected = "";
    following = false;
  }
  if (!screens[name]) return;
  const list = rows();
  if (!list.length) return;
  // A painted list is one stop in the tab ring, on its first row, so a reader
  // who has never pressed `j` can still walk into it. It takes no focus: it is
  // where the ring enters the list, not a selection the reader made.
  if (!selected) {
    place(list, 0, false);
    return;
  }
  const at = list.findIndex((row, index) => identify(row, index) === selected);
  if (at < 0) {
    // The remembered row is not in this list (another project's, or pruned),
    // so the list gets its way in back rather than none at all.
    selected = "";
    following = false;
    place(list, 0, false);
    return;
  }
  const take = following;
  place(list, at, false);
  // The router parks focus on the region after every paint, so taking it back
  // waits for the frame that follows rather than racing it.
  if (take) requestAnimationFrame(() => list[at].focus({ preventScroll: true }));
}

function onFocusIn(event) {
  if (event.target === main) return;
  const set = config();
  if (!set) return;
  const row = event.target.closest?.(set.rows);
  if (!row) {
    following = false;
    return;
  }
  following = true;
  const list = rows();
  const index = list.indexOf(row);
  if (index >= 0) {
    place(list, index, false);
  }
}

export function installKeys() {
  if (installed) return;
  installed = true;
  document.addEventListener("keydown", onKey);
  main.addEventListener("focusin", onFocusIn);
  new MutationObserver(repainted).observe(main, { childList: true, subtree: true });
}
