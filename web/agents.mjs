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
  // Round 12 §12: Agents and tokens is a rail destination shaped like Settings
  // (RULE 11.16), nothing is opened one at a time, so there is no index. The
  // control row is reserved; it carries the filter field only over eight rows,
  // and with four agents it is empty, which is correct (RULE 11.3).
  const confidentialProjects = projects.filter((p) => p.confidential);

  let latestSeen = null;
  for (const a of agents) {
    const t = a.last_seen_at || a.created_at;
    if (t && (!latestSeen || t > latestSeen)) latestSeen = t;
  }
  const metaText = `${agents.length} ${agents.length === 1 ? "agent" : "agents"}${
    latestSeen ? ` · last call ${relative(latestSeen)}` : " · no calls yet"
  }`;

  const adminRow = `
    <div class="form-row">
      <div class="form-row-main">
        <span class="form-row-title">Admin token</span>
        <span class="form-row-sub">set at startup by <span class="mono">HUB_ADMIN_TOKEN</span></span>
      </div>
      <span class="token-pill pill ok"><span class="pill-dot"></span>live</span>
    </div>`;

  const agentRows = agents
    .map((agent) => {
      const grants = grantsByAgent[agent.id] || [];
      const projectWord = grants.length === 1 ? "project" : "projects";
      const grantDesc = grants.length > 0 ? `write on ${grants.length} ${projectWord}` : "no projects";
      const activeDesc = agent.last_seen_at
        ? `active ${relative(agent.last_seen_at)}`
        : `seen ${relative(agent.created_at)}`;
      return `
      <a class="form-row" href="#/access?agent=${encodeURIComponent(agent.id)}">
        <div class="form-row-main">
          <span class="form-row-title">${esc(agent.display_name || agent.id)}</span>
          <span class="form-row-sub">${esc(grantDesc)} · ${esc(activeDesc)} · <span class="mono">${esc(agent.personal_project_id)}</span></span>
        </div>
        <span class="form-row-chevron">${glyphSvg("chevronRight", { size: 18 })}</span>
      </a>`;
    })
    .join("");

  const filterField =
    agents.length > 8
      ? `<div class="shell-controls"><div class="form-column"><label class="sr-only" for="agents-filter">Filter agents</label><input id="agents-filter" type="search" class="index-filter" placeholder="Filter agents" data-filter="agents"></div></div>`
      : `<div class="shell-controls"></div>`;

  const addAgentBtn = `<button type="button" class="btn-outline agents-add-btn" data-action="toggle-add-agent" style="flex:none;white-space:nowrap;height:32px;display:inline-flex;align-items:center;gap:6px;padding:0 12px 0 9px;border-radius:var(--r-1);border:1px solid var(--line-strong);background:none;color:var(--ink);font:600 13px/1 var(--font-sans);cursor:pointer"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M 12 5v14M 5 12h14"></path></svg>Add agent</button>`;

  const content = `
    <div class="form-column agents-form">
      <div id="desktop-add-agent-form" style="display:none;padding:16px;background:var(--surface);border:1px solid var(--line);border-radius:var(--r-1);margin-bottom:16px">
        <form data-action="agent-create" style="display:flex;flex-direction:column;gap:8px">
          <label for="desktop-agent-id" style="font-size:13px;font-weight:600">Agent id</label>
          <input id="desktop-agent-id" name="id" required autocomplete="off" placeholder="laptop/claude" style="height:36px;padding:0 8px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--bg);color:var(--ink)">
          <label for="desktop-agent-name" style="font-size:13px;font-weight:600">Display name</label>
          <input id="desktop-agent-name" name="display_name" required placeholder="Claude on laptop" style="height:36px;padding:0 8px;border:1px solid var(--line-strong);border-radius:var(--r-1);background:var(--bg);color:var(--ink)">
          <button class="primary" type="submit" style="height:36px;margin-top:4px">Create agent</button>
        </form>
      </div>
      <div class="form-group-label">HUB</div>
      ${adminRow}
      <div class="form-group-label">AGENTS · ${agents.length}</div>
      <div class="agents-list">${agentRows || '<p class="empty">No agents have identified themselves yet.</p>'}</div>
    </div>`;

  return shellHTML({
    segment: "access",
    noIndex: true,
    stageHead: shellStageHead("Agents and tokens", metaText, addAgentBtn, "#/settings"),
    stageControls: filterField,
    stageBody: `<div class="shell-pad">${content}</div>`,
  });
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

