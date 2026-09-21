// Access: tokens, confidential projects, and agents that identified themselves.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { errorCard, esc, main, paint } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { prefs } from "./prefs.mjs";
import { render } from "./router.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

export function truncateMiddle(val, startLen = 8, endLen = 4) {
  if (!val) return "";
  if (val.length <= startLen + endLen + 1) return val;
  return `${val.slice(0, startLen)}\u2026${val.slice(-endLen)}`;
}

export async function accessScreen(gen) {
  let agents = [];
  let projects = [];
  const grantsByAgent = {};
  try {
    const res = await Promise.all([api("/api/v1/agents"), api("/api/v1/projects")]);
    agents = res[0].agents || [];
    projects = res[1].projects || [];
    await Promise.all(
      agents.map(async (agent) => {
        try {
          grantsByAgent[agent.id] = (
            await api(`/api/v1/agents/${encodeURIComponent(agent.id)}/grants`)
          ).grants || [];
        } catch {
          grantsByAgent[agent.id] = [];
        }
      }),
    );
  } catch (error) {
    paint(gen, errorCard("Access", error));
    return;
  }

  const token = prefs.token || "";
  const tokenTruncated = truncateMiddle(token, 10, 4);
  const confidentialProjects = projects.filter((p) => p.confidential);

  const confidentialRows = confidentialProjects.length
    ? confidentialProjects
        .map(
          (p) => `
      <div class="row confidential-row">
        <span class="confidential-name grow">${esc(p.display_name)} ${glyphSvg("lock", { size: 14 })}</span>
        <span class="meta">confidential</span>
      </div>`,
        )
        .join("")
    : '<p class="empty">No confidential projects.</p>';

  const agentRows = agents.length
    ? agents
        .map((agent) => {
          const grants = grantsByAgent[agent.id] || [];
          const grantRows = grants.length
            ? `<div class="agent-grants">
                ${grants
                  .map(
                    (grant) => `
                  <div class="agent-grant-row">
                    <span class="meta mono">${esc(grant.project_id)} · ${esc(grant.access)}</span>
                    <button type="button" class="btn-hairline danger" data-action="agent-ungrant" data-id="${esc(agent.id)}" data-project="${esc(grant.project_id)}" aria-label="Remove grant on ${esc(grant.project_id)} for ${esc(agent.display_name || agent.id)}">Remove grant</button>
                  </div>`,
                  )
                  .join("")}
              </div>`
            : "";

          return `
      <div class="row agent-record-row" data-agent-id="${esc(agent.id)}">
        <div class="agent-record-main">
          <div class="grow">
            <div class="agent-title-line">
              <span class="title">${esc(agent.display_name || agent.id)}</span>
              <span class="record-badge">record</span>
            </div>
            <div class="meta">first seen ${relative(agent.created_at)}${agent.last_seen_at ? " · active " + relative(agent.last_seen_at) : ""} · <span class="mono">${esc(agent.personal_project_id)}</span></div>
          </div>
          <div class="agent-record-actions">
            <button type="button" class="btn-hairline" data-action="agent-token" data-id="${esc(agent.id)}" aria-label="Reissue token for ${esc(agent.display_name || agent.id)}">Reissue token</button>
            <button type="button" class="btn-hairline danger" data-action="agent-revoke" data-id="${esc(agent.id)}" aria-label="Revoke token for ${esc(agent.display_name || agent.id)}">Revoke token</button>
          </div>
        </div>
        ${grantRows}
      </div>`;
        })
        .join("")
    : '<p class="empty">No agents have identified themselves yet.</p>';

  const agentOptions = agents.length
    ? agents
        .map(
          (a) =>
            `<option value="${esc(a.id)}">${esc(a.display_name && a.display_name !== a.id ? `${a.display_name} (${a.id})` : a.id)}</option>`,
        )
        .join("")
    : '<option value="" disabled>No agents available</option>';

  const projectOptions = projects.length
    ? projects
        .map(
          (p) =>
            `<option value="${esc(p.id)}">${esc(p.display_name && p.display_name !== p.id ? `${p.display_name} (${p.id})` : p.id)}</option>`,
        )
        .join("")
    : '<option value="" disabled>No projects available</option>';

  paint(
    gen,
    `
    <div class="access-header">
      <a class="access-back-btn" href="#/settings" aria-label="Back to settings">
        ${glyphSvg("chevronBack", { size: 20 })}
      </a>
      <h1 class="access-title">Access</h1>
    </div>
    <div class="access-screen">
      <div class="access-section-head">
        <span class="section-label">TOKEN</span>
      </div>
      <div class="card token-card">
        <div class="token-head">
          <span class="token-name">hub-main</span>
          <span class="token-pill pill ok"><span class="pill-dot"></span>live</span>
        </div>
        <button type="button" class="token-copy-btn" data-action="copy-token" data-token="${esc(token)}" aria-label="Copy full token: ${esc(token)}">
          <span class="mono token-val">${esc(tokenTruncated || "no token")}</span>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M 9 9h10v12H9z"/><path d="M 15 9V4H5v14h4"/></svg>
        </button>
        <p class="token-sentence">The admin token comes from the hub's startup configuration. It changes when the operator restarts the hub with a different HUB_ADMIN_TOKEN.</p>
        <p class="token-sentence meta">A token is an identity of its own. Several agents may share one - a proxy or an aggregator usually does.</p>
      </div>

      <div class="access-section-head">
        <span class="section-label">CONFIDENTIAL PROJECTS</span>
        <span class="section-sub">A confidential project is absent, not refused: a token with no grant to it sees no project, no rows, no error.</span>
      </div>
      <div class="card confidential-projects">
        ${confidentialRows}
      </div>

      <div class="access-section-head agents-head">
        <span class="section-label">AGENTS THAT IDENTIFIED THEMSELVES</span>
        <span class="mono agents-count">${agents.length}</span>
      </div>
      <div class="card agents-records">
        ${agentRows}
      </div>

      <div class="access-section-head">
        <span class="section-label">CREATE AGENT</span>
        <span class="section-sub">Register an agent identity and allocate its personal space.</span>
      </div>
      <form class="card access-form-card" data-action="agent-create">
        <label for="agent-id">Agent id</label>
        <input id="agent-id" name="id" required autocomplete="off" placeholder="laptop/claude">
        <label for="agent-name">Display name</label>
        <input id="agent-name" name="display_name" required placeholder="Claude on laptop">
        <p>
          <button class="primary" type="submit">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M 12 5v14 M 5 12h14"/></svg>
            Create agent
          </button>
        </p>
      </form>

      <div class="access-section-head">
        <span class="section-label">GRANT PROJECT</span>
        <span class="section-sub">Grant an agent access to a project.</span>
      </div>
      <form class="card access-form-card" data-action="agent-grant">
        <label for="grant-agent">Agent</label>
        <select id="grant-agent" name="agent" required>
          <option value="" disabled selected>Select agent…</option>
          ${agentOptions}
        </select>
        <label for="grant-project">Project</label>
        <select id="grant-project" name="project" required>
          <option value="" disabled selected>Select project…</option>
          ${projectOptions}
        </select>
        <p>
          <button class="primary" type="submit">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M 12 5v14 M 5 12h14"/></svg>
            Grant project
          </button>
        </p>
      </form>

      <div class="access-section-head">
        <span class="section-label">REVOKED TOKENS</span>
        <span class="section-sub">History, not state.</span>
      </div>
      <div class="card revoked-tokens">
        <p class="empty">No revoked tokens.</p>
      </div>
    </div>
  `,
  );
}

