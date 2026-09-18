// The modal a decision is asked through, before the action that cannot be
// taken back runs.
//
// It is the platform's own `<dialog>`: `showModal` puts it in the top layer,
// makes the rest of the document inert, traps the tab ring inside it and
// answers Esc, so the parts an ARIA dialog would have to reimplement are the
// browser's. What is left to hold here is the page scroll behind it, the safe
// action taking focus first, and focus going back to whatever opened it.
//
// Every string a caller passes is set with textContent, so an agent's text
// cannot become markup on the way in.

import { main } from "./dom.mjs";

let sequence = 0;
let openDialog = null;

// title, body: sentences. list: mono lines naming what the action touches.
// note: the footer line, for the reversible path. safe: the label that keeps
// things as they are, focused first. danger: the label that commits. tone:
// `danger` for a destructive commit, `action` for a decision that is recorded.
export function confirmAction({ title, body, list = [], note = "", safe = "Keep", danger, tone = "danger" }) {
  // Two dialogs at once would fight over the top layer and over which control
  // focus goes back to. The second ask keeps.
  if (openDialog) return Promise.resolve(false);

  const opener = document.activeElement;
  const el = document.createElement("dialog");
  el.className = "dialog";
  sequence += 1;
  const titleId = `dialog-title-${sequence}`;
  el.setAttribute("aria-labelledby", titleId);

  const form = document.createElement("form");
  form.method = "dialog";
  form.className = "dialog-form";

  const heading = document.createElement("h2");
  heading.className = "dialog-title";
  heading.id = titleId;
  heading.textContent = title;
  form.appendChild(heading);

  if (body) {
    const text = document.createElement("p");
    text.className = "dialog-body";
    text.textContent = body;
    form.appendChild(text);
  }

  if (list.length) {
    const items = document.createElement("ul");
    items.className = "dialog-list";
    for (const line of list) {
      const item = document.createElement("li");
      item.textContent = line;
      items.appendChild(item);
    }
    form.appendChild(items);
  }

  const actions = document.createElement("div");
  actions.className = "dialog-actions";
  const keep = document.createElement("button");
  keep.type = "submit";
  keep.className = "dialog-safe";
  keep.value = "keep";
  keep.textContent = safe;
  const commit = document.createElement("button");
  commit.type = "submit";
  commit.className = `dialog-commit ${tone}`;
  commit.value = "commit";
  commit.textContent = danger;
  actions.append(keep, commit);
  form.appendChild(actions);

  if (note) {
    const footer = document.createElement("p");
    footer.className = "dialog-note";
    footer.textContent = note;
    form.appendChild(footer);
  }

  // The page behind a modal is inert either way, but Chromium's own ring runs
  // through the document between the last control and the first, which reads
  // as focus having left. This keeps the ring on the dialog's own controls.
  el.addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    const ring = [...el.querySelectorAll("button:not([disabled])")];
    if (!ring.length) return;
    const edge = event.shiftKey ? ring[0] : ring[ring.length - 1];
    if (document.activeElement !== edge) return;
    event.preventDefault();
    (event.shiftKey ? ring[ring.length - 1] : ring[0]).focus();
  });

  el.appendChild(form);
  document.body.appendChild(el);
  openDialog = el;
  // The document behind a modal must not scroll under it; the top layer stops
  // pointer and focus reaching it but not the wheel.
  document.documentElement.classList.add("has-dialog");
  el.showModal();
  keep.focus();

  return new Promise((resolve) => {
    el.addEventListener(
      "close",
      () => {
        openDialog = null;
        document.documentElement.classList.remove("has-dialog");
        el.remove();
        // Esc and the backdrop leave the return value empty, which is Keep.
        const committed = el.returnValue === "commit";
        const back = opener instanceof HTMLElement && opener.isConnected ? opener : main;
        back.focus({ preventScroll: true });
        resolve(committed);
      },
      { once: true },
    );
  });
}
