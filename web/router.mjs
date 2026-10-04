// The hash router. It owns which screen is current, the nav marking, and the
// one place a screen's failure becomes an error card.

import { beginRender, esc, main, paint } from "./dom.mjs";
import { refreshBadge } from "./events.mjs";
import { applyPrefs } from "./prefs.mjs";
import { resetScrollCollapse } from "./shell-layout.mjs";

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

// The path the last render painted. A change that keeps it and only moves the
// query is the same screen with a different selection, not a new place: the
// reader is already there, so the route focus below would take them out of
// what they were reading.
let lastPath = null;

// Where focus goes after a paint that happens under a reader who is already on
// the screen. The screen names it by a lookup rather than by a node, because
// the node it means is painted by the render in flight and is not in the page
// yet. A lookup that finds nothing leaves focus where the browser dropped it,
// and the route focus below takes it from there.
let pendingRestore = null;

export function restoreFocusAfterRender(resolve) {
  pendingRestore = resolve;
}

function setCurrent(screen, path = "") {
  // The project-owned screens sit under the Projects tab: the segmented
  // project view has that tab on every segment, and the artifact viewer is
  // the artifact inside a project, so it keeps the tab where it was opened.
  // Settings, Storage and Agents and tokens are pushed from More: on a phone
  // More stays current while they are open.
  const isPushedFromMore =
    screen === "settings" || screen === "storage" || screen === "access" || screen === "agents";
  const railNav = screen === "artifacts" ? "projects" : screen;
  const tabNav = screen === "artifacts" ? "projects" : isPushedFromMore ? "more" : screen;

  document.querySelectorAll(".tabbar a, .rail a").forEach((anchor) => {
    if (anchor.classList.contains("rail-project-item")) {
      const projId = anchor.dataset.projectId;
      if (
        projId &&
        (path === `/projects/${projId}` ||
          path.startsWith(`/projects/${projId}/`) ||
          path === `/projects/${encodeURIComponent(projId)}` ||
          path.startsWith(`/projects/${encodeURIComponent(projId)}/`))
      ) {
        anchor.setAttribute("aria-current", "page");
      } else {
        anchor.removeAttribute("aria-current");
      }
      return;
    }
    const href = (anchor.getAttribute("href") || "").replace(/^#\//, "").split("?")[0];
    const target = href.split("/")[0];
    if (anchor.classList.contains("rail-item")) {
      if (anchor.dataset.route === railNav) anchor.setAttribute("aria-current", "page");
      else anchor.removeAttribute("aria-current");
    } else {
      if (target === tabNav) anchor.setAttribute("aria-current", "page");
      else anchor.removeAttribute("aria-current");
    }
  });
  document.title = screen.charAt(0).toUpperCase() + screen.slice(1) + " · Agent Hub";
}

export async function render() {
  applyPrefs();
  resetScrollCollapse();
  const gen = beginRender();
  const hash = location.hash.replace(/^#/, "") || "/home";
  const [path, query = ""] = hash.split("?");
  const params = new URLSearchParams(query);
  const screen = path.split("/")[1] || "home";
  const sameScreen = path === lastPath;
  lastPath = path;
  setCurrent(screen, path);
  try {
    // The path is handed over so a route with an id of its own, such as the
    // project view and the artifact viewer, can read its segment without
    // reaching into the location itself.
    refreshBadge(await (screens[screen] || screens.home)(params, gen, path));
  } catch (error) {
    // A refusal to authenticate is not this screen's to report: there is one
    // screen that can fix it, and it takes the route it interrupted so the
    // reader arrives where they were going rather than at the start.
    if (error.status === 401 && screen !== "connect") {
      location.hash = `#/connect?next=${encodeURIComponent(hash)}`;
      return;
    }
    paint(gen, `<h1>Agent Hub</h1><p class="error">${esc(error.message)}</p>`);
  }
  const focus = pendingFocus;
  const restore = pendingRestore;
  pendingFocus = null;
  pendingRestore = null;
  const chip = focus && main.querySelector(`[data-action="kind"][data-kind="${CSS.escape(focus)}"]`);
  // Only a change that kept the screen is handed to the screen's own restore.
  // After a real route change the reader has moved, and the route focus below
  // is what tells them where they landed. A restore that finds its item puts
  // focus back inside the screen, which is also what keeps the route focus from
  // taking the reader to the heading over another selection on the screen they
  // are already on.
  if (restore && sameScreen) restore();
  if (chip) {
    chip.focus({ preventScroll: true });
  } else if (!main.contains(document.activeElement) || document.activeElement === main) {
    const heading = main.querySelector("h1, [role='heading']");
    // Marked so the stylesheet can leave the ring off this one element. The
    // target is moved to for announcement, not because a reader steered here,
    // and nothing can tab to it, so a ring marks nothing reachable. The mark
    // is narrow on purpose: a blanket rule over [tabindex="-1"] would take
    // the ring off the brain tree's roving items, which are reached by arrow
    // keys and need it.
    if (heading) {
      if (!heading.hasAttribute("tabindex")) heading.setAttribute("tabindex", "-1");
      heading.dataset.routeFocus = "";
      heading.focus({ preventScroll: true });
    } else {
      main.dataset.routeFocus = "";
      main.focus({ preventScroll: true });
    }
  }
}
