// Project settings: the name, the slug it cannot change, the artifact password
// policy, the reserved retention card, and the way to delete the project.
//
// The markup is painted from literals and every value the hub or the reader
// supplied is placed afterwards through `value` and `textContent`, so nothing
// a project is called can become markup.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, main, paint, stale } from "./dom.mjs";
import { emptyStateHTML } from "./empty.mjs";
import { deleteProject } from "./settings.mjs";
import { toast } from "./toast.mjs";

// The hub's own policy values, in its order, under the design's copy.
const POLICIES = [
  ["off", "Off", "Artifacts stored in plain text on this server"],
  ["optional", "Optional", "Agents may encrypt per artifact"],
  ["required", "Required", "All artifacts E2EE · decrypted on your device"],
];

const NOT_FOUND = {
  screen: "project settings",
  title: "No project called {project}.",
  body: "It may have been deleted, or the address may be mistyped.",
  link: "All projects",
};

const GEAR =
  "M 12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z M 19 12a7 7 0 0 0-.2-1.6l2-1.5-2-3.4-2.3 1a7 7 0 0 0-2.8-1.6L13.3 2h-2.6l-.4 2.9a7 7 0 0 0-2.8 1.6l-2.3-1-2 3.4 2 1.5A7 7 0 0 0 5 12c0 .5.1 1.1.2 1.6l-2 1.5 2 3.4 2.3-1a7 7 0 0 0 2.8 1.6l.4 2.9h2.6l.4-2.9a7 7 0 0 0 2.8-1.6l2.3 1 2-3.4-2-1.5c.1-.5.2-1.1.2-1.6z";

// The way in, for the project header.
export function settingsLink(projectId) {
  return `<a class="proj-gear" href="#/projects/${esc(encodeURIComponent(projectId))}/settings" aria-label="Project settings">
    <svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="${GEAR}"></path></svg>
  </a>`;
}

// The form with edits pending, if there is one. Every way out of the screen is
// a hash change, so that is where leaving is asked about. This listener is
// registered when the module loads, which is before the router's own, so
// stopping the event here is what keeps the form on screen.
let guard = null;
let asking = false;

const pending = () => !!guard && guard.form.isConnected && guard.dirty();

// Where this entry sits in the session's history. A hash change does not say
// whether the reader went forward or back, so every entry is stamped with its
// place as it is first seen: a new entry is one past the entry it was reached
// from, and an entry that already carries a place was travelled back or
// forward to. Undoing a move is then a step of the right size, which leaves
// the history the reader came through as it was. Replacing the entry instead
// would overwrite the one they went back to.
const PLACE = "hubPlace";
let here = history.state?.[PLACE] ?? 0;
history.replaceState({ ...(history.state || {}), [PLACE]: here }, "");
// Moves of our own making, whose hash change is not the reader's.
let undoing = 0;

function arrive() {
  const place = history.state?.[PLACE];
  if (place != null) {
    here = place;
    return;
  }
  here += 1;
  history.replaceState({ ...(history.state || {}), [PLACE]: here }, "");
}

window.addEventListener("hashchange", async (event) => {
  if (undoing) {
    undoing -= 1;
    arrive();
    // The form is still on screen and still the reader's; a repaint would
    // cost them the edits the question is about.
    if (pending()) event.stopImmediatePropagation();
    return;
  }
  if (!pending()) {
    guard = null;
    arrive();
    return;
  }
  event.stopImmediatePropagation();
  const target = location.hash;
  const place = history.state?.[PLACE];
  // A fresh entry is one step on; a stamped one is as far as its place says.
  const moved = place != null ? place - guard.place : 1;
  if (place == null) history.replaceState({ ...(history.state || {}), [PLACE]: guard.place + 1 }, "");
  undoing += 1;
  history.go(-moved);
  if (asking) return;
  asking = true;
  const leave = await confirmAction({
    title: "Discard unsaved changes?",
    body: "The edits to this project have not been saved.",
    safe: "Keep editing",
    danger: "Discard changes",
  });
  asking = false;
  if (!leave) return;
  guard = null;
  // Back or forward is taken again as the same step; a link is followed anew.
  if (place != null) history.go(moved);
  else location.hash = target;
});

// A reload or a closed tab is the one exit a hash change does not see.
window.addEventListener("beforeunload", (event) => {
  if (pending()) event.preventDefault();
});

function localDate(timestamp) {
  const at = new Date(timestamp);
  if (Number.isNaN(at.getTime())) return "";
  const two = (n) => String(n).padStart(2, "0");
  return `${at.getFullYear()}-${two(at.getMonth() + 1)}-${two(at.getDate())}`;
}

