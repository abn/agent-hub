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
//
// A dialog may carry one optional text field, for words that go with the
// decision. Then the dialog holds something that is the reader's, and the
// rules the inbox card keeps apply here: Esc does not close over text, and a
// commit the hub refuses leaves the dialog open with the words in the field
// and the refusal beside it.

import { api } from "./api.mjs";
import { esc, main } from "./dom.mjs";

let sequence = 0;
let openDialog = null;

// title, body: sentences. list: mono lines naming what the action touches.
// note: the footer line, for the reversible path. safe: the label that keeps
// things as they are, focused first. danger: the label that commits. tone:
// `danger` for a destructive commit, `action` for a decision that is recorded.
// field: `{ label, limit }`, an optional note. commit: the work itself, given
// the trimmed note and run while the dialog is still open; what it throws is
// shown in the dialog, and the dialog closes as committed once it returns.
export function confirmAction({
  title,
  body,
  list = [],
  note = "",
  safe = "Keep",
  danger,
  tone = "danger",
  field = null,
  commit: work = null,
}) {
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

  // The count is how long the note is against the hub's limit, in the hub's
  // own measure: characters of the trimmed text. It shows from nine tenths of
  // the limit and says in words when the note is past it. The field carries no
  // `maxlength`, which would cut a pasted note short without saying so; the
  // hub refuses a note that is too long and decides nothing.
  let input = null;
  let count = null;
  let kept = null;
  if (field) {
    const wrap = document.createElement("div");
    wrap.className = "dialog-field-wrap";
    const label = document.createElement("label");
    label.className = "dialog-label";
    label.htmlFor = `dialog-field-${sequence}`;
    label.textContent = field.label;
    const optional = document.createElement("span");
    optional.className = "dialog-optional";
    optional.textContent = " (optional)";
    label.appendChild(optional);
    input = document.createElement("textarea");
    input.className = "dialog-field";
    input.id = label.htmlFor;
    input.rows = 3;
    count = document.createElement("p");
    count.className = "dialog-count";
    count.id = `dialog-count-${sequence}`;
    count.hidden = true;
    // Said as it changes: past the limit is otherwise silent until the commit.
    count.setAttribute("aria-live", "polite");
    kept = document.createElement("p");
    kept.className = "dialog-kept";
    kept.setAttribute("role", "status");
    wrap.append(label, input, count, kept);
    form.appendChild(wrap);
    input.addEventListener("input", () => {
      const length = [...input.value.trim()].length;
      count.hidden = length < field.limit * 0.9;
      count.textContent =
        `${length} of ${field.limit} characters` +
        (length > field.limit ? `, ${length - field.limit} over the limit` : "");
      kept.textContent = "";
      // A refusal was about the note as it was sent, so an edit withdraws it.
      if (problem) problem.hidden = true;
      input.removeAttribute("aria-invalid");
    });
  }

  // What a refused commit says, beside the field when there is one. It is in
  // the page before its text changes, so the refusal is announced.
  let problem = null;
  if (work) {
    problem = document.createElement("p");
    problem.className = "dialog-error";
    problem.id = `dialog-error-${sequence}`;
    problem.setAttribute("role", "alert");
    problem.hidden = true;
    (input ? input.parentNode : form).appendChild(problem);
    if (input) input.setAttribute("aria-describedby", `${count.id} ${problem.id}`);
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
    const ring = [...el.querySelectorAll("textarea, button:not([disabled])")];
    if (!ring.length) return;
    const edge = event.shiftKey ? ring[0] : ring[ring.length - 1];
    if (document.activeElement !== edge) return;
    event.preventDefault();
    (event.shiftKey ? ring[ring.length - 1] : ring[0]).focus();
  });

  // Esc is Keep, which would throw a written note away without a word, so
  // while the field holds text Esc keeps the dialog and says why. The key is
  // answered before the platform reads it as a close request, since a second
  // request in a row is one the platform does not let a page refuse; `cancel`
  // covers a close request that is not a key. Neither closes a dialog whose
  // commit is on its way.
  let busy = false;
  const holds = () => busy || !!(input && input.value.trim());
  const refuse = (event) => {
    if (!holds()) return;
    event.preventDefault();
    if (kept && !busy) kept.textContent = `Your note is kept. ${safe} closes without sending it.`;
  };
  el.addEventListener("keydown", (event) => {
    if (event.key === "Escape") refuse(event);
  });
  el.addEventListener("cancel", refuse);

  // With work to do, the commit button runs it here rather than closing. The
  // buttons stay enabled, so focus stays where it is, and are deaf meanwhile.
  if (work) {
    keep.addEventListener("click", (event) => {
      if (busy) event.preventDefault();
    });
    commit.addEventListener("click", async (event) => {
      event.preventDefault();
      if (busy) return;
      busy = true;
      form.setAttribute("aria-busy", "true");
      problem.hidden = true;
      if (input) input.removeAttribute("aria-invalid");
      try {
        await work(input ? input.value.trim() : "");
      } catch (error) {
        problem.textContent = error.message;
        problem.hidden = false;
        if (input) {
          input.setAttribute("aria-invalid", "true");
          input.focus();
        }
        return;
      } finally {
        busy = false;
        form.removeAttribute("aria-busy");
      }
      el.close("commit");
    });
  }

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
        if (opener instanceof HTMLElement && opener.isConnected) {
          opener.focus({ preventScroll: true });
        } else if (!document.activeElement || document.activeElement === document.body) {
          main.focus({ preventScroll: true });
        }
        resolve(committed);
      },
      { once: true },
    );
  });
}