export async function agentsSection() {
  let agents = [];
  try {
    agents = (await api("/api/v1/agents")).agents || [];
  } catch (error) {
    return errorCard("Access", error);
  }

  const agentRows = agents
    .map(
      (agent) => `
    <div class="row agent-record-row">
      <div class="grow">
        <div class="title">${esc(agent.display_name || agent.id)} <span class="record-badge">record</span></div>
        <div class="meta">first seen ${relative(agent.created_at)}${agent.last_seen_at ? " · active " + relative(agent.last_seen_at) : ""} · <span class="mono">${esc(agent.personal_project_id)}</span></div>
      </div>
    </div>`,
    )
    .join("");

  return `
    <div class="card access-card">
      <h2>Access</h2>
      <p class="meta">A token is an identity of its own. Several agents may share one - a proxy or an aggregator usually does.</p>
      <p><a class="button" href="#/access">Manage access</a></p>
      <div class="meta" style="margin-top: var(--s-3); margin-bottom: var(--s-1);">Agents that identified themselves:</div>
      ${agentRows || '<p class="empty">No agents yet.</p>'}
    </div>
  `;
}

export async function copyToken(token) {
  if (navigator.clipboard && navigator.clipboard.writeText) {
    try {
      await navigator.clipboard.writeText(token);
    } catch {}
  }
  toast("Token copied.");
}

