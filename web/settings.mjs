// Settings: the token, the two design knobs, notifications, project deletion,
// and the agents and access section.

import { agentsSection } from "./agents.mjs";
import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { errorCard, esc, paint } from "./dom.mjs";
import { prefs } from "./prefs.mjs";
import { render } from "./router.mjs";
import { toast } from "./toast.mjs";

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

export async function enableNotifications() {
  if (!("Notification" in window)) return;
  await Notification.requestPermission();
  render();
}

async function projectsSection() {
  let projects;
  try {
    projects = (await api("/api/v1/projects")).projects.filter((project) => !project.owner_agent);
  } catch (error) {
    return errorCard("Delete a project", error);
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

export async function settingsScreen(gen) {
  const agents = await agentsSection();
  const projects = await projectsSection();
  paint(
    gen,
    `
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
      <label for="shortcuts">Single-key shortcuts</label>
      <select id="shortcuts" name="shortcuts">
        <option value="on"${prefs.shortcuts === "on" ? " selected" : ""}>On</option>
        <option value="off"${prefs.shortcuts === "off" ? " selected" : ""}>Off</option>
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
    ${agents}`,
  );
}

// `leave` is for a screen that is about the project itself: once the project is
// gone there is nothing there to repaint, so it says where to go instead.
export async function deleteProject(id, leave) {
  const confirmed = await confirmAction({
    title: `Delete project ${id}?`,
    body: "Its events, artifacts, and session brains are removed for good.",
    note: "Deleting a project cannot be undone.",
    safe: "Keep",
    danger: "Delete project",
  });
  if (!confirmed) return;
  await api(`/api/v1/projects/${encodeURIComponent(id)}`, { method: "DELETE" });
  if (leave) leave();
  else await render();
  toast("Project deleted.");
}
