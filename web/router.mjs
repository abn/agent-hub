// The hash router. It owns which screen is current, the nav marking, and the
// one place a screen's failure becomes an error card.

import { esc, main } from "./dom.mjs";
import { refreshBadge } from "./events.mjs";
import { applyPrefs } from "./prefs.mjs";

// The screen table, registered by the entry point. Keeping it out of this
// module is what lets a screen ask for a render without an import cycle.
let screens = {};

export function setScreens(table) {
  screens = table;
}

// The chip a render should return focus to, set when a filter is toggled.
let pendingFocus = null;

export function focusAfterRender(kind) {
  pendingFocus = kind;
}

function setCurrent(screen) {
  document.querySelectorAll(".tabbar a, .topbar nav a").forEach((anchor) => {
    const target = (anchor.getAttribute("href") || "").replace(/^#\//, "").split("?")[0];
    if (target === screen) anchor.setAttribute("aria-current", "page");
    else anchor.removeAttribute("aria-current");
  });
  document.title = screen.charAt(0).toUpperCase() + screen.slice(1) + " · Agent Hub";
}

export async function render() {
  applyPrefs();
  const hash = location.hash.replace(/^#/, "") || "/home";
  const [path, query = ""] = hash.split("?");
  const params = new URLSearchParams(query);
  const screen = path.split("/")[1] || "home";
  setCurrent(screen);
  try {
    await (screens[screen] || screens.home)(params);
    refreshBadge();
  } catch (error) {
    main.innerHTML = `<h1>Agent Hub</h1><p class="error">${esc(error.message)}</p>`;
  }
  const focus = pendingFocus;
  pendingFocus = null;
  const chip = focus && main.querySelector(`[data-action="kind"][data-kind="${CSS.escape(focus)}"]`);
  if (chip) chip.focus({ preventScroll: true });
  else main.focus({ preventScroll: true });
}
