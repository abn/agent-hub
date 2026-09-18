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

main.addEventListener("click", (event) => {
  const button = event.target.closest("[data-action]");
  if (!button || button.tagName !== "BUTTON") return;
  const { action, id } = button.dataset;
  if (action === "answer") answer(id).catch((error) => alert(error.message));
  if (action === "approve")
    approve(id, button.dataset.summary).catch((error) => {
      alert(error.message);
      render();
    });
  if (action === "kind")
    toggleKind(button.dataset.kind, main.querySelector('[data-role="project"]')?.value);
  if (action === "end") endSession(id).catch((error) => alert(error.message));
  if (action === "prune") pruneSession(id).catch((error) => alert(error.message));
  if (action === "agent-trust")
    setAgentTrust(id, button.dataset.trust).catch((error) => alert(error.message));
  if (action === "agent-token") reissueToken(id).catch((error) => alert(error.message));
  if (action === "agent-revoke") revokeToken(id).catch((error) => alert(error.message));
  if (action === "agent-ungrant")
    ungrant(id, button.dataset.project).catch((error) => alert(error.message));
  if (action === "artifact-open") openArtifact(id);
  if (action === "project-delete") deleteProject(id).catch((error) => alert(error.message));
  if (action === "notification-enable")
    enableNotifications().catch((error) => alert(error.message));
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
      .catch((error) => alert(error.message));
  } else if (action === "agent-create") {
    const payload = { id: data.get("id"), display_name: data.get("display_name") };
    const trust = String(data.get("trust") || "");
    if (trust) payload.trust = trust;
    api("/api/v1/agents", { method: "POST", body: JSON.stringify(payload) })
      .then(() => render())
      .catch((error) => alert(error.message));
  } else if (action === "agent-grant") {
    api(`/api/v1/agents/${encodeURIComponent(data.get("agent"))}/grants`, {
      method: "POST",
      body: JSON.stringify({ project_id: data.get("project"), access: data.get("access") }),
    })
      .then(() => render())
      .catch((error) => alert(error.message));
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
