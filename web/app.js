// Agent Hub PWA. A small vanilla single-page app over the REST API.
import { decrypt } from "./crypto.mjs";

const main = document.getElementById("main");
const badge = document.getElementById("tab-badge");

const prefs = {
  token: localStorage.getItem("hub.token") || "",
  theme: localStorage.getItem("hub.theme") || "system",
  density: localStorage.getItem("hub.density") || "comfortable",
};

// The kind filters are per project, so switching projects does not carry a
// filter across. They are view state, not a saved preference.
const projectFilters = new Map();

// The chip a render should return focus to, set when a filter is toggled.
let pendingFocus = null;

function applyPrefs() {
  const resolved =
    prefs.theme === "system"
      ? matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light"
      : prefs.theme;
  document.documentElement.dataset.theme = resolved;
  document.documentElement.dataset.density = prefs.density;
  document.querySelector('meta[name="theme-color"]').content =
    resolved === "dark" ? "#141311" : "#F5F3EE";
}

function esc(value) {
  return String(value ?? "").replace(
    /[&<>"']/g,
    (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );
}

async function api(path, options = {}) {
  const headers = Object.assign({}, options.headers || {});
  if (prefs.token) headers.Authorization = "Bearer " + prefs.token;
  if (options.body) headers["Content-Type"] = "application/json";
  const response = await fetch(path, Object.assign({}, options, { headers }));
  if (response.status === 401) throw new Error("unauthorized: set a token in Settings");
  if (!response.ok) {
    let detail = response.statusText;
    try {
      const problem = await response.json();
      detail = problem.detail || problem.title || detail;
    } catch {}
    throw new Error(detail);
  }
  return response.status === 204 ? null : response.json();
}

const glyph = (kind) => `<span class="glyph" data-kind="${esc(kind)}" aria-hidden="true">${
  kind === "finished" ? "✓" : kind === "question" ? "?" : kind === "approval" ? "!" : "•"
}</span>`;

const when = (ts) => esc(String(ts).slice(0, 16).replace("T", " "));

const KINDS = ["signal", "finished", "question", "answer", "approval", "artifact", "session"];
const KIND_LABELS = {
  signal: "Updates",
  finished: "Finished",
  question: "Questions",
  answer: "Answers",
  approval: "Approvals",
  artifact: "Artifacts",
  session: "Sessions",
};

// The action a caller can take on an event. A question is answered; an
// approval is a decision. Both are the human's to act on, so the row carries
// the verb rather than only a label.
function actionFor(event) {
  const id = esc(event.event_id || event.id);
  if (event.kind === "question") {
    return `<button type="button" class="action" data-action="answer" data-id="${id}">Reply</button>`;
  }
  if (event.kind === "approval") {
    return `<button type="button" class="action" data-action="approve" data-id="${id}" data-summary="${esc(event.summary)}">Approve</button>`;
  }
  return "";
}

// Whether an event still expects the human. A feed event carries the inbox
// status, so a decided item stops offering its action even though the event
// itself is append-only.
function isOpen(event) {
  return event.inbox_status === "action" || event.inbox_status === "waiting";
}

function eventRow(event) {
  const open = isOpen(event);
  return `<div class="row ${open ? "unread" : ""}">
    ${glyph(event.kind)}
    <div class="grow">
      <div class="title">${esc(event.summary)}</div>
      <div class="meta mono">${esc(event.actor)} · ${when(event.created_at)}</div>
    </div>
    ${open ? actionFor(event) : ""}
  </div>`;
}

function inboxRow(item) {
  const action =
    item.status === "action" || item.status === "waiting" ? actionFor(item) : "";
  return `<div class="row">
    ${glyph(item.kind)}
    <div class="grow">
      <div class="title">${esc(item.summary)}</div>
      <div class="meta">${esc(item.project_id)} · ${esc(item.actor)} · ${esc(item.status)}</div>
    </div>
    ${action}
  </div>`;
}

// A day bucket for grouping: Today, Yesterday, or the date.
function dayOf(ts, now) {
  const date = new Date(ts);
  const start = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const day = new Date(date.getFullYear(), date.getMonth(), date.getDate());
  const diff = Math.round((start - day) / 86400000);
  if (diff <= 0) return "Today";
  if (diff === 1) return "Yesterday";
  return date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

// Group events into day buckets, newest day first, preserving order inside a
// day. The feed and Home both use this, so their shape stays identical.
function byDay(events) {
  const now = new Date();
  const groups = [];
  for (const event of events) {
    const label = dayOf(event.created_at, now);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.events.push(event);
    else groups.push({ label, events: [event] });
  }
  return groups;
}

function groupedEvents(events, row) {
  return byDay(events)
    .map(
      (group) =>
        `<h2 class="day">${esc(group.label)}</h2>
         <div class="card">${group.events.map(row).join("")}</div>`,
    )
    .join("");
}

function kindChips(active) {
  const chips = KINDS.map((kind) => {
    const pressed = active.has(kind) ? "true" : "false";
    return `<button type="button" class="chip" data-action="kind" data-kind="${kind}" aria-pressed="${pressed}">${KIND_LABELS[kind]}</button>`;
  }).join("");
  return `<div class="toolbar" role="group" aria-label="Filter by kind">${chips}</div>`;
}

function projectToolbar(projects, selected) {
  const options = projects
    .map(
      (p) =>
        `<option value="${esc(p.id)}"${p.id === selected ? " selected" : ""}>${esc(p.display_name)}</option>`,
    )
    .join("");
  return `<div class="toolbar"><label class="sr-only" for="project">Project</label>
    <select id="project" data-role="project">${options}</select></div>`;
}

async function home() {
  const data = await api("/api/v1/home");
  main.innerHTML = `
    <h1>Home</h1>
    <div class="card"><div class="counts">
      <a class="count" href="#/inbox"><span class="n">${data.waiting}</span><span class="l">waiting on you</span></a>
      <a class="count" href="#/inbox"><span class="n">${data.unread}</span><span class="l">unread</span></a>
    </div></div>
    <nav class="toolbar" aria-label="More">
      <a class="chip" href="#/sessions">Sessions</a>
      <a class="chip" href="#/storage">Storage</a>
    </nav>
    <h2>Recent</h2>
    ${groupedEvents(data.recent, eventRow) || '<p class="empty">Nothing has happened yet.</p>'}`;
}

async function inbox() {
  const { items } = await api("/api/v1/inbox");
  // The queue is what waits on the human and what they have not read. A
  // resolved or read item has left it; the feed keeps its history.
  const waiting = items.filter((item) => item.status === "action" || item.status === "waiting");
  const unread = items.filter((item) => item.status === "unread");
  const section = (title, rows) =>
    rows.length ? `<h2>${title}</h2><div class="card">${rows.map(inboxRow).join("")}</div>` : "";
  main.innerHTML = `<h1>Inbox</h1>${
    section("Waiting on you", waiting) + section("Unread", unread) ||
    '<p class="empty">Inbox is clear. Finished work, questions, and approvals will land here.</p>'
  }`;
}

async function projectsScreen(selected) {
  const { projects } = await api("/api/v1/projects");
  if (!projects.length) {
    main.innerHTML = `<h1>Projects</h1><p class="empty">No projects yet. Create one in Settings.</p>`;
    return;
  }
  const current = selected || projects[0].id;
  const active = new Set(projectFilters.get(current) || []);
  // The kind filter runs in the query, before the limit, so a chip finds the
  // newest events of its kind rather than only those inside a fetched window.
  const kinds = [...active].map((kind) => `&kinds=${encodeURIComponent(kind)}`).join("");
  const page = await api(`/api/v1/projects/${encodeURIComponent(current)}/feed?limit=100${kinds}`);
  const empty = active.size ? "No events match this filter." : "No events yet.";
  main.innerHTML = `
    <h1>Project feed</h1>
    ${projectToolbar(projects, current)}
    ${kindChips(active)}
    ${groupedEvents(page.events, eventRow) || `<p class="empty">${empty}</p>`}`;
}

async function searchScreen(term) {
  let results = "";
  if (term) {
    const data = await api(`/api/v1/search?q=${encodeURIComponent(term)}`);
    results = data.groups.length
      ? data.groups
          .map(
            (group) =>
              `<h2>${esc(group.kind)}</h2><div class="card">${group.hits
                .map(
                  (hit) =>
                    `<div class="row"><div class="grow"><div class="title">${esc(hit.title || hit.ref_id)}</div><div class="meta">${esc(hit.snippet)}</div><div class="meta mono">${esc(hit.project_id)}</div></div></div>`,
                )
                .join("")}</div>`,
          )
          .join("")
      : '<p class="empty">No matches.</p>';
  }
  main.innerHTML = `
    <h1>Search</h1>
    <form class="toolbar" data-action="search">
      <label class="sr-only" for="q">Search</label>
      <input id="q" name="q" type="search" value="${esc(term || "")}" placeholder="Search your own machine" autocomplete="off">
      <button class="primary" type="submit">Search</button>
    </form>
    ${results}`;
}

async function artifactsScreen(selected) {
  const { projects } = await api("/api/v1/projects");
  if (!projects.length) {
    main.innerHTML = `<h1>Artifacts</h1><p class="empty">No projects yet. Create one in Settings.</p>`;
    return;
  }
  const current = selected || projects[0].id;
  const { artifacts } = await api(`/api/v1/projects/${encodeURIComponent(current)}/artifacts`);
  const rows = artifacts
    .map(
      (a) => `<div class="row">
        ${glyph("artifact")}
        <div class="grow">
          <div class="title">${esc(a.title)}</div>
          <div class="meta mono">v${a.version} \u00b7 ${a.protected ? "protected" : "public"} \u00b7 ${a.size_bytes} bytes</div>
        </div>
        <button type="button" data-action="artifact-open" data-id="${esc(a.id)}" aria-label="Open ${esc(a.title)}">Open</button>
      </div>`,
    )
    .join("");
  main.innerHTML = `
    <h1>Artifacts</h1>
    ${projectToolbar(projects, current)}
    <div class="card">${rows || '<p class="empty">This project has no artifacts yet. An agent publishing one will show it here.</p>'}</div>`;
}

async function sessionsScreen(selected) {
  const { projects } = await api("/api/v1/projects");
  if (!projects.length) {
    main.innerHTML = `<h1>Sessions</h1><p class="empty">No projects yet.</p>`;
    return;
  }
  const current = selected || projects[0].id;
  const { sessions } = await api(`/api/v1/sessions?project=${encodeURIComponent(current)}`);
  const rows = sessions
    .map(
      (s) => `<div class="row">
        ${glyph("session")}
        <div class="grow">
          <div class="title">${esc(s.session_name)}</div>
          <div class="meta mono">${esc(s.agent)} · ${esc(s.status)} · ${when(s.last_activity)}</div>
        </div>
        <a class="button" href="#/session?project=${encodeURIComponent(current)}&id=${esc(s.id)}" aria-label="Open ${esc(s.session_name)}">Open</a>
        ${
          s.status === "ended"
            ? `<button type="button" class="danger" data-action="prune" data-id="${esc(s.id)}">Prune</button>`
            : `<button type="button" data-action="end" data-id="${esc(s.id)}">End</button>`
        }
      </div>`,
    )
    .join("");
  main.innerHTML = `
    <h1>Sessions</h1>
    ${projectToolbar(projects, current)}
    <div class="card">${rows || '<p class="empty">No sessions yet.</p>'}</div>`;
}

async function sessionDetail(project, id) {
  const { projects } = await api("/api/v1/projects");
  const current = project || (projects[0] && projects[0].id);
  if (!current || !id) {
    location.hash = "#/sessions";
    return;
  }
  const { sessions } = await api(`/api/v1/sessions?project=${encodeURIComponent(current)}`);
  const session = sessions.find((item) => item.id === id);
  if (!session) {
    location.hash = "#/sessions";
    return;
  }
  const query = encodeURIComponent(id);
  const kv = await api(`/api/v1/sessions/${query}/brain?path=${encodeURIComponent("/kv")}`);
  const fs = await api(`/api/v1/sessions/${query}/brain?path=${encodeURIComponent("/fs")}`);
  const action =
    session.status === "ended"
      ? `<button type="button" class="danger" data-action="prune" data-id="${esc(session.id)}">Prune</button>`
      : `<button type="button" data-action="end" data-id="${esc(session.id)}">End</button>`;
  main.innerHTML = `
    <p class="meta"><a href="#/sessions?project=${encodeURIComponent(current)}">Back to sessions</a></p>
    <h1>${esc(session.session_name)}</h1>
    <div class="card">
      <div class="row"><div class="grow"><div class="meta">Project</div><div class="title mono">${esc(session.project_id)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Agent</div><div class="title mono">${esc(session.agent)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Status</div><div class="title">${esc(session.status)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Started</div><div class="title mono">${when(session.created_at)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Last activity</div><div class="title mono">${when(session.last_activity)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Brain file</div><div class="title mono">${esc(session.brain_path)}</div></div></div>
      <div class="row"><div class="grow"><div class="meta">Session id</div><div class="title mono">${esc(session.id)}</div></div>${action}</div>
    </div>
    ${brainTree("Keys", kv.entries)}
    ${brainTree("Files", fs.entries)}`;
}

function brainTree(label, entries) {
  const rows = entries
    .map(
      (entry) => `<div class="row"><div class="grow"><div class="title mono">${esc(entry)}</div></div></div>`,
    )
    .join("");
  return `<h2>${label}</h2><div class="card">${rows || '<p class="empty">Empty.</p>'}</div>`;
}

async function storageScreen() {
  const usage = await api("/api/v1/storage");
  const mb = (bytes) => (bytes / (1024 * 1024)).toFixed(2) + " MB";
  const rows = usage.projects
    .map(
      (p) => `<div class="row"><div class="grow"><div class="title">${esc(p.project_id)}</div>
        <div class="meta mono">artifacts ${mb(p.artifact_bytes)} · sessions ${mb(p.session_bytes)}</div></div></div>`,
    )
    .join("");
  main.innerHTML = `
    <h1>Storage</h1>
    ${
      rows
        ? `<div class="card"><div class="counts"><div class="count"><span class="n">${mb(usage.total_bytes)}</span><span class="l">used</span></div></div></div>
           <div class="card">${rows}</div>`
        : '<p class="empty">Nothing is stored yet. Session brains and artifact blobs appear here as agents work.</p>'
    }
    <p class="meta">Session pruning lives in the Sessions screen. You are the garbage collector: no automatic expiry ships.</p>`;
}

function notificationsSection() {
  if (!("Notification" in window) || !("serviceWorker" in navigator)) {
    return `<div class="card"><h2>Notifications</h2><p class="meta">This browser does not support notifications. The badge and inbox still show what is waiting.</p></div>`;
  }
  if (Notification.permission === "granted") {
    return `<div class="card"><h2>Notifications</h2><p class="meta">Notifications are on for work waiting on you.</p></div>`;
  }
  if (Notification.permission === "denied") {
    return `<div class="card"><h2>Notifications</h2><p class="meta">Notifications are blocked in the browser settings. The badge and inbox still show what is waiting.</p></div>`;
  }
  return `<div class="card"><h2>Notifications</h2>
    <p class="meta">Notify me when work starts waiting on you and the app is in the background. Unread on its own stays quiet.</p>
    <p><button type="button" data-action="notification-enable">Enable notifications</button></p></div>`;
}

async function enableNotifications() {
  if (!("Notification" in window)) return;
  await Notification.requestPermission();
  render();
}

async function settingsScreen() {
  const agents = await agentsSection();
  const projects = await projectsSection();
  main.innerHTML = `
    <h1>Settings</h1>
    <form class="card" data-action="prefs">
      <label for="token">Control-surface token</label>
      <input id="token" name="token" type="password" value="${esc(prefs.token)}" autocomplete="off">
      <label for="theme">Theme</label>
      <select id="theme" name="theme">
        <option value="system"${prefs.theme === "system" ? " selected" : ""}>System</option>
        <option value="light"${prefs.theme === "light" ? " selected" : ""}>Light</option>
        <option value="dark"${prefs.theme === "dark" ? " selected" : ""}>Dark</option>
      </select>
      <label for="density">Density</label>
      <select id="density" name="density">
        <option value="comfortable"${prefs.density === "comfortable" ? " selected" : ""}>Comfortable</option>
        <option value="compact"${prefs.density === "compact" ? " selected" : ""}>Compact</option>
      </select>
      <p><button class="primary" type="submit">Save</button></p>
    </form>
    <form class="card" data-action="project">
      <h2>New project</h2>
      <label for="id">Slug</label>
      <input id="id" name="id" pattern="[A-Za-z0-9_-]+" required>
      <label for="display_name">Display name</label>
      <input id="display_name" name="display_name" required>
      <p><button class="primary" type="submit">Create</button></p>
    </form>
    ${notificationsSection()}
    ${projects}
    ${agents}`;
}

async function projectsSection() {
  let projects;
  try {
    projects = (await api("/api/v1/projects")).projects.filter((project) => !project.owner_agent);
  } catch (error) {
    return `<div class="card"><h2>Delete a project</h2><p class="meta">${esc(error.message)}</p></div>`;
  }
  const rows = projects
    .map(
      (project) => `<div class="row">
        <div class="grow">
          <div class="title">${esc(project.display_name)}</div>
          <div class="meta mono">${esc(project.id)}</div>
        </div>
        <button type="button" class="danger" data-action="project-delete" data-id="${esc(project.id)}" aria-label="Delete ${esc(project.display_name)}">Delete</button>
      </div>`,
    )
    .join("");
  return `<div class="card">
    <h2>Delete a project</h2>
    <p class="meta">Deleting a project removes its events, artifacts, and session brains for good. Agent personal spaces are kept here.</p>
    ${rows || '<p class="empty">No projects can be deleted.</p>'}
  </div>`;
}

async function agentsSection() {
  let agents;
  try {
    agents = (await api("/api/v1/agents")).agents;
  } catch (error) {
    return `<div class="card"><h2>Agents and access</h2><p class="meta">${esc(error.message)}</p></div>`;
  }

  const grantsByAgent = {};
  await Promise.all(
    agents.map(async (agent) => {
      try {
        grantsByAgent[agent.id] = (
          await api(`/api/v1/agents/${encodeURIComponent(agent.id)}/grants`)
        ).grants;
      } catch {
        grantsByAgent[agent.id] = [];
      }
    }),
  );

  const rows = agents
    .map((agent) => {
      const grants = grantsByAgent[agent.id] || [];
      const grantRows = grants
        .map(
          (grant) =>
            `<div class="meta mono">${esc(grant.project_id)} \u00b7 ${esc(grant.access)} ` +
            `<button type="button" data-action="agent-ungrant" data-id="${esc(agent.id)}" data-project="${esc(grant.project_id)}" aria-label="Remove grant on ${esc(grant.project_id)}">Remove</button></div>`,
        )
        .join("");
      const promote = agent.trust === "trusted" ? "untrusted" : "trusted";
      return `<div class="row">
        <div class="grow">
          <div class="title">${esc(agent.display_name)} <span class="pill">${esc(agent.trust)}</span></div>
          <div class="meta mono">${esc(agent.id)} \u00b7 ${esc(agent.personal_project_id)}</div>
          <div class="toolbar">
            <button type="button" data-action="agent-trust" data-id="${esc(agent.id)}" data-trust="${promote}" aria-label="${agent.trust === "trusted" ? "Demote" : "Promote"} ${esc(agent.display_name)}">${agent.trust === "trusted" ? "Demote" : "Promote"}</button>
            <button type="button" data-action="agent-token" data-id="${esc(agent.id)}" aria-label="Reissue token for ${esc(agent.display_name)}">Reissue token</button>
            <button type="button" class="danger" data-action="agent-revoke" data-id="${esc(agent.id)}" aria-label="Revoke token for ${esc(agent.display_name)}">Revoke token</button>
          </div>
          <div class="meta">Grants</div>
          ${grantRows || '<div class="meta">None.</div>'}
          <form data-action="agent-grant">
            <input type="hidden" name="agent" value="${esc(agent.id)}">
            <label class="sr-only" for="grant-project-${esc(agent.id)}">Project</label>
            <input id="grant-project-${esc(agent.id)}" name="project" required placeholder="project id">
            <label class="sr-only" for="grant-access-${esc(agent.id)}">Access</label>
            <select id="grant-access-${esc(agent.id)}" name="access">
              <option value="read">read</option>
              <option value="write">write</option>
            </select>
            <p><button class="primary" type="submit">Add grant</button></p>
          </form>
        </div>
      </div>`;
    })
    .join("");

  return `<div class="card">
    <h2>Agents and access</h2>
    <form data-action="agent-create">
      <label for="agent-id">Agent id</label>
      <input id="agent-id" name="id" required autocomplete="off" placeholder="laptop/claude">
      <label for="agent-name">Display name</label>
      <input id="agent-name" name="display_name" required>
      <label for="agent-trust">Trust</label>
      <select id="agent-trust" name="trust">
        <option value="">deployment default</option>
        <option value="trusted">trusted</option>
        <option value="untrusted">untrusted</option>
      </select>
      <p><button class="primary" type="submit">Create agent</button></p>
    </form>
    ${rows || '<p class="empty">No agents yet.</p>'}
  </div>`;
}

function showToken(token) {
  const card = document.createElement("div");
  card.className = "card";
  card.setAttribute("role", "status");
  card.setAttribute("aria-live", "polite");
  const label = document.createElement("p");
  label.className = "title";
  label.textContent = "New token, shown once";
  const code = document.createElement("p");
  code.className = "token";
  code.textContent = token;
  const note = document.createElement("p");
  note.className = "meta";
  note.textContent = "Copy it now. Reissuing replaces it and revokes the previous token.";
  card.append(label, code, note);
  main.prepend(card);
  card.scrollIntoView();
}

async function setAgentTrust(id, trust) {
  await api(`/api/v1/agents/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify({ trust }),
  });
  await render();
}

async function reissueToken(id) {
  const issued = await api(`/api/v1/agents/${encodeURIComponent(id)}/token`, {
    method: "POST",
  });
  await render();
  showToken(issued.token);
}

async function revokeToken(id) {
  if (!confirm(`Revoke the token for ${id}? The agent loses access immediately.`)) return;
  await api(`/api/v1/agents/${encodeURIComponent(id)}/token`, { method: "DELETE" });
  await render();
}

async function ungrant(id, project) {
  await api(`/api/v1/agents/${encodeURIComponent(id)}/grants/${encodeURIComponent(project)}`, {
    method: "DELETE",
  });
  await render();
}

async function deleteProject(id) {
  if (
    !confirm(
      `Delete project ${id}? Its events, artifacts, and session brains are removed for good.`,
    )
  )
    return;
  await api(`/api/v1/projects/${encodeURIComponent(id)}`, { method: "DELETE" });
  toast("Project deleted.");
  render();
}

async function openArtifact(id) {
  const meta = await api(`/api/v1/artifacts/${encodeURIComponent(id)}`);
  let content = meta.content;
  let decrypted = false;
  if (meta.protected) {
    const password = prompt("Password for this artifact");
    if (!password) return;
    try {
      content = await decrypt(password, meta.envelope, meta.content);
    } catch {
      throw new Error("could not decrypt: check the password");
    }
    decrypted = true;
  }
  showArtifact(meta, content, decrypted);
}

function showArtifact(meta, content, decrypted) {
  main.innerHTML = "";
  const back = document.createElement("p");
  const link = document.createElement("a");
  link.href = "#/artifacts";
  link.textContent = "Back to artifacts";
  back.appendChild(link);
  const heading = document.createElement("h1");
  heading.textContent = meta.title;
  const note = document.createElement("p");
  note.className = "meta";
  note.textContent = meta.protected
    ? decrypted
      ? "Decrypted in your browser. The server never held the plaintext."
      : "Protected."
    : "Public artifact.";
  main.append(back, heading, note);

  // Agent-authored HTML is sandboxed; markdown is rendered by the hub and the
  // result is sandboxed the same way. A protected markdown artifact has no
  // server-rendered HTML, so its decrypted source stays plain text.
  const framed =
    meta.kind === "html" ? content : meta.kind === "markdown" ? meta.rendered : null;
  if (typeof framed === "string" && framed.length) {
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", "");
    frame.setAttribute("title", meta.title);
    frame.style.width = "100%";
    frame.style.height = "60vh";
    frame.style.border = "1px solid var(--line)";
    frame.srcdoc = framed;
    main.appendChild(frame);
  } else {
    const pre = document.createElement("pre");
    pre.className = "card";
    pre.textContent = content;
    main.appendChild(pre);
  }
}

function setCurrent(screen) {
  document.querySelectorAll(".tabbar a, .topbar nav a").forEach((anchor) => {
    const target = (anchor.getAttribute("href") || "").replace(/^#\//, "").split("?")[0];
    if (target === screen) anchor.setAttribute("aria-current", "page");
    else anchor.removeAttribute("aria-current");
  });
  document.title = screen.charAt(0).toUpperCase() + screen.slice(1) + " \u00b7 Agent Hub";
}

async function render() {
  applyPrefs();
  const hash = location.hash.replace(/^#/, "") || "/home";
  const [path, query = ""] = hash.split("?");
  const params = new URLSearchParams(query);
  const screen = path.split("/")[1] || "home";
  setCurrent(screen);
  try {
    if (screen === "inbox") await inbox();
    else if (screen === "feed") await projectsScreen(params.get("project"));
    else if (screen === "search") await searchScreen(params.get("q"));
    else if (screen === "artifacts") await artifactsScreen(params.get("project"));
    else if (screen === "sessions") await sessionsScreen(params.get("project"));
    else if (screen === "session") await sessionDetail(params.get("project"), params.get("id"));
    else if (screen === "storage") await storageScreen();
    else if (screen === "settings") await settingsScreen();
    else await home();
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

async function refreshBadge() {
  try {
    const data = await api("/api/v1/home");
    const count = data.waiting || 0;
    badge.hidden = count === 0;
    badge.textContent = String(count);
    noteWaiting(count);
  } catch {
    badge.hidden = true;
  }
}

let knownWaiting = null;

function noteWaiting(count) {
  if (knownWaiting === null) {
    knownWaiting = count;
    return;
  }
  const grew = count > knownWaiting;
  knownWaiting = count;
  if (grew && document.hidden) showWaitingNotification(count);
}

function showWaitingNotification(count) {
  if (!("Notification" in window) || Notification.permission !== "granted") return;
  if (!("serviceWorker" in navigator)) return;
  navigator.serviceWorker.ready
    .then((registration) => {
      if (registration.active) registration.active.postMessage({ type: "waiting", count });
    })
    .catch(() => {});
}

// Read the server's freshness stream. It carries no event data; each tick just
// refetches the badge. A dropped stream reconnects, and the slow poll covers
// any gap. The loop also waits for a token, so entering one in Settings starts
// the stream without a reload.
let streamRunning = false;
async function startStream() {
  if (streamRunning) return;
  if (!prefs.token) {
    setTimeout(startStream, 15000);
    return;
  }
  streamRunning = true;
  try {
    const response = await fetch("/api/v1/stream", {
      headers: { Authorization: "Bearer " + prefs.token, Accept: "text/event-stream" },
    });
    if (!response.ok || !response.body) throw new Error("stream unavailable");
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const frames = buffer.split("\n\n");
      buffer = frames.pop();
      if (frames.some((frame) => frame.includes("event: tick"))) refreshBadge();
    }
  } catch {}
  streamRunning = false;
  setTimeout(startStream, 15000);
}

async function answer(id) {
  const body = prompt("Your answer");
  if (!body) return;
  await api(`/api/v1/questions/${encodeURIComponent(id)}/answer`, {
    method: "POST",
    body: JSON.stringify({ body }),
  });
  render();
}

// An approval is a decision. It is recorded on the feed and leaves the waiting
// queue, so the confirm names what is approved and the toast states the result.
async function approve(id, summary) {
  const named = summary ? `"${summary}"` : "this action";
  const message = `Approve ${named}? Your decision is recorded on the feed and resolves the waiting item.`;
  if (!confirm(message)) return;
  await api(`/api/v1/approvals/${encodeURIComponent(id)}/decision`, {
    method: "POST",
    body: JSON.stringify({ decision: "approve" }),
  });
  toast("Approved, recorded on the feed.");
  render();
}

// The kind chips are per project. The toggle keeps focus on the chip it
// pressed rather than dropping a keyboard user back at the top of the page.
function toggleKind(kind, projectId) {
  if (!projectId) return;
  const active = new Set(projectFilters.get(projectId) || []);
  if (active.has(kind)) active.delete(kind);
  else active.add(kind);
  projectFilters.set(projectId, [...active]);
  pendingFocus = kind;
  render();
}

async function endSession(id) {
  await api(`/api/v1/sessions/${encodeURIComponent(id)}/end`, { method: "POST" });
  render();
}

function toast(message, undo) {
  const el = document.createElement("div");
  el.className = "card";
  el.style.position = "fixed";
  el.style.left = "var(--s-4)";
  el.style.right = "var(--s-4)";
  el.style.bottom = "72px";
  el.style.zIndex = "20";
  const row = document.createElement("div");
  row.className = "row";
  row.style.border = "0";
  const text = document.createElement("span");
  text.className = "grow";
  text.textContent = message;
  row.appendChild(text);
  if (undo) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "primary";
    button.textContent = "Undo";
    button.addEventListener("click", () => {
      undo().finally(() => el.remove());
    });
    row.appendChild(button);
  }
  el.appendChild(row);
  document.body.appendChild(el);
  setTimeout(() => el.remove(), undo ? 30000 : 5000);
}

async function pruneSession(id) {
  const token = await api(`/api/v1/storage/sessions/${encodeURIComponent(id)}`, {
    method: "DELETE",
  });
  render();
  toast("Session pruned.", async () => {
    await api(`/api/v1/prune/undo/${encodeURIComponent(token.undo_token)}`, { method: "POST" });
    render();
  });
}

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
  if (action === "artifact-open") openArtifact(id).catch((error) => alert(error.message));
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
    prefs.token = String(data.get("token") || "");
    prefs.theme = String(data.get("theme") || "system");
    prefs.density = String(data.get("density") || "comfortable");
    localStorage.setItem("hub.token", prefs.token);
    localStorage.setItem("hub.theme", prefs.theme);
    localStorage.setItem("hub.density", prefs.density);
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
setInterval(refreshBadge, 60000);
startStream();

render();
