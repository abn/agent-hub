// The shell around the screens: the desktop top bar's node line and its
// search field's slash shortcut. The top bar is static markup in the shell
// document; what comes from the hub fills in at runtime.

import { api } from "./api.mjs";
import { prefs } from "./prefs.mjs";

const nodeLine = document.getElementById("top-node");
const searchField = document.getElementById("top-search");

// The node line names the hub the operator is looking at. It is the storage
// response's node, which the real-numbers surface already reports, so the two
// cannot drift. Without a token there is nothing to say, and the line stays
// out of the way until one is entered.
let askedForToken = false;
async function fillNodeLine() {
  if (!prefs.token) {
    askedForToken = true;
    setTimeout(fillNodeLine, 15000);
    return;
  }
  try {
    const usage = await api("/api/v1/storage");
    nodeLine.textContent = `${usage.node.host} · ${usage.node.mode}`;
  } catch {
    // A hub that refuses still has a line to draw once it accepts a token;
    // try again on the same cadence as the mailbox stream.
    setTimeout(fillNodeLine, 15000);
  }
}

// The tab bar's badge is the app's one count. The top bar badge mirrors it so
// the desktop Inbox link reads the same number, without the shell touching
// the freshness module's single source of truth.
const tabBadge = document.getElementById("tab-badge");
const topBadge = document.getElementById("top-badge");
function mirrorBadge() {
  if (!tabBadge || !topBadge) return;
  topBadge.hidden = tabBadge.hidden;
  topBadge.textContent = tabBadge.textContent;
}
if (tabBadge && "MutationObserver" in window) {
  new MutationObserver(mirrorBadge).observe(tabBadge, {
    attributes: true,
    childList: true,
    subtree: true,
    attributeFilter: ["hidden"],
  });
}

// Slash focuses the top bar's own search field on desktop, before the keyboard
// map's slash handler (which navigates to the Search screen) sees the key.
// The capture phase runs first, and the shortcut is only claimed when the
// field is actually on screen, so mobile keeps the map's behaviour untouched.
document.addEventListener(
  "keydown",
  (event) => {
    if (event.key !== "/" || event.altKey || event.ctrlKey || event.metaKey) return;
    if (prefs.shortcuts !== "on") return;
    const target = event.target;
    if (target && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName))) return;
    if (document.querySelector("dialog[open]")) return;
    if (location.hash.startsWith("#/search")) {
      const q = document.getElementById("q");
      if (q) {
        event.preventDefault();
        event.stopPropagation();
        q.focus();
        q.select?.();
        return;
      }
    }
    if (!searchField || !searchField.getClientRects().length) return;
    event.preventDefault();
    event.stopPropagation();
    searchField.focus();
    searchField.select?.();
  },
  true,
);

export function installShell() {
  if (askedForToken) return;
  fillNodeLine();
  mirrorBadge();
}