export function slugify(name) {
  return String(name || "")
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export function suggestSlug(slug) {
  const match = String(slug || "").match(/^(.*?)-(\d+)$/);
  if (match) {
    const base = match[1];
    const num = parseInt(match[2], 10) + 1;
    return `${base}-${num}`;
  }
  return `${slug || "project"}-2`;
}

export function openCreateProjectDialog({ onCreated } = {}) {
  if (openDialog || document.querySelector("dialog[open]")) return Promise.resolve(null);

  const opener = document.activeElement;
  const dialog = document.createElement("dialog");
  dialog.className = "project-create-dialog";
  sequence += 1;
  const titleId = `project-create-title-${sequence}`;
  dialog.setAttribute("aria-labelledby", titleId);

  dialog.innerHTML = `
    <form class="project-create-form" method="dialog">
      <div class="project-create-handle" aria-hidden="true"></div>
      <div class="project-create-head">
        <h2 id="${titleId}" class="project-create-title">New project</h2>
        <button type="button" class="project-create-close" aria-label="Cancel" data-action="cancel">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"></path></svg>
        </button>
      </div>
      <div class="project-create-body">
        <label for="project-create-name" class="project-create-label">Name</label>
        <input id="project-create-name" class="project-create-name" name="name" type="text" autocomplete="off" required placeholder="">
        <div class="project-create-slug-row">
          <span class="project-create-slug-path mono">
            <span class="project-create-slug-prefix">/p/</span><span class="project-create-slug-val"></span>
          </span>
          <input id="project-create-slug-input" class="project-create-slug-input mono" name="slug" type="text" autocomplete="off" aria-label="Project slug" style="display: none;">
          <span class="project-create-taken-text mono" style="display: none;"></span>
          <button type="button" class="project-create-edit-btn" data-action="edit-slug">Edit</button>
        </div>
        <div class="project-create-slug-hint">The address agents use. Lower case, numbers and hyphens; it cannot change once an agent has written to the project.</div>
        <p class="project-create-error" role="alert" style="display: none;"></p>
      </div>
      <div class="project-create-foot">
        <div class="project-create-actions">
          <button type="button" class="project-create-cancel" data-action="cancel">Cancel</button>
          <button type="submit" class="project-create-submit">Create project</button>
        </div>
        <div class="project-create-subline">Opens the project. Nothing is shared until you share an artifact.</div>
      </div>
    </form>
  `;

  const form = dialog.querySelector("form");
  const nameInput = dialog.querySelector("#project-create-name");
  const slugRow = dialog.querySelector(".project-create-slug-row");
  const slugPath = dialog.querySelector(".project-create-slug-path");
  const slugVal = dialog.querySelector(".project-create-slug-val");
  const slugInput = dialog.querySelector("#project-create-slug-input");
  const takenText = dialog.querySelector(".project-create-taken-text");
  const editBtn = dialog.querySelector(".project-create-edit-btn");
  const errorEl = dialog.querySelector(".project-create-error");
  const submitBtn = dialog.querySelector(".project-create-submit");

  let slugEdited = false;
  let currentSlug = "";
  let takenSuggestion = "";

  const renderSlug = () => {
    errorEl.style.display = "none";
    if (takenSuggestion) {
      slugRow.classList.add("taken");
      slugPath.style.display = "none";
      slugInput.style.display = "none";
      editBtn.style.display = "none";
      takenText.style.display = "inline";
      takenText.innerHTML = `${esc(currentSlug)} is taken - try <button type="button" class="project-create-suggest-btn" data-action="use-suggest">${esc(takenSuggestion)}</button>`;
    } else {
      slugRow.classList.remove("taken");
      takenText.style.display = "none";
      if (slugEdited) {
        slugPath.style.display = "none";
        slugInput.style.display = "inline-block";
        editBtn.style.display = "none";
        slugInput.value = currentSlug;
      } else {
        slugPath.style.display = "inline";
        slugInput.style.display = "none";
        editBtn.style.display = "inline-flex";
        slugVal.textContent = currentSlug;
      }
    }
  };

  nameInput.addEventListener("input", () => {
    if (!slugEdited) {
      currentSlug = slugify(nameInput.value);
    }
    takenSuggestion = "";
    renderSlug();
  });

  editBtn.addEventListener("click", (e) => {
    e.preventDefault();
    slugEdited = true;
    takenSuggestion = "";
    renderSlug();
    slugInput.focus();
    slugInput.select();
  });

  slugInput.addEventListener("input", () => {
    slugEdited = true;
    currentSlug = slugify(slugInput.value);
    takenSuggestion = "";
  });

  slugRow.addEventListener("click", (e) => {
    const suggestBtn = e.target.closest?.('[data-action="use-suggest"], .project-create-suggest-btn');
    if (suggestBtn && takenSuggestion) {
      e.preventDefault();
      currentSlug = takenSuggestion;
      slugInput.value = currentSlug;
      takenSuggestion = "";
      renderSlug();
      if (slugEdited) {
        slugInput.focus();
      }
    }
  });

  const closeDialog = () => {
    dialog.close("cancel");
  };

  dialog.querySelectorAll('[data-action="cancel"]').forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.preventDefault();
      closeDialog();
    });
  });

  let busy = false;
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (busy) return;

    if (takenSuggestion) {
      currentSlug = takenSuggestion;
      slugInput.value = currentSlug;
      takenSuggestion = "";
      renderSlug();
    }

    const name = nameInput.value.trim();
    const slug = (slugEdited ? slugInput.value : currentSlug).trim();
    if (!name || !slug) return;

    busy = true;
    form.setAttribute("aria-busy", "true");
    errorEl.style.display = "none";
    try {
      const created = await api("/api/v1/projects", {
        method: "POST",
        body: JSON.stringify({ id: slug, display_name: name }),
      });
      busy = false;
      dialog.close("created");
      if (onCreated) onCreated(created);
      location.hash = `#/projects/${encodeURIComponent(slug)}/feed`;
    } catch (err) {
      busy = false;
      form.removeAttribute("aria-busy");
      const isConflict = err.status === 409 || String(err.message || "").toLowerCase().includes("already exists");
      if (isConflict) {
        takenSuggestion = suggestSlug(slug);
        renderSlug();
      } else {
        errorEl.textContent = err.message || "Failed to create project";
        errorEl.style.display = "block";
      }
    }
  });

  dialog.addEventListener("keydown", (e) => {
    if (e.key === "Tab") {
      const ring = [...dialog.querySelectorAll('input:not([style*="display: none"]), button:not([style*="display: none"]):not([disabled])')];
      if (!ring.length) return;
      const edge = e.shiftKey ? ring[0] : ring[ring.length - 1];
      if (document.activeElement !== edge) return;
      e.preventDefault();
      (e.shiftKey ? ring[ring.length - 1] : ring[0]).focus();
    }
  });

  document.body.appendChild(dialog);
  openDialog = dialog;
  document.documentElement.classList.add("has-dialog");
  dialog.showModal();
  nameInput.focus();

  return new Promise((resolve) => {
    dialog.addEventListener(
      "close",
      () => {
        openDialog = null;
        document.documentElement.classList.remove("has-dialog");
        dialog.remove();
        if (opener instanceof HTMLElement && opener.isConnected) {
          opener.focus({ preventScroll: true });
        } else if (!document.activeElement || document.activeElement === document.body) {
          main.focus({ preventScroll: true });
        }
        resolve(dialog.returnValue === "created");
      },
      { once: true },
    );
  });
}
