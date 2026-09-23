import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { errorCard, esc, main, paint } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { prefs } from "./prefs.mjs";
import { render } from "./router.mjs";
import { shellHTML, shellStageHead } from "./shell-layout.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

export function truncateMiddle(val, startLen = 8, endLen = 4) {
  if (!val) return "";
  if (val.length <= startLen + endLen + 1) return val;
  return `${val.slice(0, startLen)}\u2026${val.slice(-endLen)}`;
}

function renderDesktopAgents(agents, projects, grantsByAgent) {
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

  return `
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
          <span class="token-name">Admin token</span>
          <span class="token-pill pill ok"><span class="pill-dot"></span>live</span>
        </div>
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
  `;
}

function renderMobileAgentDetail(agent, projects, grants) {
  const meta = `first seen ${relative(agent.created_at)}${agent.last_seen_at ? " · active " + relative(agent.last_seen_at) : ""} · ${agent.personal_project_id}`;
  const stageHead = shellStageHead(agent.display_name || agent.id, meta, "", "#/access");
  const stageControls = `
    <div class="shell-controls" style="display:flex;align-items:center;gap:8px;padding:0 16px;height:44px;background:var(--surface);border-bottom:1px solid var(--line);box-sizing:border-box">
      <button type="button" class="btn-hairline" data-action="agent-token" data-id="${esc(agent.id)}" style="height:36px;padding:0 14px;border-radius:var(--r-1);border:1px solid var(--line-strong);background:none;color:var(--ink);font:600 14px/1 var(--font-sans);cursor:pointer">Reissue token</button>
      <button type="button" class="btn-hairline danger" data-action="agent-revoke" data-id="${esc(agent.id)}" style="height:36px;padding:0 14px;border-radius:var(--r-1);border:1px solid var(--danger);background:none;color:var(--danger);font:600 14px/1 var(--font-sans);cursor:pointer">Revoke token</button>
    </div>
  `;
  const grantRows = grants.length
    ? grants
        .map(
          (grant) => `
        <div class="row" style="display:flex;align-items:center;justify-content:space-between;padding:12px 16px;border-bottom:1px solid var(--line);background:var(--surface)">
          <span class="mono" style="font-size:13px;font-family:var(--font-mono)">${esc(grant.project_id)} · ${esc(grant.access)}</span>
          <button type="button" class="btn-hairline danger" data-action="agent-ungrant" data-id="${esc(agent.id)}" data-project="${esc(grant.project_id)}" style="height:32px;padding:0 10px;border-radius:var(--r-1);border:1px solid var(--danger);background:none;color:var(--danger);font:600 12px/1 var(--font-sans);cursor:pointer">Remove grant</button>
        </div>
      `,
        )
        .join("")
    : '<div style="padding:16px;color:var(--ink-3);font-size:13px">No project grants.</div>';

  const projectOptions = projects.length
    ? projects
        .map(
          (p) =>
            `<option value="${esc(p.id)}">${esc(p.display_name && p.display_name !== p.id ? `${p.display_name} (${p.id})` : p.id)}</option>`,
        )
        .join("")
    : '<option value="" disabled>No projects available</option>';

  const content = `
    <div class="access-screen" style="flex:1;min-height:0;overflow:hidden">
      <div style="padding:20px 16px 8px;font:600 12px/1 var(--font-mono);color:var(--ink-3);letter-spacing:.06em">PROJECT GRANTS</div>
      <div class="agent-grants-list" style="border-top:1px solid var(--line)">
        ${grantRows}
      </div>
      <div style="padding:20px 16px 8px;font:600 12px/1 var(--font-mono);color:var(--ink-3);letter-spacing:.06em">ADD GRANT</div>
      <form class="card access-form-card" data-action="agent-grant" style="margin:0 16px;padding:16px;display:flex;flex-direction:column;gap:10px">
        <input type="hidden" name="agent" value="${esc(agent.id)}">
        <label for="grant-project" style="font-size:13px;font-weight:600">Project</label>
        <select id="grant-project" name="project" required style="height:36px;padding:0 8px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--bg);color:var(--ink)">
          <option value="" disabled selected>Select project…</option>
          ${projectOptions}
        </select>
        <button class="primary" type="submit" style="height:36px;align-self:flex-start">Grant project</button>
      </form>
    </div>
  `;

  return shellHTML({
    segment: "access",
    noIndex: true,
    stageHead,
    stageControls,
    stageBody: content,
  });
}