function skeleton(projectId, deletable) {
  const back = `#/projects/${esc(encodeURIComponent(projectId))}/feed`;
  const options = POLICIES.map(
    ([value, label, note]) => `
      <label class="pset-option">
        <input type="radio" name="artifact_password_policy" value="${value}" aria-describedby="pset-policy-${value}">
        <span class="grow">
          <span class="pset-option-name">${label}</span>
          <span class="pset-option-note" id="pset-policy-${value}">${note}</span>
        </span>
      </label>`,
  ).join("");
  return `
    <div class="proj-head">
      <a class="proj-back" href="${back}" aria-label="Back to the project">
        <svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M 15 6l-6 6 6 6"></path></svg>
      </a>
      <div class="grow">
        <h1>Project settings</h1>
        <p class="pset-sub mono"></p>
      </div>
    </div>
    <form class="pset" data-action="project-settings" novalidate>
      <div class="pset-field">
        <label for="pset-name">Name</label>
        <input id="pset-name" name="display_name" autocomplete="off" required>
        <p class="pset-error" id="pset-name-error" hidden></p>
      </div>
      <div class="pset-field">
        <span class="pset-label">Slug</span>
        <span class="pset-hint mono">used in MCP calls · read-only after creation</span>
        <p class="pset-slug"></p>
      </div>
      <fieldset class="pset-policy" role="radiogroup">
        <legend class="section-label">Artifact password policy</legend>
        <div class="pset-options">${options}</div>
        <p class="pset-error" id="pset-policy-error" hidden></p>
      </fieldset>
      <h2 class="section-label">Retention <span class="pset-reserved">· reserved</span></h2>
      <p class="pset-retention">Automatic pruning isn’t in v1. You are the garbage collector: see <a href="#/storage">Storage</a>.</p>
      <p class="pset-error pset-problem" role="alert" hidden></p>
      <div class="pset-actions">
        <button class="primary" type="submit" disabled>Save</button>
        ${deletable ? '<button type="button" class="pset-delete">Delete project…</button>' : ""}
      </div>
    </form>`;
}

function say(field, box, message) {
  box.textContent = message;
  box.hidden = !message;
  if (message) {
    field.setAttribute("aria-invalid", "true");
    field.setAttribute("aria-describedby", box.id);
  } else {
    field.removeAttribute("aria-invalid");
    field.removeAttribute("aria-describedby");
  }
}

export async function projectSettingsScreen(gen, path) {
  const projectId = decodeURIComponent((path || "").split("/")[2] || "");
  let saved;
  try {
    saved = await api(`/api/v1/projects/${encodeURIComponent(projectId)}`);
  } catch (error) {
    if (error.status !== 404) throw error;
    paint(
      gen,
      `<h1>Project settings</h1>${emptyStateHTML(NOT_FOUND, { project: projectId }, { href: "#/projects" })}`,
    );
    return;
  }
  if (stale(gen)) return;

  // An agent's personal space is settable but not deletable: it goes with the
  // agent, so the control the hub would refuse is not offered.
  paint(gen, skeleton(saved.id, !saved.owner_agent));
  document.title = "Project settings · Agent Hub";

  const form = main.querySelector(".pset");
  const name = form.elements.display_name;
  const nameError = form.querySelector("#pset-name-error");
  const policy = form.querySelector(".pset-policy");
  const policyError = form.querySelector("#pset-policy-error");
  const problem = form.querySelector(".pset-problem");
  const save = form.querySelector('button[type="submit"]');
  const radios = [...form.querySelectorAll('input[type="radio"]')];

  const created = localDate(saved.created_at);
  main.querySelector(".pset-sub").textContent = created ? `${saved.id} · created ${created}` : saved.id;
  form.querySelector(".pset-slug").textContent = saved.id;

  const show = () => {
    name.value = saved.display_name;
    for (const radio of radios) radio.checked = radio.value === saved.artifact_password_policy;
  };
  show();

  // Only what differs from the hub's copy is a change, so a save carries
  // nothing the reader did not touch.
  const changes = () => {
    const body = {};
    const typed = name.value.trim();
    if (typed !== saved.display_name) body.display_name = typed;
    const picked = radios.find((radio) => radio.checked);
    if (picked && picked.value !== saved.artifact_password_policy) {
      body.artifact_password_policy = picked.value;
    }
    return body;
  };
  let saving = false;
  const sync = () => {
    save.disabled = saving || Object.keys(changes()).length === 0;
    form.toggleAttribute("aria-busy", saving);
  };
  guard = {
    form,
    place: here,
    dirty: () => Object.keys(changes()).length > 0,
  };

  name.addEventListener("input", () => {
    say(name, nameError, "");
    problem.hidden = true;
    sync();
  });
  policy.addEventListener("change", () => {
    say(policy, policyError, "");
    problem.hidden = true;
    sync();
  });

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (saving) return;
    const body = changes();
    if (!Object.keys(body).length) return;
    if ("display_name" in body && !body.display_name) {
      say(name, nameError, "A project needs a name.");
      name.focus();
      return;
    }
    saving = true;
    sync();
    try {
      saved = await api(`/api/v1/projects/${encodeURIComponent(saved.id)}`, {
        method: "PATCH",
        body: JSON.stringify(body),
      });
      show();
      // Save goes quiet once nothing is left to save, and a disabled button
      // drops the focus it held, so the reader is put back on the form.
      if (!form.contains(document.activeElement) || document.activeElement === save) name.focus();
      toast("Project saved.");
    } catch (error) {
      // The hub names the field it refused. Anything else, a dropped
      // connection or a missing token, is about the save as a whole.
      if ("display_name" in body && /display name/i.test(error.message)) {
        say(name, nameError, error.message);
        name.focus();
      } else if (/artifact_password_policy/.test(error.message)) {
        say(policy, policyError, error.message);
      } else {
        problem.textContent = `Nothing changed: ${error.message}`;
        problem.hidden = false;
      }
    } finally {
      saving = false;
      sync();
    }
  });

  const remove = form.querySelector(".pset-delete");
  if (remove) {
    remove.addEventListener("click", () => {
      deleteProject(saved.id, () => {
        guard = null;
        location.hash = "#/projects";
      }).catch((error) => {
        problem.textContent = `Nothing changed: ${error.message}`;
        problem.hidden = false;
      });
    });
  }
}
