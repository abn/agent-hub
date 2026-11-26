// The hash router. It owns which screen is current, the nav marking, and the
// one place a screen's failure becomes an error state inside the shell.

import { api } from "./api.mjs";
import { primeMembers } from "./author.mjs";
import { beginRender, main, paint, stale } from "./dom.mjs";
import { ERROR_COPY, errorDetail, errorShellHTML } from "./empty.mjs";
import { refreshBadge } from "./events.mjs";
import { applyPrefs, prefs } from "./prefs.mjs";
import { installShellLayout, resetScrollCollapse, shellHTML, shellStageHead } from "./shell-layout.mjs";

// The screen table, registered by the entry point. Keeping it out of this
// module is what lets a screen ask for a render without an import cycle.
let screens = {};

export function setScreens(table) {
  screens = table;
}

// The behavioural harness reads the router's own table to prove every screen is
// covered by the render guard. It is the registered list rather than a second
// one in the test, so a screen added to the table fails until it is held. This
// is the same shape as the sync counter the shell exposes for its own check.
if (typeof window !== "undefined") {
  window.__router = { screens: () => Object.keys(screens) };
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
  // The artifacts list has its own rail row. It, Settings, Storage and Members
  // and tokens are pushed from More: on a phone More stays current while they
  // are open.
  const isPushedFromMore =
    screen === "settings" ||
    screen === "notifications" ||
    screen === "storage" ||
    screen === "access" ||
    screen === "phones" ||
    screen === "artifacts";
  // The Inbox's address is Now showing what needs the reader, so it marks Now.
  const own = screen === "inbox" ? "home" : screen;
  // Notifications is pushed from Settings, so the rail keeps Settings marked.
  const railNav = screen === "notifications" || screen === "phones" ? "settings" : own;
  const tabNav = isPushedFromMore ? "more" : own;

  document.querySelectorAll(".tabbar a, .rail a").forEach((anchor) => {
    if (anchor.classList.contains("rail-project-item")) {
      const projId = /** @type {HTMLElement} */ (anchor).dataset.projectId;
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
      if (/** @type {HTMLElement} */ (anchor).dataset.route === railNav) anchor.setAttribute("aria-current", "page");
      else anchor.removeAttribute("aria-current");
    } else {
      if (target === tabNav) anchor.setAttribute("aria-current", "page");
      else anchor.removeAttribute("aria-current");
    }
  });
  const name = own === "home" ? "now" : screen;
  document.title = name.charAt(0).toUpperCase() + name.slice(1) + " · Agent Hub";
}

// What a failed screen is called in its header: the screen's own name, or the
// project a project route is inside, since the failure replaces the stage that
// would have carried the project's name.
const TITLES = {
  home: "Now",
  inbox: "Now",
  projects: "Projects",
  feed: "Feed",
  sessions: "Sessions",
  session: "Sessions",
  artifacts: "Artifacts",
  search: "Search",
  storage: "Storage",
  settings: "Settings",
  notifications: "Notifications",
  more: "More",
  access: "Access",
  phones: "Phones",
  connect: "Connect",
  id: "Opening",
};

function failedTitle(screen, path) {
  const project = screen === "projects" ? path.split("/")[2] : "";
  if (project) {
    try {
      return decodeURIComponent(project);
    } catch {
      return project;
    }
  }
  return TITLES[screen] || "Agent Hub";
}

// The failed screen keeps the shell. Retry runs the same render again, so it
// recovers as soon as the hub does without the reader losing their place.
function paintFailure(gen, screen, path, error) {
  paint(
    gen,
    errorShellHTML(failedTitle(screen, path), ERROR_COPY.load, {
      detail: errorDetail(error),
      where: { action: "retry", button: true },
    }),
  );
  if (stale(gen)) return;
  installShellLayout(main);
  main.querySelector('[data-action="retry"]')?.addEventListener("click", () => render());
}

