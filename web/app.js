// Agent Hub PWA. A small vanilla single-page app over the REST API.
//
// This is the entry point: it names the screens the router can paint, routes
// the delegated events to the handler that owns each action, and starts the
// freshness stream. Everything else lives in its own module beside this file.

import { reissueToken, revokeToken, setAgentTrust, ungrant } from "./agents.mjs";
import { api } from "./api.mjs";
import { artifactsScreen, openArtifact } from "./artifacts.mjs";
import { main } from "./dom.mjs";
import { refreshBadge, startStream } from "./events.mjs";
import { projectsScreen, toggleKind } from "./feed.mjs";
import { home } from "./home.mjs";
import { answer, approve, inbox } from "./inbox.mjs";
import { savePrefs } from "./prefs.mjs";
import { render, setScreens } from "./router.mjs";
import { searchScreen } from "./search.mjs";
import { endSession, pruneSession, sessionDetail, sessionsScreen } from "./sessions.mjs";
import { deleteProject, enableNotifications, settingsScreen } from "./settings.mjs";
import { storageScreen } from "./storage.mjs";
import { toast } from "./toast.mjs";

setScreens({
  home: (params, gen) => home(gen),
  inbox: (params, gen) => inbox(gen),
  feed: (params, gen) => projectsScreen(params.get("project"), gen),
  search: (params, gen) => searchScreen(params.get("q"), gen),
  artifacts: (params, gen) => artifactsScreen(params.get("project"), gen),
  sessions: (params, gen) => sessionsScreen(params.get("project"), gen),
  session: (params, gen) => sessionDetail(params.get("project"), params.get("id"), gen),
  storage: (params, gen) => storageScreen(gen),
  settings: (params, gen) => settingsScreen(gen),
});

// What a failed write says. The detail is the hub's own; the sentence around
// it states that the screen is as it was.
const failed = (error) => toast(`Nothing changed: ${error.message}`);

// A write repaints the screen it wrote to, so the control the reader pressed
// is a new node or has gone with the item it acted on. Focus follows it: the
// same control while the item is there, the next item offering the same verb
// once it is not, and the region itself only when neither is left. An action
// that placed focus itself, in a composer or on an undo, keeps it.
function acted(button, work) {
  const { action, id } = button.dataset;
  work.catch(failed).then(() => {
    if (document.activeElement !== main && document.activeElement !== document.body) return;
    const verb = `[data-action="${action}"]`;
    const same = id && main.querySelector(`${verb}[data-id="${CSS.escape(id)}"]`);
    (same || main.querySelector(verb) || main).focus({ preventScroll: true });
  });
}

main.addEventListener("click", (event) => {
  const button = event.target.closest("[data-action]");
  if (!button || button.tagName !== "BUTTON") return;
  const { action, id } = button.dataset;
  if (action === "answer") acted(button, answer(id, button));
  if (action === "approve") acted(button, approve(id, button.dataset.summary));
  if (action === "kind")
    toggleKind(button.dataset.kind, main.querySelector('[data-role="project"]')?.value);
  if (action === "end") acted(button, endSession(id));
  if (action === "prune") acted(button, pruneSession(id, button.dataset.agent));
  if (action === "agent-trust") acted(button, setAgentTrust(id, button.dataset.trust));
  if (action === "agent-token") acted(button, reissueToken(id));
  if (action === "agent-revoke") acted(button, revokeToken(id));
  if (action === "agent-ungrant") acted(button, ungrant(id, button.dataset.project));
  if (action === "artifact-open") openArtifact(id);
  if (action === "project-delete") acted(button, deleteProject(id));
  if (action === "notification-enable") acted(button, enableNotifications());
});

main.addEventListener("submit", (event) => {
  const form = event.target.closest("form[data-action]");
  if (!form) return;
  event.preventDefault();
  const data = new FormData(form);
  const action = form.dataset.action;
  if (action === "search") {
    location.hash = `#/search?q=${encodeURIComponent(data.get("q") || "")}`;
  } else if (action === "prefs") {
    savePrefs({
      token: data.get("token"),
      theme: data.get("theme"),
      density: data.get("density"),
    });
    render();
  } else if (action === "project") {
    api("/api/v1/projects", {
      method: "POST",
      body: JSON.stringify({ id: data.get("id"), display_name: data.get("display_name") }),
    })
      .then(() => (location.hash = "#/feed"))
      .catch(failed);
  } else if (action === "agent-create") {
    const payload = { id: data.get("id"), display_name: data.get("display_name") };
    const trust = String(data.get("trust") || "");
    if (trust) payload.trust = trust;
    api("/api/v1/agents", { method: "POST", body: JSON.stringify(payload) })
      .then(() => render())
      .catch(failed);
  } else if (action === "agent-grant") {
    api(`/api/v1/agents/${encodeURIComponent(data.get("agent"))}/grants`, {
      method: "POST",
      body: JSON.stringify({ project_id: data.get("project"), access: data.get("access") }),
    })
      .then(() => render())
      .catch(failed);
  }
});

main.addEventListener("change", (event) => {
  if (event.target.dataset.role === "project") {
    const screen = location.hash.replace(/^#/, "").split("?")[0] || "/feed";
    location.hash = `${screen}?project=${encodeURIComponent(event.target.value)}`;
  }
});

const skipLink = document.querySelector(".skip-link");
if (skipLink) {
  skipLink.addEventListener("click", (event) => {
    event.preventDefault();
    main.focus();
  });
}

window.addEventListener("hashchange", render);

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js").catch(() => {});
}

// The freshness stream nudges a refetch when a write lands; the slow poll is
// the fallback if the stream drops or the browser cannot stream a fetch.
setInterval(() => refreshBadge(), 60000);
startStream();

render();