function renderMobileAgentsList(agents, projects, grantsByAgent) {
  let latestSeen = null;
  for (const a of agents) {
    const t = a.last_seen_at || a.created_at;
    if (t && (!latestSeen || t > latestSeen)) latestSeen = t;
  }
  const agentCount = agents.length;
  const metaText = `${agentCount} ${agentCount === 1 ? "agent" : "agents"}${
    latestSeen ? ` · last call ${relative(latestSeen)}` : " · no calls yet"
  }`;

  const confidentialProjects = projects.filter((p) => p.confidential);
  const confidentialCount = confidentialProjects.length;

  const stageHead = shellStageHead("Agents and tokens", metaText, "", "#/more");
  const stageControls = `
    <div class="shell-controls" style="display:flex;align-items:center;gap:8px;padding:0 16px;height:44px;background:var(--surface);border-bottom:1px solid var(--line);box-sizing:border-box">
      <span style="flex:1;min-width:0;font-size:13px;color:var(--ink-2)">${confidentialCount} confidential projects</span>
      <button type="button" class="btn-outline" data-action="toggle-add-agent" style="flex:none;white-space:nowrap;height:36px;display:inline-flex;align-items:center;gap:6px;padding:0 14px;border-radius:var(--r-1);border:1px solid var(--line-strong);background:none;color:var(--ink);font:600 14px/1 var(--font-sans);cursor:pointer">Add agent</button>
    </div>
  `;

  const agentRows = agents
    .map((agent, idx) => {
      const grants = grantsByAgent[agent.id] || [];
      const projectWord = grants.length === 1 ? "project" : "projects";
      const grantDesc = grants.length > 0 ? `write on ${grants.length} ${projectWord}` : "no projects";
      const activeDesc = agent.last_seen_at ? `active ${relative(agent.last_seen_at)}` : `seen ${relative(agent.created_at)}`;
      const metaLine = `${grantDesc} · ${activeDesc}`;

      return `
      <a class="row agent-row" href="#/access?agent=${encodeURIComponent(agent.id)}" style="display:flex;align-items:center;gap:12px;min-height:60px;padding:10px 8px 10px 16px;${idx === 0 ? "border-top:1px solid var(--line);" : ""}border-bottom:1px solid var(--line);background:var(--surface);color:var(--ink);text-decoration:none;box-sizing:border-box">
        <span style="flex:1;min-width:0;display:flex;flex-direction:column;gap:4px">
          <span class="title" style="font-size:15px;font-weight:600;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(agent.display_name || agent.id)}</span>
          <span class="meta" style="font-size:12px;color:var(--ink-3)">${esc(metaLine)}</span>
        </span>
        <span style="flex:none;width:32px;height:44px;display:grid;place-items:center;color:var(--ink-3)">${glyphSvg("chevronRight", { size: 18 })}</span>
      </a>
    `;
    })
    .join("");

  const content = `
    <div class="access-screen" style="flex:1;min-height:0;overflow:hidden">
      <div id="mobile-add-agent-form" style="display:none;padding:16px;background:var(--surface);border-bottom:1px solid var(--line)">
        <form data-action="agent-create" style="display:flex;flex-direction:column;gap:8px">
          <label for="mobile-agent-id" style="font-size:13px;font-weight:600">Agent id</label>
          <input id="mobile-agent-id" name="id" required autocomplete="off" placeholder="laptop/claude" style="height:36px;padding:0 8px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--bg);color:var(--ink)">
          <label for="mobile-agent-name" style="font-size:13px;font-weight:600">Display name</label>
          <input id="mobile-agent-name" name="display_name" required placeholder="Claude on laptop" style="height:36px;padding:0 8px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--bg);color:var(--ink)">
          <button class="primary" type="submit" style="height:36px;margin-top:4px">Create agent</button>
        </form>
      </div>

      <div style="padding:20px 16px 8px;font:600 12px/1 var(--font-mono);color:var(--ink-3);letter-spacing:.06em">HUB</div>
      <div class="row" style="display:flex;align-items:center;gap:12px;min-height:60px;padding:10px 16px;border-top:1px solid var(--line);border-bottom:1px solid var(--line);background:var(--surface);box-sizing:border-box">
        <span style="flex:1;min-width:0;display:flex;flex-direction:column;gap:4px">
          <span style="font-size:15px;font-weight:600">Admin token</span>
          <span style="font-size:12px;color:var(--ink-3)">set at startup by <span class="mono" style="font-family:var(--font-mono)">HUB_ADMIN_TOKEN</span></span>
        </span>
        <span class="token-pill pill ok"><span class="pill-dot"></span>live</span>
      </div>

      <div style="padding:20px 16px 8px;font:600 12px/1 var(--font-mono);color:var(--ink-3);letter-spacing:.06em">AGENTS · ${agents.length}</div>
      <div class="agents-list">
        ${agentRows}
      </div>
    </div>
  `;

  return shellHTML({
    segment: "access",
    noIndex: true,
    stageHead,
    stageControls,
    stageBody: content,
  });
}

function setupMobileAgentsEvents() {
  const root = main.querySelector(".access-screen") || main;
  if (!root || root.dataset.mobileEventsBound === "on") return;
  root.dataset.mobileEventsBound = "on";

  root.addEventListener("click", (event) => {
    const toggleBtn = event.target.closest("[data-action='toggle-add-agent']");
    if (toggleBtn) {
      event.preventDefault();
      const form = document.getElementById("mobile-add-agent-form");
      if (form) {
        form.style.display = form.style.display === "none" ? "block" : "none";
        if (form.style.display === "block") {
          form.querySelector("input")?.focus();
        }
      }
    }
  });
}

export async function accessScreen(gen, params = null) {
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

  const isDesktop = window.matchMedia("(min-width: 720px)").matches;
  if (isDesktop) {
    paint(gen, renderDesktopAgents(agents, projects, grantsByAgent));
  } else {
    const agentParam =
      params?.get("agent") || new URLSearchParams(location.hash.split("?")[1] || "").get("agent");
    if (agentParam) {
      const agent = agents.find((a) => a.id === agentParam);
      if (agent) {
        paint(gen, renderMobileAgentDetail(agent, projects, grantsByAgent[agent.id] || []));
      } else {
        paint(gen, renderMobileAgentsList(agents, projects, grantsByAgent));
        setupMobileAgentsEvents();
      }
    } else {
      paint(gen, renderMobileAgentsList(agents, projects, grantsByAgent));
      setupMobileAgentsEvents();
    }
  }
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
      <div class="row access-row">
        <span class="access-glyph">${glyphSvg("idCard", { size: 17 })}</span>
        <div class="grow">
          <div class="title">Access</div>
          <div class="meta">A token is an identity of its own. Several agents may share one - a proxy or an aggregator usually does.</div>
        </div>
      </div>
      <p><a class="button access-link" href="#/access">${glyphSvg("idCard", { size: 17 })} Manage access</a></p>
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