export function showToken(token, agentName = "") {
  const existing = document.querySelector(".issued-token-card");
  if (existing) existing.remove();

  const card = document.createElement("div");
  card.className = "card issued-token-card";
  card.setAttribute("role", "status");
  card.setAttribute("aria-live", "polite");

  const title = document.createElement("div");
  title.className = "title";
  title.textContent = agentName ? `New token for ${agentName}, shown once` : "New token, shown once";

  const copyBtn = document.createElement("button");
  copyBtn.type = "button";
  copyBtn.className = "token-copy-btn";
  copyBtn.dataset.action = "copy-token";
  copyBtn.dataset.token = token;
  copyBtn.setAttribute("aria-label", `Copy new token: ${token}`);

  const val = document.createElement("span");
  val.className = "mono token-val";
  val.textContent = token;

  const copySvg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  copySvg.setAttribute("width", "16");
  copySvg.setAttribute("height", "16");
  copySvg.setAttribute("viewBox", "0 0 24 24");
  copySvg.setAttribute("fill", "none");
  copySvg.setAttribute("stroke", "currentColor");
  copySvg.setAttribute("stroke-width", "1.8");
  copySvg.setAttribute("stroke-linecap", "round");
  copySvg.setAttribute("stroke-linejoin", "round");
  copySvg.setAttribute("aria-hidden", "true");
  const path1 = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path1.setAttribute("d", "M 9 9h10v12H9z");
  const path2 = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path2.setAttribute("d", "M 15 9V4H5v14h4");
  copySvg.append(path1, path2);

  copyBtn.append(val, copySvg);
  copyBtn.addEventListener("click", () => copyToken(token));

  const note = document.createElement("p");
  note.className = "token-sentence meta";
  note.textContent = "Copy it now. Reissuing replaces it and revokes the previous token.";

  card.append(title, copyBtn, note);

  const container = document.querySelector(".access-screen") || main;
  container.prepend(card);
  card.scrollIntoView({ behavior: "smooth", block: "start" });
}

export async function reissueToken(id) {
  const issued = await api(`/api/v1/agents/${encodeURIComponent(id)}/token`, {
    method: "POST",
  });
  await render();
  showToken(issued.token, id);
}

export async function revokeToken(id) {
  const confirmed = await confirmAction({
    title: `Revoke the token for ${id}?`,
    body: "The agent loses access immediately. A new token can be issued, but this one is gone.",
    note: "Revoking cannot be undone.",
    safe: "Keep",
    danger: "Revoke token",
    commit: async () => {
      await api(`/api/v1/agents/${encodeURIComponent(id)}/token`, { method: "DELETE" });
      await render();
    },
  });
  if (confirmed) {
    toast(`Token revoked for ${id}.`);
  }
}

export async function ungrant(id, project) {
  const confirmed = await confirmAction({
    title: `Remove grant on ${project}?`,
    body: `The agent will lose access to project ${project}.`,
    note: "Removing a grant cannot be undone.",
    safe: "Keep",
    danger: "Remove grant",
    commit: async () => {
      await api(`/api/v1/agents/${encodeURIComponent(id)}/grants/${encodeURIComponent(project)}`, {
        method: "DELETE",
      });
      await render();
    },
  });
  if (confirmed) {
    toast(`Grant removed on ${project}.`);
  }
}
