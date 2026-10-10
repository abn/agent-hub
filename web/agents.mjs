import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { errorCard, esc, main, paint } from "./dom.mjs";
import { mcpSetup } from "./feed.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { prefs } from "./prefs.mjs";
import { render } from "./router.mjs";
import { installShellLayout, shellHTML, shellStageHead } from "./shell-layout.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

export function truncateMiddle(val, startLen = 8, endLen = 4) {
  if (!val) return "";
  if (val.length <= startLen + endLen + 1) return val;
  return `${val.slice(0, startLen)}\u2026${val.slice(-endLen)}`;
}

import { registerScreen } from "./keys.mjs";

function formatTokenIssued(ts) {
  if (!ts) return "recently";
  const d = new Date(ts);
  if (isNaN(d.getTime())) return "recently";
  return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

function renderDesktopAgents(agents, projects, grantsByAgent, selectedAgent = null, isCreating = false) {
  // Round 13.1, RULE 13.2 & 13.3: Agents and tokens is a list-and-item screen on desktop.
  // When creating (RULE 13.3):
  // - Add agent button takes aria-pressed="true" and background var(--surface-2)
  // - The index selection clears
  // - Stage 52 header "New agent", 40 control row "It gets its token once, when you create it."
  // - Stage body carries form with Agent id, Display name, Create agent and Cancel.
  // The form's column is the shared 640 every other form is, so round 13's 440
  // no longer holds; the forms gate reads the column from the page.
  const confidentialProjects = projects.filter((p) => p.confidential);
  const confidentialCount = confidentialProjects.length;

  const addAgentBtn = `<button type="button" class="btn-outline agents-add-btn" data-action="toggle-add-agent"${isCreating ? ' aria-pressed="true"' : ""}><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M 12 5v14M 5 12h14"></path></svg>Add agent</button>`;

  const indexHead = `
    <div class="shell-head agents-index-head">
      <b class="shell-title-line agents-index-title">Agents and tokens</b>
      ${addAgentBtn}
    </div>`;

  const indexControls = `
    <div class="shell-controls agents-index-controls">
      <span class="agents-count">${confidentialCount} confidential projects</span>
      ${
        agents.length > 8
          ? `<div class="form-column"><label class="sr-only" for="agents-filter">Filter agents</label><input id="agents-filter" type="search" class="index-filter" placeholder="Filter agents" data-filter="agents"></div>`
          : ""
      }
    </div>`;

  const adminRow = `
    <div class="row agents-admin-row">
      <span class="agents-admin-text">
        <span class="agents-admin-title">Admin token</span>
        <span class="agents-admin-sub">set by <span class="mono agents-env">HUB_ADMIN_TOKEN</span></span>
      </span>
      <span class="token-pill pill ok"><span class="pill-dot"></span>live</span>
    </div>`;

  const agentRows = agents
    .map((agent) => {
      const grants = grantsByAgent[agent.id] || [];
      const projectWord = grants.length === 1 ? "project" : "projects";
      const grantDesc = grants.length > 0 ? `${grants.length} ${projectWord}` : "no projects";
      const activeDesc = agent.last_seen_at
        ? `active ${relative(agent.last_seen_at)}`
        : `seen ${relative(agent.created_at)}`;
      const pending = agent.state === "pending";
      const metaDesc = pending ? `waiting to join · asked ${relative(agent.created_at)}` : `${grantDesc} · ${activeDesc}`;
      const isSelected = !isCreating && selectedAgent && selectedAgent.id === agent.id;
      const currentAttr = isSelected ? ' aria-current="true"' : "";
      return `
        <a class="row agent-index-row" href="#/access?agent=${encodeURIComponent(agent.id)}"${currentAttr}>
          <span class="agent-index-name">
            <span class="agent-index-label">${esc(agent.display_name || agent.id)}</span>
            ${pending ? '<span class="pill agent-pending-pill">pending</span>' : ""}
          </span>
          <span class="agent-index-meta">${esc(metaDesc)}</span>
        </a>`;
    })
    .join("");

  const indexBody = `
    <div class="agents-index">
      <div class="agents-section-label">HUB</div>
      ${adminRow}
      <div class="agents-section-label">AGENTS · ${agents.length}</div>
      <div class="agents-list">${agentRows || '<p class="empty agents-empty-note">No agents have identified themselves yet.</p>'}</div>
    </div>`;

  let stageHead = "";
  let stageControls = "";
  let stageBody = "";

  if (isCreating) {
    stageHead = `
      <div class="shell-head agent-stage-head">
        <b class="shell-title-line agent-stage-title">New agent</b>
      </div>`;

    stageControls = `
      <div class="shell-controls agent-create-controls">
        It gets its token once, when you create it.
      </div>`;

    stageBody = `
      <div class="agent-stage-content agent-create-body">
        <div class="agent-create-column">
          <form class="agent-create-stage-form" data-action="desktop-agent-create">
            <label class="agent-create-field">
              <span class="agent-create-label">Agent id</span>
              <input class="agent-create-input agent-create-id" id="stage-agent-id" name="id" required autocomplete="off" placeholder="deploy-bot-2">
              <span id="stage-agent-id-error" class="field-error agent-create-error" style="display:none"></span>
              <span class="agent-create-hint">Lowercase letters, digits and dashes. Cannot change later.</span>
            </label>
            <label class="agent-create-field">
              <span class="agent-create-label">Display name</span>
              <input class="agent-create-input" id="stage-agent-name" name="display_name" required placeholder="Deploy bot (staging)">
              <span id="stage-agent-name-error" class="field-error agent-create-error" style="display:none"></span>
            </label>
            <div class="agent-create-actions">
              <button class="primary agent-create-submit" type="submit">Create agent</button>
              <button type="button" class="btn-outline agent-create-cancel" data-action="agent-cancel-create">Cancel</button>
            </div>
          </form>
        </div>
      </div>`;
  } else if (selectedAgent) {
    const grants = grantsByAgent[selectedAgent.id] || [];
    const lastCall = selectedAgent.last_seen_at
      ? `last call ${relative(selectedAgent.last_seen_at)}`
      : "no calls yet";
    const stageMeta = `${selectedAgent.id} · ${lastCall}`;
    const issuedDate = formatTokenIssued(selectedAgent.created_at);
    const pending = selectedAgent.state === "pending";

    stageHead = `
      <div class="shell-head agent-stage-head">
        <b class="shell-title-line agent-stage-title">${esc(selectedAgent.display_name || selectedAgent.id)}</b>
        ${pending ? '<span class="pill agent-pending-pill">pending</span>' : ""}
        <span class="agent-stage-meta">${esc(stageMeta)}</span>
      </div>`;

    const liveToken = selectedAgent.has_live_token !== false;

    stageControls = `
      <div class="shell-controls agent-detail-controls">
        <span class="agent-token-note">${liveToken ? `Token issued ${esc(issuedDate)}` : "No live token"}</span>
        <button type="button" class="btn-outline agent-reissue-btn" data-action="${liveToken ? "agent-token" : "agent-issue"}" data-id="${esc(selectedAgent.id)}" data-name="${esc(selectedAgent.display_name || selectedAgent.id)}">${liveToken ? "Reissue token" : "Issue token"}</button>
      </div>`;

    const projectRows = grants.length
      ? grants
          .map((grant) => {
            const proj = projects.find((p) => p.id === grant.project_id);
            const projName = proj?.display_name || grant.project_id;
            const confidential = proj ? proj.confidential === true : true;
            const lead = confidential
              ? `<span class="agent-project-lock" role="img" aria-label="confidential">${glyphSvg("lock", { size: 16 })}</span>`
              : `<span class="agent-project-lead" aria-hidden="true"></span>`;
            return `
              <div class="row agent-project-row">
                ${lead}
                <span class="agent-project-name">${esc(projName)}</span>
                <span class="agent-row-menu">
                  <button type="button" class="hub-btn-glyph" data-action="agent-project-menu" aria-label="More actions for ${esc(projName)}" aria-haspopup="menu" aria-expanded="false">${glyphSvg("overflow", { size: 18 })}</button>
                  <div class="shell-group-menu" role="menu" hidden>
                    <button type="button" role="menuitem" data-action="agent-ungrant" data-id="${esc(selectedAgent.id)}" data-project="${esc(grant.project_id)}">Remove access</button>
                  </div>
                </span>
              </div>`;
          })
          .join("")
      : `<div class="agent-projects-empty">No confidential projects granted. Every public project is open to this agent.</div>`;

    const grantRow = `
      <button type="button" class="agent-grant-row agent-grant-open" data-action="agent-grant-open" data-id="${esc(selectedAgent.id)}" data-name="${esc(selectedAgent.display_name || selectedAgent.id)}">
        <span class="agent-grant-glyph" aria-hidden="true"><svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M 12 5v14M 5 12h14"></path></svg></span>
        Grant a project
      </button>`;

    const enrolReason =
      pending && selectedAgent.enrol_note
        ? `<div class="agent-enrol-reason">
            <span class="agent-enrol-label">Asked to join</span>
            <p class="agent-enrol-note">${esc(selectedAgent.enrol_note)}</p>
          </div>`
        : "";

    stageBody = `
      <div class="agent-stage-content agent-detail-body">
        <div class="agent-detail-column">
          ${enrolReason}
          <div class="agent-detail-label">PROJECTS · ${grants.length}</div>
          ${projectRows}
          ${grantRow}
          <div class="agent-detail-gap"></div>
          ${
            liveToken
              ? `<button type="button" class="agent-revoke-btn" data-action="agent-revoke" data-id="${esc(selectedAgent.id)}" data-name="${esc(selectedAgent.display_name || selectedAgent.id)}">Revoke token</button>`
              : ""
          }
        </div>
      </div>`;
  } else {
    stageHead = `
      <div class="shell-head agent-empty-head">
        <h1 class="shell-title-line agent-stage-title">Agents and tokens</h1>
      </div>`;
    stageControls = `
      <div class="shell-controls agent-empty-controls">
        <span class="shell-meta mono agent-empty-meta">no agent selected</span>
      </div>`;
    stageBody = `
      <div class="shell-pad agent-empty-pad">
        <p class="empty">Select an agent from the list.</p>
      </div>`;
  }

  return shellHTML({
    segment: "access",
    hasSelection: Boolean(selectedAgent) || isCreating,
    indexHead,
    indexControls,
    indexBody,
    stageHead,
    stageControls,
    stageBody,
  });
}

function renderMobileAgentDetail(agent, projects, grants) {
  const pending = agent.state === "pending";
  const meta = `${pending ? "pending enrolment · " : ""}first seen ${relative(agent.created_at)}${agent.last_seen_at ? " · active " + relative(agent.last_seen_at) : ""} · ${agent.personal_project_id}`;
  const stageHead = shellStageHead(agent.display_name || agent.id, meta, "", "#/access");
  const liveToken = agent.has_live_token !== false;
  const stageControls = `
    <div class="shell-controls access-tools">
      ${
        liveToken
          ? `<button type="button" class="btn-hairline access-token-btn" data-action="agent-token" data-id="${esc(agent.id)}" data-name="${esc(agent.display_name || agent.id)}">Reissue token</button>
      <button type="button" class="btn-hairline access-token-btn" data-action="agent-revoke" data-id="${esc(agent.id)}" data-name="${esc(agent.display_name || agent.id)}">Revoke token</button>`
          : `<span class="agent-token-note">No live token</span>
      <button type="button" class="btn-hairline access-token-btn" data-action="agent-issue" data-id="${esc(agent.id)}" data-name="${esc(agent.display_name || agent.id)}">Issue token</button>`
      }
    </div>
  `;
  const grantedIds = new Set(grants.map((grant) => grant.project_id));
  const grantRows = grants.length
    ? grants
        .map((grant) => {
          const proj = projects.find((p) => p.id === grant.project_id);
          const projName = proj?.display_name || grant.project_id;
          const confidential = proj ? proj.confidential === true : true;
          const lead = confidential
            ? `<span class="agent-project-lock" role="img" aria-label="confidential">${glyphSvg("lock", { size: 16 })}</span>`
            : `<span class="agent-project-lead" aria-hidden="true"></span>`;
          return `
        <div class="row access-grant-row">
          ${lead}
          <span class="agent-project-name">${esc(projName)}</span>
          <button type="button" class="btn-hairline access-ungrant-btn" data-action="agent-ungrant" data-id="${esc(agent.id)}" data-project="${esc(grant.project_id)}">Remove access</button>
        </div>
      `;
        })
        .join("")
    : '<div class="agents-empty-note">No confidential projects granted. Every public project is open to this agent.</div>';

  const grantable = projects.filter((p) => p.confidential && !grantedIds.has(p.id));
  const projectOptions = grantable.length
    ? grantable
        .map(
          (p) =>
            `<option value="${esc(p.id)}">${esc(p.display_name && p.display_name !== p.id ? `${p.display_name} (${p.id})` : p.id)}</option>`,
        )
        .join("")
    : '<option value="" disabled>No confidential projects to grant</option>';

  const content = `
    <div class="access-screen">
      ${
        pending && agent.enrol_note
          ? `<div class="agent-enrol-reason">
              <span class="agent-enrol-label">Asked to join</span>
              <p class="agent-enrol-note">${esc(agent.enrol_note)}</p>
            </div>`
          : ""
      }
      <div class="access-section-label">PROJECT GRANTS</div>
      <div class="agent-grants-list">
        ${grantRows}
      </div>
      <div class="access-section-label">ADD GRANT</div>
      <form class="card access-form-card access-grant-form" data-action="agent-grant">
        <input type="hidden" name="agent" value="${esc(agent.id)}">
        <label class="access-field-label" for="grant-project">Project</label>
        <select class="access-field" id="grant-project" name="project" required>
          <option value="" disabled selected>Select project…</option>
          ${projectOptions}
        </select>
        <button class="primary access-grant-submit" type="submit"${grantable.length ? "" : " disabled"}>Grant project</button>
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
    <div class="shell-controls access-tools">
      <span class="access-count">${confidentialCount} confidential projects</span>
      <button type="button" class="btn-outline access-add-btn" data-action="toggle-add-agent">Add agent</button>
    </div>
  `;

  const agentRows = agents
    .map((agent) => {
      const grants = grantsByAgent[agent.id] || [];
      const projectWord = grants.length === 1 ? "project" : "projects";
      const grantDesc = grants.length > 0 ? `${grants.length} ${projectWord}` : "no projects";
      const activeDesc = agent.last_seen_at ? `active ${relative(agent.last_seen_at)}` : `seen ${relative(agent.created_at)}`;
      const pending = agent.state === "pending";
      const metaLine = pending ? `waiting to join · asked ${relative(agent.created_at)}` : `${grantDesc} · ${activeDesc}`;

      return `
      <a class="row agent-row" href="#/access?agent=${encodeURIComponent(agent.id)}">
        <span class="agents-list-text">
          <span class="title agent-row-name">
            <span class="agent-index-label">${esc(agent.display_name || agent.id)}</span>
            ${pending ? '<span class="pill agent-pending-pill">pending</span>' : ""}
          </span>
          <span class="meta agents-admin-sub">${esc(metaLine)}</span>
        </span>
        <span class="agent-row-chevron">${glyphSvg("chevronRight", { size: 18 })}</span>
      </a>
    `;
    })
    .join("");

  const content = `
    <div class="access-screen">
      <div id="mobile-add-agent-form" class="access-add-form" style="display:none">
        <form class="access-add-fields" data-action="agent-create">
          <label class="access-field-label" for="mobile-agent-id">Agent id</label>
          <input class="access-field" id="mobile-agent-id" name="id" required autocomplete="off" placeholder="laptop/claude">
          <label class="access-field-label" for="mobile-agent-name">Display name</label>
          <input class="access-field" id="mobile-agent-name" name="display_name" required placeholder="Claude on laptop">
          <button class="primary access-create-submit" type="submit">Create agent</button>
        </form>
      </div>

      <div class="access-section-label">HUB</div>
      <div class="row agents-list-admin">
        <span class="agents-list-text">
          <span class="agents-list-admin-title">Admin token</span>
          <span class="agents-admin-sub">set at startup by <span class="mono agents-env">HUB_ADMIN_TOKEN</span></span>
        </span>
        <span class="token-pill pill ok"><span class="pill-dot"></span>live</span>
      </div>

      <div class="access-section-label">AGENTS · ${agents.length}</div>
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

let desktopState = null;
let currentGen = 0;

// The trailing overflow on a project row. A menu opened here is closed by the
// document listener installed once, so a render does not stack handlers.
let rowMenusWired = false;
function wireRowMenus() {
  if (rowMenusWired || typeof document === "undefined") return;
  rowMenusWired = true;
  const closeAll = (except) => {
    for (const menu of document.querySelectorAll(".agent-row-menu [role='menu']:not([hidden])")) {
      if (menu === except) continue;
      menu.hidden = true;
      menu
        .closest(".agent-row-menu")
        ?.querySelector("[aria-haspopup='menu']")
        ?.setAttribute("aria-expanded", "false");
    }
  };
  document.addEventListener("click", (event) => {
    const toggle = event.target.closest?.("[data-action='agent-project-menu']");
    if (toggle) {
      const menu = toggle.closest(".agent-row-menu")?.querySelector("[role='menu']");
      const open = !!menu?.hidden;
      closeAll(menu);
      if (menu) {
        if (open) {
          const rect = toggle.getBoundingClientRect();
          menu.style.top = `${Math.round(rect.bottom + 6)}px`;
          menu.style.left = `${Math.round(Math.max(8, Math.min(rect.left, window.innerWidth - 200)))}px`;
        }
        menu.hidden = !open;
        toggle.setAttribute("aria-expanded", String(open));
        if (open) menu.querySelector("button")?.focus();
      }
      return;
    }
    if (!event.target.closest?.(".agent-row-menu")) closeAll();
    else closeAll(event.target.closest(".agent-row-menu")?.querySelector("[role='menu']"));
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeAll();
  });
}

function setupAgentsEvents() {
  const root = main.querySelector(".shell[data-segment='access']") || main.querySelector(".access-screen") || main;
  if (!root || root.dataset.agentsEventsBound === "on") return;
  root.dataset.agentsEventsBound = "on";
  wireRowMenus();

  root.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      const row = document.activeElement?.closest(".agent-index-row");
      if (row) {
        requestAnimationFrame(() => {
          const reissueBtn = main.querySelector('.shell-stage [data-action="agent-token"]') || main.querySelector('.shell-stage');
          if (reissueBtn) reissueBtn.focus();
        });
      }
    } else if (event.key === "Escape") {
      if (desktopState && desktopState.isCreating) {
        event.preventDefault();
        desktopState.isCreating = false;
        paint(currentGen, renderDesktopAgents(
          desktopState.agents,
          desktopState.projects,
          desktopState.grantsByAgent,
          desktopState.selectedAgent,
          false
        ));
        installShellLayout(main);
        setupAgentsEvents();
        const target = desktopState.previousSelected
          ? main.querySelector(`.agent-index-row[href*="${encodeURIComponent(desktopState.previousSelected)}"]`)
          : main.querySelector(".agent-index-row");
        target?.focus();
        return;
      }
      if (document.activeElement?.closest(".shell-stage")) {
        event.preventDefault();
        const selected = main.querySelector('.agent-index-row[aria-current="true"]') || main.querySelector('.agent-index-row');
        if (selected) selected.focus();
      }
    }
  });

  root.addEventListener("click", (event) => {
    const grantBtn = event.target.closest("[data-action='agent-grant-open']");
    if (grantBtn && desktopState) {
      event.preventDefault();
      openGrantPicker(
        grantBtn.dataset.id,
        grantBtn.dataset.name,
        desktopState.projects,
        desktopState.grantsByAgent[grantBtn.dataset.id] || [],
      );
      return;
    }
    const toggleBtn = event.target.closest("[data-action='toggle-add-agent']");
    if (toggleBtn) {
      event.preventDefault();
      if (desktopState && window.matchMedia("(min-width: 720px)").matches) {
        desktopState.isCreating = !desktopState.isCreating;
        if (desktopState.isCreating) {
          desktopState.previousSelected = desktopState.selectedAgent?.id || null;
        }
        paint(currentGen, renderDesktopAgents(
          desktopState.agents,
          desktopState.projects,
          desktopState.grantsByAgent,
          desktopState.isCreating ? null : desktopState.selectedAgent,
          desktopState.isCreating
        ));
        installShellLayout(main);
        setupAgentsEvents();
        if (desktopState.isCreating) {
          requestAnimationFrame(() => {
            document.getElementById("stage-agent-id")?.focus();
          });
        } else {
          const target = desktopState.previousSelected
            ? main.querySelector(`.agent-index-row[href*="${encodeURIComponent(desktopState.previousSelected)}"]`)
            : main.querySelector(".agent-index-row");
          target?.focus();
        }
        return;
      }
      const form = document.getElementById("mobile-add-agent-form");
      if (form) {
        form.style.display = form.style.display === "none" ? "block" : "none";
        if (form.style.display === "block") {
          form.querySelector("input")?.focus();
        }
      }
      return;
    }

    const cancelBtn = event.target.closest("[data-action='agent-cancel-create']");
    if (cancelBtn && desktopState) {
      event.preventDefault();
      desktopState.isCreating = false;
      paint(currentGen, renderDesktopAgents(
        desktopState.agents,
        desktopState.projects,
        desktopState.grantsByAgent,
        desktopState.selectedAgent,
        false
      ));
      installShellLayout(main);
      setupAgentsEvents();
      const target = desktopState.previousSelected
        ? main.querySelector(`.agent-index-row[href*="${encodeURIComponent(desktopState.previousSelected)}"]`)
        : main.querySelector(".agent-index-row");
      target?.focus();
      return;
    }
  });

  root.addEventListener("submit", async (event) => {
    const form = event.target.closest("[data-action='desktop-agent-create']");
    if (!form) return;
    event.preventDefault();

    const idInput = form.querySelector("#stage-agent-id");
    const nameInput = form.querySelector("#stage-agent-name");
    const idError = form.querySelector("#stage-agent-id-error");
    const nameError = form.querySelector("#stage-agent-name-error");

    if (idError) idError.style.display = "none";
    if (nameError) nameError.style.display = "none";

    const idVal = (idInput?.value || "").trim();
    const nameVal = (nameInput?.value || "").trim();

    if (!idVal) {
      if (idError) {
        idError.textContent = "Agent id is required.";
        idError.style.display = "block";
      }
      idInput?.focus();
      return;
    }
    if (!/^[a-z0-9-]+$/.test(idVal)) {
      if (idError) {
        idError.textContent = "Lowercase letters, digits and dashes only.";
        idError.style.display = "block";
      }
      idInput?.focus();
      return;
    }
    if (!nameVal) {
      if (nameError) {
        nameError.textContent = "Display name is required.";
        nameError.style.display = "block";
      }
      nameInput?.focus();
      return;
    }

    try {
      await api("/api/v1/agents", {
        method: "POST",
        body: JSON.stringify({ id: idVal, display_name: nameVal }),
      });
      if (desktopState) {
        desktopState.isCreating = false;
      }
      location.hash = `#/access?agent=${encodeURIComponent(idVal)}`;
      // RULE 13.3: the new agent's token is issued once, now, and B2 opens on
      // it directly. There is nothing to reissue, so no confirm state.
      let issued = null;
      try {
        issued = await api(`/api/v1/agents/${encodeURIComponent(idVal)}/token`, { method: "POST" });
      } catch {}
      revealIssuedToken(issued?.token || null, nameVal || idVal, idVal);
    } catch (err) {
      if (idError) {
        idError.textContent = err.message || "Failed to create agent.";
        idError.style.display = "block";
      }
      idInput?.focus();
    }
  });
}

