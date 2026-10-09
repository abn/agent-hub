// Agent Hub PWA. A small vanilla single-page app over the REST API.
//
// This is the entry point: it names the screens the router can paint, routes
// the delegated events to the handler that owns each action, and starts the
// freshness stream. Everything else lives in its own module beside this file.

import {
  accessScreen,
  copyToken,
  issueToken,
  reissueToken,
  revealIssuedToken,
  revokeToken,
  ungrant,
} from "./agents.mjs";
import { api } from "./api.mjs";
import { connectShow, connectSubmit, connectScreen } from "./connect.mjs";
import { artifactRoute, pickVersion, toggleVersionMenu } from "./artifacts.mjs";
import { main, stale } from "./dom.mjs";
import { refreshBadge, startStream } from "./events.mjs";
import { toggleKind } from "./feed.mjs";
import {
  wikiCommentResolve,
  wikiForgetHistory,
  wikiHistoryEarlier,
  wikiInstruction,
  wikiNew,
  wikiPromote,
  wikiReview,
  wikiRevert,
  wikiSave,
  wikiVersionsEarlier,
} from "./wiki.mjs";
import { home } from "./home.mjs";
import { answer, approve, inbox } from "./inbox.mjs";
import { installKeys } from "./keys.mjs";
import { moreScreen } from "./more.mjs";
import { savePrefs } from "./prefs.mjs";
import { projectFromHash, projectScreen } from "./project.mjs";
import { projectSettingsScreen } from "./project-settings.mjs";
import { render, setScreens } from "./router.mjs";
import { searchScreen } from "./search.mjs";
import { endSession, pruneSession, sessionInProject } from "./sessions.mjs";
import { deleteProject, enableNotifications, settingsScreen, signOut } from "./settings.mjs";
import { installShell } from "./shell.mjs";
import { pruneProject, storageScreen } from "./storage.mjs";
import { toast } from "./toast.mjs";

// The legacy per-project addresses feed, sessions and artifacts now live at
// `#/projects/<id>/<segment>`. A hash that names one redirects so a saved
// link keeps working; a bare one lands on the first project. The move waits on
// the projects, so a reader who has gone elsewhere by then is left where they
// are.
const toFirstProject = (segment, gen) => async () => {
  const { projects } = await api("/api/v1/projects");
  if (stale(gen)) return;
  const target = projects.length
    ? `#/projects/${encodeURIComponent(projects[0].id)}/${segment}`
    : "#/projects";
  location.hash = target;
};

setScreens({
  home: (params, gen) => home(gen),
  inbox: (params, gen) => inbox(gen),
  projects: (params, gen, path) =>
    path.split("/")[3] === "settings" ? projectSettingsScreen(gen, path) : projectScreen(params, gen, path),
  feed: (params, gen) => {
    if (params.get("project")) location.hash = `#/projects/${encodeURIComponent(params.get("project"))}/feed`;
    else toFirstProject("feed", gen)();
  },
  sessions: (params, gen) => {
    if (params.get("project")) location.hash = `#/projects/${encodeURIComponent(params.get("project"))}/sessions`;
    else toFirstProject("sessions", gen)();
  },
  // `#/artifacts/<id>` opens the artifact in its project's Artifacts shell.
  artifacts: (params, gen, path) => {
    const segments = (path || "").split("/");
    if (segments[2]) return artifactRoute(params, gen, path);
    else if (params.get("project")) location.hash = `#/projects/${encodeURIComponent(params.get("project"))}/artifacts`;
    else toFirstProject("artifacts", gen)();
  },
  // `#/session?...` is the address older links carry. A session is read
  // inside its project, so the entry resolves the project and forwards to that
  // project's sessions segment instead of painting a bare detail.
  session: (params, gen) => sessionInProject(params, gen),
  search: (params, gen) => searchScreen(params.get("q"), gen),
  storage: (params, gen) => storageScreen(gen),
  settings: (params, gen) => settingsScreen(gen),
  more: (params, gen) => moreScreen(gen),
  access: (params, gen) => accessScreen(gen),
  connect: (params, gen) => connectScreen(params, gen),
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
  if (action === "kind") toggleKind(button.dataset.kind, projectFromHash());
  if (action === "end") acted(button, endSession(id));
  if (action === "prune") acted(button, pruneSession(id, button.dataset.agent));
  if (action === "copy-token") copyToken(button.dataset.token);
  if (action === "more-refresh") acted(button, render());
  if (action === "agent-token") acted(button, reissueToken(id, button.dataset.name));
  if (action === "agent-issue") acted(button, issueToken(id, button.dataset.name));
  if (action === "agent-revoke") acted(button, revokeToken(id, button.dataset.name));
  if (action === "agent-ungrant") acted(button, ungrant(id, button.dataset.project));
  if (action === "project-back") projectBack();
  if (action === "wiki-new") wikiNew(button);
  if (action === "wiki-save") acted(button, wikiSave(button));
  if (action === "wiki-review") acted(button, wikiReview(button));
  if (action === "wiki-comment-resolve") wikiCommentResolve(button);
  if (action === "wiki-history-earlier") wikiHistoryEarlier(button);
  if (action === "wiki-versions-earlier") wikiVersionsEarlier(button);
  if (action === "wiki-revert") wikiRevert(button);
  if (action === "wiki-forget-history") wikiForgetHistory(button);
  if (action === "wiki-promote") wikiPromote(button);
  if (action === "wiki-instruction") wikiInstruction(button);
  if (action === "version-toggle") toggleVersionMenu(button);
  if (action === "version-pick") pickVersion(id, button.dataset.version);
  if (action === "aside-toggle") {
    const panes = main.querySelector(".panes");
    if (panes) {
      panes.classList.toggle("aside-open");
      const open = panes.classList.contains("aside-open");
      button.setAttribute("aria-expanded", String(open));
    }
  }
  if (action === "aside-prune") {
    const projId = button.dataset.project || id;
    const count = Number(button.dataset.sessions || 0);
    const bytes = Number(button.dataset.bytes || 0);
    acted(button, pruneProject({ project_id: projId, prunable_sessions: count, prunable_bytes: bytes }));
  }
  if (action === "project-delete") acted(button, deleteProject(id));
  if (action === "signout") acted(button, signOut());
  if (action === "notification-enable") acted(button, enableNotifications());
});