function setupAgentsEvents() {
  const root = main.querySelector(".access-screen") || main;
  if (!root || root.dataset.agentsEventsBound === "on") return;
  root.dataset.agentsEventsBound = "on";

  root.addEventListener("click", (event) => {
    const toggleBtn = event.target.closest("[data-action='toggle-add-agent']");
    if (toggleBtn) {
      event.preventDefault();
      const form =
        document.getElementById("desktop-add-agent-form") ||
        document.getElementById("mobile-add-agent-form");
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
    setupAgentsEvents();
  } else {
    const agentParam =
      params?.get("agent") || new URLSearchParams(location.hash.split("?")[1] || "").get("agent");
    if (agentParam) {
      const agent = agents.find((a) => a.id === agentParam);
      if (agent) {
        paint(gen, renderMobileAgentDetail(agent, projects, grantsByAgent[agent.id] || []));
      } else {
        paint(gen, renderMobileAgentsList(agents, projects, grantsByAgent));
        setupAgentsEvents();
      }
    } else {
      paint(gen, renderMobileAgentsList(agents, projects, grantsByAgent));
      setupAgentsEvents();
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

// §13, the reissue reveal. One dialog, two states.
//
// State 1 is the confirmation a destructive action gets: the current token
// stops working now, Cancel or Reissue. State 2 shows the new token once, with
// the copy glyph and Done. Done is the only way out of state 2: Escape and the
// scrim are inert there, because a token that can be dismissed by a stray key
// is a token the operator loses without noticing.
//
// CHECK 12.1.C: the token string exists in the DOM only while this dialog is
// open. The element is removed on close, so nothing of the string survives it,
// rather than being hidden or left in a data attribute.
export function revealIssuedToken(token, agentName = "") {
  const opener = document.activeElement;
  const el = document.createElement("dialog");
  el.className = "dialog dialog-reveal";
  const titleId = "reveal-title";
  el.setAttribute("aria-labelledby", titleId);

  let state = 1;
  let visible = null;

  const form = document.createElement("form");
  form.method = "dialog";

  const heading = document.createElement("h2");
  heading.className = "dialog-title";
  heading.id = titleId;

  const body = document.createElement("p");
  body.className = "dialog-body";

  const note = document.createElement("p");
  note.className = "dialog-note";

  const actions = document.createElement("div");
  actions.className = "dialog-actions";

  const safe = document.createElement("button");
  safe.type = "button";
  safe.className = "dialog-safe";

  const commit = document.createElement("button");
  commit.type = "button";
  commit.className = "dialog-commit danger";

  // State 2 only. Created when the token is revealed and removed on close, so
  // the string is never in the DOM outside the open dialog.
  const fieldWrap = document.createElement("div");
  fieldWrap.className = "dialog-field-wrap";
  fieldWrap.hidden = true;
  const valueRow = document.createElement("div");
  valueRow.className = "reveal-value";
  const value = document.createElement("span");
  value.className = "mono reveal-token";
  const copyBtn = document.createElement("button");
  copyBtn.type = "button";
  copyBtn.className = "reveal-copy";
  copyBtn.setAttribute("aria-label", "Copy the new token");
  // RULE 11.13's two-sheet copy glyph, the same markup Settings uses for the
  // path it owns. The set has no `copy` key: the motif lives where it is used.
  copyBtn.innerHTML = `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="9" width="10" height="10" rx="2"></rect><path d="M15 9V6.5A1.5 1.5 0 0 0 13.5 5h-7A1.5 1.5 0 0 0 5 6.5v7A1.5 1.5 0 0 0 6.5 15H9"></path></svg>`;
  copyBtn.addEventListener("click", () => {
    if (visible) copyToken(visible);
  });
  valueRow.append(value, copyBtn);
  fieldWrap.append(valueRow);

  function paintState(next) {
    state = next;
    const name = agentName || "this agent";
    if (state === 1) {
      heading.textContent = `Reissue the token for ${name}?`;
      body.textContent =
        "The current token stops working now. Anything still using it is refused until it is given the new one.";
      note.textContent = "Reissuing cannot be undone.";
      safe.textContent = "Cancel";
      commit.textContent = "Reissue";
      fieldWrap.hidden = true;
      value.textContent = "";
      body.hidden = false;
    } else {
      heading.textContent = `New token for ${name}`;
      body.textContent = "Copy it now. It is shown once and cannot be read again.";
      body.hidden = false;
      note.textContent = "Done closes this and drops the token from the page.";
      safe.hidden = true;
      commit.textContent = "Done";
      fieldWrap.hidden = false;
      value.textContent = visible || "";
    }
  }

  paintState(1);

  safe.addEventListener("click", () => el.close("cancel"));
  commit.addEventListener("click", async () => {
    if (state === 1) {
      commit.disabled = true;
      try {
        const issued = await api(`/api/v1/agents/${encodeURIComponent(agentName)}/token`, {
          method: "POST",
        });
        visible = issued.token;
        value.textContent = visible;
        commit.disabled = false;
        paintState(2);
        valueRow.querySelector(".reveal-copy")?.focus();
      } catch (error) {
        commit.disabled = false;
        note.textContent = error.message;
        note.dataset.tone = "danger";
      }
      return;
    }
    el.close("done");
  });

  // State 2 is closed by Done alone: Escape and a scrim click are refused.
  el.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && state === 2) event.preventDefault();
  });
  el.addEventListener("cancel", (event) => {
    if (state === 2) event.preventDefault();
  });

  form.append(heading, body, fieldWrap, note, actions);
  actions.append(safe, commit);
  el.appendChild(form);
  document.body.appendChild(el);
  document.documentElement.classList.add("has-dialog");
  el.showModal();
  safe.focus();

  el.addEventListener(
    "close",
    () => {
      // CHECK 12.1.C, kept by removal: the string leaves the DOM here, so it
      // cannot be read out of the page once the dialog is gone.
      visible = null;
      value.textContent = "";
      el.remove();
      document.documentElement.classList.remove("has-dialog");
      if (opener instanceof HTMLElement && opener.isConnected) {
        opener.focus({ preventScroll: true });
      }
      if (typeof render === "function") render();
    },
    { once: true },
  );

  return new Promise((resolve) => {
    el.addEventListener(
      "close",
      () => {
        resolve(el.returnValue === "done");
      },
      { once: true },
    );
  });
}

export async function reissueToken(id) {
  // The dialog does the asking and the issuing: state 1 confirms, state 2
  // reveals, and the string leaves the DOM when it closes (CHECK 12.1.C).
  await revealIssuedToken(null, id);
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