export async function accessScreen(gen, params = null) {
  currentGen = gen;
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

  const agentParam =
    params?.get("agent") || new URLSearchParams(location.hash.split("?")[1] || "").get("agent");

  const isDesktop = window.matchMedia("(min-width: 720px)").matches;
  if (isDesktop) {
    const selectedAgent = agentParam ? agents.find((a) => a.id === agentParam) : null;
    desktopState = {
      agents,
      projects,
      grantsByAgent,
      selectedAgent,
      isCreating: false,
      previousSelected: selectedAgent?.id || null,
    };
    paint(gen, renderDesktopAgents(agents, projects, grantsByAgent, selectedAgent, false));
    installShellLayout(main);
    setupAgentsEvents();
  } else {
    desktopState = null;
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
        <div class="grow">
          <div class="title">Access</div>
          <div class="meta">A token is an identity of its own. Several agents may share one - a proxy or an aggregator usually does.</div>
        </div>
      </div>
      <p><a class="button access-link" href="#/access">Manage access</a></p>
      <div class="meta access-card-lead">Agents that identified themselves:</div>
      ${agentRows || '<p class="empty">No agents yet.</p>'}
    </div>
  `;
}

export async function copyToken(token, message = "Token copied.") {
  if (navigator.clipboard && navigator.clipboard.writeText) {
    try {
      await navigator.clipboard.writeText(token);
    } catch {}
  }
  toast(message);
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
export function revealIssuedToken(token, agentName = "", agentId = "") {
  const opener = document.activeElement;
  const el = document.createElement("dialog");
  el.className = "dialog dialog-reveal";
  const titleId = "reveal-title";
  el.setAttribute("aria-labelledby", titleId);

  let state = 1;
  // A token passed in (a just-created agent) opens straight at the reveal;
  // otherwise state 1 confirms before issuing.
  let visible = token || null;
  // The setup snippet for the revealed token, built with the token already in
  // place. Held only while the dialog is open, like the token itself.
  let setup = null;

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
  copyBtn.innerHTML = glyphSvg("copy", { size: 18 });
  copyBtn.addEventListener("click", () => {
    if (visible) copyToken(visible);
  });
  valueRow.append(value, copyBtn);
  fieldWrap.append(valueRow);

  // State 2 only, built on paint and cleared on close. The token is embedded,
  // so a reader copying the setup never splices it in by hand.
  const setupWrap = document.createElement("div");
  setupWrap.className = "reveal-setup";
  setupWrap.hidden = true;
  const setupHead = document.createElement("div");
  setupHead.className = "reveal-setup-head";
  const setupLabel = document.createElement("span");
  setupLabel.className = "dialog-label";
  setupLabel.textContent = "MCP setup for this agent";
  const setupCopy = document.createElement("button");
  setupCopy.type = "button";
  setupCopy.className = "reveal-copy";
  setupCopy.setAttribute("aria-label", "Copy setup for this agent");
  setupCopy.innerHTML = glyphSvg("copy", { size: 18 });
  setupCopy.addEventListener("click", () => {
    if (setup) copyToken(setup, "MCP setup copied.");
  });
  const setupText = document.createElement("pre");
  setupText.className = "reveal-setup-text";
  setupHead.append(setupLabel, setupCopy);
  setupWrap.append(setupHead, setupText);

  function paintState(next) {
    state = next;
    const name = agentName || agentId || "this agent";
    if (state === 1) {
      heading.textContent = `Reissue the token for ${name}?`;
      body.textContent =
        "The current token stops working now. Anything still using it is refused until it is given the new one.";
      note.textContent = "Reissuing cannot be undone.";
      safe.textContent = "Cancel";
      commit.textContent = "Reissue";
      fieldWrap.hidden = true;
      value.textContent = "";
      setup = null;
      setupText.textContent = "";
      setupWrap.hidden = true;
      body.hidden = false;
    } else {
      heading.textContent = `Token for ${name}`;
      body.textContent = "Copy it now. It is shown once and cannot be read again.";
      body.hidden = false;
      note.textContent = "Done closes this and drops the token from the page.";
      safe.hidden = true;
      commit.textContent = "Done";
      fieldWrap.hidden = false;
      value.textContent = visible || "";
      setup = visible ? mcpSetup("", visible) : null;
      setupText.textContent = setup || "";
      setupWrap.hidden = !setup;
    }
  }

  paintState(visible ? 2 : 1);

  safe.addEventListener("click", () => el.close("cancel"));
  commit.addEventListener("click", async () => {
    if (state === 1) {
      commit.disabled = true;
      try {
        const issued = await api(`/api/v1/agents/${encodeURIComponent(agentId || agentName)}/token`, {
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

  form.append(heading, body, fieldWrap, setupWrap, note, actions);
  actions.append(safe, commit);
  el.appendChild(form);
  document.body.appendChild(el);
  document.documentElement.classList.add("has-dialog");
  el.showModal();
  if (visible) valueRow.querySelector(".reveal-copy")?.focus();
  else safe.focus();

  el.addEventListener(
    "close",
    () => {
      // CHECK 12.1.C, kept by removal: the string leaves the DOM here, so it
      // cannot be read out of the page once the dialog is gone.
      visible = null;
      value.textContent = "";
      setup = null;
      setupText.textContent = "";
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

export async function reissueToken(id, name = "") {
  // The dialog does the asking and the issuing: state 1 confirms, state 2
  // reveals, and the string leaves the DOM when it closes (CHECK 12.1.C). The
  // id addresses the API; the display name is what the reader sees.
  await revealIssuedToken(null, name || id, id);
}

// The picker a grant opens. Only confidential projects without a grant are
// listed: a public project is already open to every agent, so there is nothing
// to grant. This mirrors the shared dialog's shape (a platform `<dialog>`, the
// safe control focused first, focus kept inside its own ring).
function openGrantPicker(agentId, agentName, projects, grants) {
  const granted = new Set((grants || []).map((grant) => grant.project_id));
  const options = (projects || []).filter((p) => p.confidential && !granted.has(p.id));

  const opener = document.activeElement;
  const el = document.createElement("dialog");
  el.className = "dialog";
  const titleId = "grant-title";
  el.setAttribute("aria-labelledby", titleId);
  el.setAttribute("aria-label", `Grant a project to ${agentName || agentId}`);

  const form = document.createElement("form");
  form.method = "dialog";

  const heading = document.createElement("h2");
  heading.className = "dialog-title";
  heading.id = titleId;
  heading.textContent = "Grant a project";
  form.appendChild(heading);

  const body = document.createElement("p");
  body.className = "dialog-body";

  const problem = document.createElement("p");
  problem.className = "dialog-note";
  problem.hidden = true;

  const actions = document.createElement("div");
  actions.className = "dialog-actions";
  const safe = document.createElement("button");
  safe.type = "button";
  safe.className = "dialog-safe";
  safe.textContent = "Cancel";
  const commit = document.createElement("button");
  commit.type = "button";
  commit.className = "dialog-commit";
  commit.textContent = "Grant";
  commit.disabled = !options.length;
  actions.append(safe, commit);

  let select = null;
  if (options.length) {
    body.textContent =
      "Only confidential projects without a grant are listed. A public project is already open to this agent.";
    const wrap = document.createElement("div");
    wrap.className = "dialog-field-wrap";
    const label = document.createElement("label");
    label.className = "dialog-label";
    label.htmlFor = "grant-project-select";
    label.textContent = "Project";
    select = document.createElement("select");
    select.id = "grant-project-select";
    select.className = "dialog-field grant-picker-field";
    for (const project of options) {
      const option = document.createElement("option");
      option.value = project.id;
      option.textContent =
        project.display_name && project.display_name !== project.id
          ? `${project.display_name} (${project.id})`
          : project.id;
      select.appendChild(option);
    }
    wrap.append(label, select);
    form.append(body, wrap, problem, actions);
  } else {
    body.textContent = "No confidential projects are left to grant. Make a project confidential first.";
    safe.textContent = "Close";
    form.append(body, actions);
  }

  safe.addEventListener("click", () => el.close("cancel"));
  commit.addEventListener("click", async () => {
    if (commit.disabled || !select) return;
    commit.disabled = true;
    problem.hidden = true;
    try {
      await api(`/api/v1/agents/${encodeURIComponent(agentId)}/grants`, {
        method: "POST",
        body: JSON.stringify({ project_id: select.value }),
      });
    } catch (error) {
      problem.textContent = error.message;
      problem.hidden = false;
      commit.disabled = false;
      return;
    }
    el.close("grant");
  });

  el.addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    const ring = [...el.querySelectorAll("select, button:not([disabled])")];
    if (!ring.length) return;
    const edge = event.shiftKey ? ring[0] : ring[ring.length - 1];
    if (document.activeElement !== edge) return;
    event.preventDefault();
    (event.shiftKey ? ring[ring.length - 1] : ring[0]).focus();
  });

  el.appendChild(form);
  document.body.appendChild(el);
  document.documentElement.classList.add("has-dialog");
  el.showModal();
  (select || safe).focus();

  return new Promise((resolve) => {
    el.addEventListener(
      "close",
      () => {
        const done = el.returnValue === "grant";
        el.remove();
        document.documentElement.classList.remove("has-dialog");
        if (opener instanceof HTMLElement && opener.isConnected) {
          opener.focus({ preventScroll: true });
        }
        if (done && typeof render === "function") render();
        resolve(done);
      },
      { once: true },
    );
  });
}

export async function issueToken(id, name = "") {
  // Issuing where there is no live token is not destructive, so the reveal is
  // shown directly rather than behind the reissue confirmation.
  const issued = await api(`/api/v1/agents/${encodeURIComponent(id)}/token`, { method: "POST" });
  await revealIssuedToken(issued.token, name || id, id);
}

export async function revokeToken(id, name = "") {
  const confirmed = await confirmAction({
    title: `Revoke ${name || id}'s token?`,
    body: "Its token stops working now. Its projects stay granted, and a new token restores its access.",
    safe: "Cancel",
    tone: "primary",
    danger: "Revoke token",
    commit: async () => {
      await api(`/api/v1/agents/${encodeURIComponent(id)}/token`, { method: "DELETE" });
      await render();
    },
  });
  if (confirmed) {
    toast(`Token revoked for ${name || id}.`);
  }
}

export async function ungrant(id, project) {
  const confirmed = await confirmAction({
    title: `Remove grant on ${project}?`,
    body: `The agent will lose access to project ${project}.`,
    note: "Removing a grant cannot be undone.",
    safe: "Keep",
    danger: "Remove grant",
    tone: "primary",
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

registerScreen("access", { rows: ".agent-index-row" });