export async function render() {
  applyPrefs();
  resetScrollCollapse();
  const hash = location.hash.replace(/^#/, "") || "/home";
  const [path, query = ""] = hash.split("?");
  const params = new URLSearchParams(query);
  const screen = path.split("/")[1] || "home";
  // An address no screen answers to is replaced by Now's own, so the bar
  // names the place the reader is in and Back does not return to the dead one.
  if (!Object.hasOwn(screens, screen)) {
    location.replace("#/home");
    return;
  }
  const gen = beginRender();
  const sameScreen = path === lastPath;
  lastPath = path;
  setCurrent(screen, path);
  try {
    // The path is handed over so a route with an id of its own, such as the
    // project view and the artifact viewer, can read its segment without
    // reaching into the location itself.
    // Receipts name the member unless the hub has only one, so the count is
    // known before a row is drawn. It is cached, so most renders ask nothing.
    await primeMembers();
    refreshBadge(await screens[screen](params, gen, path));
  } catch (error) {
    // A failure that lands after the reader has moved on is not theirs to see,
    // and a late refusal must not take them to Connect from somewhere else.
    if (stale(gen)) return;
    // A refusal to authenticate is not this screen's to report: there is one
    // screen that can fix it, and it takes the route it interrupted so the
    // reader arrives where they were going rather than at the start.
    if (error.status === 401 && screen !== "connect") {
      // A token that was sent and refused is told apart from none at all.
      const why = prefs.token ? "&reason=refused" : "";
      location.hash = `#/connect?next=${encodeURIComponent(hash)}${why}`;
      return;
    }
    paintFailure(gen, screen, path, error);
  }
  const focus = pendingFocus;
  const restore = pendingRestore;
  pendingFocus = null;
  pendingRestore = null;
  const chip = /** @type {HTMLElement} */ (focus && main.querySelector(`[data-action="kind"][data-kind="${CSS.escape(focus)}"]`));
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
    const heading = /** @type {HTMLElement} */ (main.querySelector("h1, [role='heading']"));
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

// `#/id/<id>` is the short address an agent hands over for a hub resource. The
// hub says what the id names and where it is read; this screen asks, then
// replaces itself with that route so Back leaves the link rather than landing
// on a hop that forwards again.
const OPENING = shellHTML({
  noIndex: true,
  segment: "opening",
  stageHead: shellStageHead("Opening", ""),
  stageControls: `<div class="shell-controls"></div>`,
  stageBody: `<div class="shell-pad"><p class="empty" role="status">Opening the link.</p></div>`,
});

// Only a route inside the app is followed, and never this screen again.
const isRoute = (route) => typeof route === "string" && route.startsWith("#/") && !route.startsWith("#/id/");

export async function resourceRoute(path, gen) {
  let id = "";
  try {
    id = decodeURIComponent((path || "").split("/")[2] || "");
  } catch {}
  if (!id) {
    location.replace("#/home");
    return;
  }
  paint(gen, OPENING);
  if (stale(gen)) return;
  installShellLayout(main);
  let found;
  try {
    found = await api(`/api/v1/resolve/${encodeURIComponent(id)}`);
  } catch (error) {
    // The same sentence for a missing id and one the reader may not read, so
    // the page says nothing about whether the id exists elsewhere. Any other
    // failure is the router's: Connect for a refusal, Retry for the rest.
    if (stale(gen) || error.status !== 404) throw error;
    paint(gen, errorShellHTML("Not found", ERROR_COPY.resourceGone, { where: { href: "#/home" } }));
    if (!stale(gen)) installShellLayout(main);
    return;
  }
  if (stale(gen)) return;
  if (!isRoute(found?.route)) {
    paint(gen, errorShellHTML("Not found", ERROR_COPY.resourceGone, { where: { href: "#/home" } }));
    if (!stale(gen)) installShellLayout(main);
    return;
  }
  location.replace(found.route);
}
