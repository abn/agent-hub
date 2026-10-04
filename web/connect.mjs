// The screen that asks for the hub's access token.
//
// Every route the hub serves under /api/v1 refuses a request without one, so
// a reader with no token has nothing to read. The router sends them here with
// the route they were going to, this screen checks the token against the hub
// before keeping it, and then puts them back.

import { api } from "./api.mjs";
import { esc, paint } from "./dom.mjs";
import { saveToken } from "./prefs.mjs";
import {
  installShellLayout,
  shellHTML,
} from "./shell-layout.mjs";
import { toast } from "./toast.mjs";

// Where the reader was going before the hub asked who they were. Only a route
// of this app is accepted: a `next` from a crafted link is not ours to follow,
// a bare or doubled slash is not a route, and this screen is not a place to
// send a reader who has just left it.
export function nextFrom(params) {
  const wanted = String(params.get("next") || "");
  return /^\/(?!connect\b)[^/]/.test(wanted) ? wanted : "/home";
}

const LOCK =
  '<svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">' +
  '<rect x="5" y="11" width="14" height="10" rx="2"></rect>' +
  '<path d="M 8 11V8a4 4 0 0 1 8 0v3"></path></svg>';

const CONNECT_STYLE = `<style>
.connect-container {
  padding: 20px 16px 48px;
  max-width: 480px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  background: var(--bg);
  box-sizing: border-box;
}
/* The form is a stack of its own, so the gap between the field and the button
   is the same 16 the container puts between the intro and the form. */
.connect-form {
  display: flex;
  flex-direction: column;
  gap: 16px;
}
.connect-intro {
  margin: 0;
  font-size: var(--t-15);
  line-height: 1.55;
  color: var(--ink-2);
}
.connect-group {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.connect-label {
  font-size: var(--t-13);
  font-weight: 600;
  color: var(--ink);
  margin: 0;
}
.connect-field-wrap {
  height: 48px;
  min-height: 48px;
  box-sizing: border-box;
  display: flex;
  align-items: center;
  padding: 0 4px 0 14px;
  border: 1px solid var(--line-strong);
  border-radius: var(--r-1);
  background: var(--surface);
}
/* The ring is the field box's, as the search field draws it, so nothing inside
   the box draws a second one. The transparent outlines are what a browser in
   forced colours has left to colour in, where every shadow is gone. */
.connect-field-wrap:focus-within {
  outline: 2px solid transparent;
  outline-offset: 2px;
  box-shadow: var(--focus);
}
.connect-field-wrap .connect-field:focus-visible,
.connect-field-wrap .connect-eye-btn:focus-visible {
  outline: 2px solid transparent;
  outline-offset: 2px;
  box-shadow: none;
}
.connect-field {
  flex: 1;
  min-width: 0;
  height: 100%;
  border: 0;
  background: none;
  font-family: var(--font-mono);
  font-size: var(--t-15);
  color: var(--ink);
  padding: 0;
  outline: none;
}
.connect-eye-btn {
  flex: 0 0 44px;
  width: 44px;
  height: 44px;
  min-height: 44px;
  display: grid;
  place-items: center;
  background: none;
  border: 0;
  border-radius: var(--r-1);
  color: var(--ink-2);
  cursor: pointer;
  padding: 0;
}
.connect-eye-btn:hover {
  color: var(--ink);
}
.connect-error {
  margin: 0;
  font-size: var(--t-13);
  color: var(--danger);
  line-height: 1.4;
}
.connect-submit-btn {
  height: 48px;
  min-height: 48px;
  width: 100%;
  border: 0;
  border-radius: var(--r-1);
  background: var(--ink);
  color: var(--ink-inverse);
  font: 600 15px/1 var(--font-sans);
  cursor: pointer;
}
.connect-submit-btn:hover {
  opacity: 0.92;
}
.connect-submit-btn:focus-visible {
  outline: 2px solid transparent;
  outline-offset: 2px;
  box-shadow: var(--focus);
}
.connect-tools-text {
  font-size: var(--t-13);
  color: var(--ink-2);
}
</style>`;

if (typeof document !== "undefined") {
  document.addEventListener("click", (event) => {
    const eye = event.target.closest?.(".connect-eye-btn, [data-role='show-token']");
    if (eye && eye.tagName === "BUTTON") {
      event.preventDefault();
      connectShow(eye);
    }
  });
}

// A hub started without an admin token refuses every control request with a
// sentence of its own. The screen reads that sentence rather than guessing,
// because a token field cannot satisfy a hub that has no token to match.
const NO_ADMIN = /no admin token is configured/i;

async function noAdminToken() {
  try {
    await api("/api/v1/home");
    return false;
  } catch (error) {
    return error.status === 401 && NO_ADMIN.test(error.message || "");
  }
}