function projectBack() {
  if (window.history.length > 1) {
    window.history.back();
    return;
  }
  location.hash = "#/projects";
}

// Search forms live in the top bar and on the Search screen, so the submit
// listener is on the document rather than on the screen region alone.
document.addEventListener("submit", (event) => {
  const form = event.target.closest("form[data-action]");
  if (!form || form.dataset.action !== "search") return;
  event.preventDefault();
  const data = new FormData(form);
  location.hash = `#/search?q=${encodeURIComponent(data.get("q") || "")}`;
});

main.addEventListener("submit", (event) => {
  const form = event.target.closest("form[data-action]");
  if (!form || form.dataset.action === "search") return;
  event.preventDefault();
  const data = new FormData(form);
  const action = form.dataset.action;
  if (action === "connect") {
    connectSubmit(form);
  } else if (action === "prefs") {
    savePrefs({
      theme: data.get("theme"),
      density: data.get("density"),
      shortcuts: data.get("shortcuts"),
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
    const payload = {
      id: String(data.get("id") || "").trim(),
      display_name: String(data.get("display_name") || "").trim(),
    };
    api("/api/v1/agents", { method: "POST", body: JSON.stringify(payload) })
      .then(async () => {
        // The token is issued once, now, and the reveal opens on it directly:
        // the copy promises it, and a mobile create that skipped it left the
        // agent with no way in. The agent's own screen is where it lands.
        location.hash = `#/access?agent=${encodeURIComponent(payload.id)}`;
        let issued = null;
        try {
          issued = await api(`/api/v1/agents/${encodeURIComponent(payload.id)}/token`, { method: "POST" });
        } catch {}
        await revealIssuedToken(issued?.token || null, payload.display_name || payload.id, payload.id);
      })
      .catch(failed);
  } else if (action === "agent-grant") {
    const agent = String(data.get("agent") || "").trim();
    const project = String(data.get("project") || "").trim();
    api(`/api/v1/agents/${encodeURIComponent(agent)}/grants`, {
      method: "POST",
      body: JSON.stringify({ project_id: project }),
    })
      .then(() => render())
      .catch(failed);
  }
});

main.addEventListener("change", (event) => {
  if (event.target.dataset.role === "show-token") connectShow(event.target);
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
  // Relative to the document, so the worker's default scope lands on the
  // app root under whatever prefix serves it, not the origin root.
  navigator.serviceWorker.register("sw.js").catch(() => {});
}

installKeys();
installShell();

// The freshness stream nudges a refetch when a write lands; the slow poll is
// the fallback if the stream drops or the browser cannot stream a fetch.
setInterval(() => refreshBadge(), 60000);
startStream();

render();
