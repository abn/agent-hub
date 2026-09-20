// The screen that asks for the hub's access token.
//
// Every route the hub serves under /api/v1 refuses a request without one, so
// a reader with no token has nothing to read. The router sends them here with
// the route they were going to, this screen checks the token against the hub
// before keeping it, and then puts them back.

import { api } from "./api.mjs";
import { esc, paint } from "./dom.mjs";
import { saveToken } from "./prefs.mjs";
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
  '<svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.5">' +
  '<rect x="5" y="10.5" width="14" height="10" rx="2"></rect>' +
  '<path d="M 8 10.5V7a4 4 0 0 1 8 0v3.5"></path></svg>';

export async function connectScreen(params, gen) {
  const next = nextFrom(params);
  // A refusal can arrive with a dialog open on the screen behind. It outlives
  // the repaint, and a modal over this one would leave the field unreachable.
  document.querySelectorAll("dialog[open]").forEach((box) => box.close());
  paint(
    gen,
    `<section class="connect card">
      <div class="connect-mark" aria-hidden="true">${LOCK}</div>
      <h1>Connect to this hub</h1>
      <p class="connect-copy">This hub asks for an access token on every request. It is the token the hub was started with, and it stays on this device.</p>
      <form data-action="connect" data-next="${esc(next)}">
        <input class="sr-only" type="text" name="username" value="hub" autocomplete="username" tabindex="-1" aria-hidden="true">
        <div class="pset-field">
          <label class="pset-label" for="hub-token">Access token</label>
          <input class="connect-field" id="hub-token" name="token" type="password" autocomplete="current-password" spellcheck="false" aria-describedby="hub-token-error">
          <p class="connect-error pset-error" id="hub-token-error" role="alert" hidden></p>
        </div>
        <label class="connect-show"><input type="checkbox" data-role="show-token"> Show token</label>
        <button class="primary" type="submit">Connect</button>
      </form>
    </section>`,
  );
}

// A token is unreadable as dots and is usually pasted, so it can be read back
// before it is sent rather than only after the hub has refused it.
export function connectShow(box) {
  const field = box.closest("form")?.querySelector(".connect-field");
  if (field) field.type = box.checked ? "text" : "password";
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
    // A frame of ours around the hub's own words, as a refused write says
    // "Nothing changed" and then why. The hub answers a token it does not
    // know and a hub with no token configured with the same code, so a
    // sentence that named one of them would be wrong half the time.
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