export async function connectScreen(params, gen) {
  const next = nextFrom(params);
  // A refusal can arrive with a dialog open on the screen behind. It outlives
  // the repaint, and a modal over this one would leave the field unreachable.
  document.querySelectorAll("dialog[open]").forEach((box) => box.close());

  // The hand-typed fallback "demo · local" named a hub that was not there. The
  // reader's own address is the real node until a token can name it properly.
  const nodeLine =
    document.getElementById("top-node")?.textContent?.trim() || location.host || "this hub";

  const stageHead = `<div class="shell-head">
    <span class="shell-slot" aria-hidden="true">${LOCK}</span>
    <div class="shell-title">
      <h1 class="shell-title-line">Connect</h1>
      <span class="shell-meta mono">${esc(nodeLine)}</span>
    </div>
  </div>`;

  const blocked = await noAdminToken();

  const stageControls = `<div class="shell-controls"><span class="connect-tools-text">${blocked ? "No admin token" : "Not connected"}</span></div>`;

  const stageBody = blocked
    ? `
    <div class="connect connect-container">
      <p class="connect-intro">This hub was started without an admin token, so its control surface is disabled. Set <span class="mono">HUB_ADMIN_TOKEN</span> and restart the hub, then reload this page.</p>
    </div>
  `
    : `
    <div class="connect connect-container">
      <p class="connect-intro">Paste the hub's admin token, the value of <span class="mono">HUB_ADMIN_TOKEN</span> at startup. It stays on this device.</p>
      <form class="connect-form" data-action="connect" data-next="${esc(next)}">
        <input class="sr-only" type="text" name="username" value="hub" autocomplete="username" tabindex="-1" aria-hidden="true">
        <div class="connect-group">
          <label class="connect-label" for="hub-token">Access token</label>
          <div class="connect-field-wrap connect-input-box">
            <input class="connect-field" id="hub-token" name="token" type="password" autocomplete="current-password" spellcheck="false" aria-describedby="hub-token-error">
            <button type="button" class="connect-eye-btn" data-role="show-token" aria-label="Show token" aria-pressed="false">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z"></path><circle cx="12" cy="12" r="3"></circle></svg>
            </button>
          </div>
          <p class="connect-error pset-error" id="hub-token-error" role="alert" hidden></p>
        </div>
        <button class="primary connect-submit-btn" type="submit">Connect</button>
      </form>
    </div>
  `;

  paint(
    gen,
    CONNECT_STYLE +
      shellHTML({
        noIndex: true,
        stageHead,
        stageControls,
        stageBody,
      }),
  );
  installShellLayout(document.querySelector(".shell"));
}

// A token is unreadable as dots and is usually pasted, so it can be read back
// before it is sent rather than only after the hub has refused it.
export function connectShow(box) {
  const form = box.closest("form") || document.querySelector('form[data-action="connect"]');
  const field = form?.querySelector(".connect-field");
  if (!field) return;
  if (box.tagName === "BUTTON") {
    const isText = field.type === "text";
    field.type = isText ? "password" : "text";
    box.setAttribute("aria-pressed", isText ? "false" : "true");
    box.setAttribute("aria-label", isText ? "Show token" : "Hide token");
  } else {
    field.type = box.checked ? "text" : "password";
  }
}

// The error line is in the page from the start and only its words change, so
// the live region announces rather than arriving with the message inside it.
function say(form, message) {
  const box = form.querySelector(".connect-error");
  const field = form.querySelector(".connect-field");
  box.textContent = message;
  box.hidden = !message;
  if (message) {
    field.setAttribute("aria-invalid", "true");
    field.focus();
  } else {
    field.removeAttribute("aria-invalid");
  }
}

let asking = false;

export async function connectSubmit(form) {
  // One question at a time: the button is disabled while the hub is asked,
  // but a form submits on Enter as well, and two answers would race.
  if (asking) return;
  const field = form.querySelector(".connect-field");
  const send = form.querySelector('button[type="submit"]');
  const typed = String(field.value || "").trim();
  if (!typed) {
    say(form, "Type the token this hub was started with.");
    return;
  }
  // A token pasted out of a document can carry a character a header cannot,
  // and the browser would throw before the hub ever saw it.
  if (!/^[\x20-\xFF]*$/.test(typed)) {
    say(form, "That token has a character the hub cannot be sent. Copy it again as plain text.");
    return;
  }
  say(form, "");
  asking = true;
  send.disabled = true;
  form.setAttribute("aria-busy", "true");
  try {
    // Checked before it is kept, so a token the hub refuses never becomes the
    // one every later screen sends. Home is what the reader lands on anyway.
    await api("/api/v1/home", { token: typed });
  } catch (error) {
    // The hub's own words. It answers a token it does not know and a hub with
    // no token configured with the same code, so a sentence of our own here
    // would have to guess which, and would be wrong half the time.
    const why = error.status === 401 ? "Not connected" : "Could not reach the hub";
    say(form, `${why}: ${error.message}`);
    return;
  } finally {
    asking = false;
    send.disabled = false;
    form.removeAttribute("aria-busy");
  }
  // The hub took it, so it is kept whatever happens next. A browser can still
  // refuse to store it: the session works, the next load will ask again, and
  // the reader is told rather than left to wonder.
  if (!saveToken(typed)) {
    toast("This browser would not store the token, so the hub will ask for it again.");
  }
  // The answer can arrive after the reader has moved on. `paint` replaces the
  // region, so a form still in the page is one the reader is still looking at.
  if (form.isConnected) location.hash = `#${form.dataset.next || "/home"}`;
}
