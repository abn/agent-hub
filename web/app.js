// Agent Hub PWA. A small vanilla single-page app over the REST API.
const main = document.getElementById("main");
const badge = document.getElementById("tab-badge");

const prefs = {
  token: localStorage.getItem("hub.token") || "",
  theme: localStorage.getItem("hub.theme") || "system",
  density: localStorage.getItem("hub.density") || "comfortable",
};

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

function eventRow(event) {
  return `<div class="row ${event.needs_action ? "unread" : ""}">
    ${glyph(event.kind)}
    <div class="grow">
      <div class="title">${esc(event.summary)}</div>
      <div class="meta mono">${esc(event.actor)} · ${when(event.created_at)}</div>
    </div>
  </div>`;
}

function inboxRow(item) {
  const reply =
    item.kind === "question"
      ? `<button type="button" class="action" data-action="answer" data-id="${esc(item.event_id)}">Reply</button>`
      : "";
  return `<div class="row">
    ${glyph(item.kind)}
    <div class="grow">
      <div class="title">${esc(item.summary)}</div>
      <div class="meta">${esc(item.project_id)} · ${esc(item.actor)} · ${esc(item.status)}</div>
    </div>
    ${reply}
  </div>`;
}

function projectToolbar(projects, selected) {
  const options = projects
    .map(
      (p) =>
        `<option value="${esc(p.id)}"${p.id === selected ? " selected" : ""}>${esc(p.display_name)}</option>`,
    )
    .join("");
  return `<div class="toolbar"><label class="sr-only" for="project">Project</label>
    <select id="project" data-role="project">${options}</select>
    <a class="button" href="#/artifacts?project=${encodeURIComponent(selected)}">Artifacts</a></div>`;
}

async function home() {
  const data = await api("/api/v1/home");
  main.innerHTML = `
    <h1>Home</h1>
    <div class="card"><div class="stat">
      <div><div class="n">${data.waiting}</div><div class="l">waiting on you</div></div>
      <div><div class="n">${data.unread}</div><div class="l">unread</div></div>
    </div></div>
    <nav class="toolbar" aria-label="More">
      <a class="chip" href="#/sessions">Sessions</a>
      <a class="chip" href="#/storage">Storage</a>
    </nav>
    <h2>Recent</h2>
    <div class="card">${data.recent.map(eventRow).join("") || '<p class="empty">No events yet.</p>'}</div>`;
}

async function inbox() {
  const { items } = await api("/api/v1/inbox");
  main.innerHTML = `<h1>Inbox</h1>${
    items.length
      ? `<div class="card">${items.map(inboxRow).join("")}</div>`
      : '<p class="empty">Inbox is clear.</p>'
  }`;
}

async function projectsScreen(selected) {
  const { projects } = await api("/api/v1/projects");
  if (!projects.length) {
    main.innerHTML = `<h1>Projects</h1><p class="empty">No projects yet. Create one in Settings.</p>`;
    return;
  }
  const current = selected || projects[0].id;
  const page = await api(`/api/v1/projects/${encodeURIComponent(current)}/feed?limit=50`);
  main.innerHTML = `
    <h1>Project feed</h1>
    ${projectToolbar(projects, current)}
    <div class="card">${page.events.map(eventRow).join("") || '<p class="empty">No events yet.</p>'}</div>`;
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
          <div class="title"><a href="/artifacts/${encodeURIComponent(a.id)}" target="_blank" rel="noopener">${esc(a.title)}</a></div>
          <div class="meta mono">v${a.version} \u00b7 ${a.protected ? "protected" : "public"} \u00b7 ${a.size_bytes} bytes</div>
        </div>
      </div>`,
    )
    .join("");
  main.innerHTML = `
    <h1>Artifacts</h1>
    ${projectToolbar(projects, current)}
    <div class="card">${rows || '<p class="empty">No artifacts yet.</p>'}</div>`;
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
    <div class="card"><div class="stat"><div><div class="n">${mb(usage.total_bytes)}</div><div class="l">used</div></div></div></div>
    <div class="card">${rows || '<p class="empty">Nothing stored yet.</p>'}</div>
    <p class="meta">Session pruning lives in the Sessions screen. You are the garbage collector: no automatic expiry ships.</p>`;
}

async function settingsScreen() {
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
    <p class="meta">Agents and access arrive with the identity layer.</p>`;
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
    else if (screen === "storage") await storageScreen();
    else if (screen === "settings") await settingsScreen();
    else await home();
    refreshBadge();
  } catch (error) {
    main.innerHTML = `<h1>Agent Hub</h1><p class="error">${esc(error.message)}</p>`;
  }
  main.focus({ preventScroll: true });
}

async function refreshBadge() {
  try {
    const data = await api("/api/v1/home");
    const count = data.waiting || 0;
    badge.hidden = count === 0;
    badge.textContent = String(count);
  } catch {
    badge.hidden = true;
  }
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
  if (action === "end") endSession(id).catch((error) => alert(error.message));
  if (action === "prune") pruneSession(id).catch((error) => alert(error.message));
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

render();
