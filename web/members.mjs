import { api } from "./api.mjs";
import { foldSessions, liveCounts, liveWords, participantsText, primeMembers } from "./author.mjs";
import { confirmAction } from "./dialog.mjs";
import { errorCard, esc, main, paint } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { render } from "./router.mjs";
import { installShellLayout, shellHTML, shellStageHead } from "./shell-layout.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

export function truncateMiddle(val, startLen = 8, endLen = 4) {
  if (!val) return "";
  if (val.length <= startLen + endLen + 1) return val;
  return `${val.slice(0, startLen)}…${val.slice(-endLen)}`;
}

import { registerScreen } from "./keys.mjs";

// A member is named like a host: what the hub accepts for a new one.
export const MEMBER_NAME = /^[a-z0-9][a-z0-9._-]{0,62}$/;

// The hub's own address as this browser reaches it, without a trailing slash.
// Resolved against the document, not the origin: a hub served behind a
// path-stripping proxy lives at a prefix, and the origin alone loses it.
export function hubUrl() {
  return new URL(".", document.baseURI).href.replace(/\/+$/, "");
}

const json = (value) => `${JSON.stringify(value, null, 2)}\n`;

// What a machine needs to reach this hub as one member, in the places it can
// live. The first group goes on that machine only and holds the token. The
// other two go in a repository and hold none, because a project file is shared
// and a token in it would be too: one for a machine with the agent-hub binary,
// which finds the token in the store, and one for a machine without it, whose
// entries read the token from the environment. Pure, so the words and the
// split are held by a unit test rather than read off a dialog.
export function memberSetup({ url, member, token }) {
  const base = String(url || "").replace(/\/+$/, "");
  return [
    {
      id: "store",
      title: "Store on that machine",
      hint: "These hold the token. Keep them out of any repository.",
      snippets: [
        {
          id: "credential",
          label: "config.toml entry",
          text: `# ~/.config/agent-hub/config.toml\n[[credential]]\nurl = "${base}"\nmember = "${member}"\ntoken = "${token}"\n`,
        },
        {
          id: "env",
          label: "environment",
          text: `export HUB_URL=${base}\nexport HUB_TOKEN=${token}\n`,
        },
      ],
    },
    {
      id: "binary",
      title: "With the agent-hub binary",
      hint: "Safe to commit, no token in it. The binary finds the token in the machine's store.",
      snippets: [
        {
          id: "project-file",
          label: ".agent-hub.toml",
          text: `project = "<project>"\nurl = "${base}"\nmember = "${member}"\n`,
        },
        {
          id: "claude",
          label: "Claude Code .mcp.json",
          text: json({ mcpServers: { "agent-hub": { command: "agent-hub", args: ["mcp"] } } }),
        },
        {
          id: "opencode",
          label: "OpenCode opencode.jsonc",
          text: json({ mcp: { "agent-hub": { type: "local", command: ["agent-hub", "mcp"] } } }),
        },
        {
          id: "antigravity",
          label: "Antigravity mcp_config.json",
          text: json({ mcpServers: { "agent-hub": { command: "agent-hub", args: ["mcp"] } } }),
        },
      ],
    },
    {
      id: "env",
      title: "Without it",
      hint: "Safe to commit, no token in it. Each entry reads HUB_TOKEN from the environment, set as above. Antigravity expands no variables and needs the binary.",
      snippets: [
        {
          id: "claude-env",
          label: "Claude Code .mcp.json",
          text: json({
            mcpServers: {
              "agent-hub": { type: "http", url: `${base}/mcp`, headers: { Authorization: "Bearer ${HUB_TOKEN}" } },
            },
          }),
        },
        {
          id: "opencode-env",
          label: "OpenCode opencode.jsonc",
          text: json({
            mcp: {
              "agent-hub": { type: "remote", url: `${base}/mcp`, headers: { Authorization: "Bearer {env:HUB_TOKEN}" } },
            },
          }),
        },
      ],
    },
  ];
}

function formatTokenIssued(ts) {
  if (!ts) return "recently";
  const d = new Date(ts);
  if (isNaN(d.getTime())) return "recently";
  return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

// What the member row says about its live sessions: a count from the hub, or
// the list itself. Either way a number.
const liveCount = (member) =>
  Array.isArray(member.live_sessions) ? member.live_sessions.length : Math.max(0, Math.trunc(Number(member.live_sessions)) || 0);

// The list's second line: pending members say so, the rest count their live
// sessions and when the hub last heard from them.
function memberMeta(member) {
  if (member.state === "pending") {
    return member.created_at ? `asked ${relative(member.created_at)}` : "asked to join";
  }
  const seen = member.last_seen_at
    ? `seen ${relative(member.last_seen_at)}`
    : member.created_at
      ? `added ${relative(member.created_at)}`
      : "never seen";
  return `${liveWords(liveCount(member), member.live_subagents)} · ${seen}`;
}

const PENDING_PILL = `<span class="pill member-pending-pill">waiting to join</span>`;

// What an approval reaches, in the name it is approved under.
export const approveNote = (name) => `Approving covers every agent on ${name}.`;

// The approval card a pending member is decided on. The name is the member's
// to rename before it is approved, and what the machine told the hub about
// itself is read before Approve. Approving admits the member and so every agent
// that runs on it, which the card says in as many words.
export function approvalCardHTML(member) {
  const enrol = member.enrol || {};
  const fact = (label, value) =>
    value
      ? `<div class="member-fact"><dt>${esc(label)}</dt><dd>${esc(value)}</dd></div>`
      : "";
  const asked = fact("First agent", enrol.agent) + fact("System", enrol.os) + fact("Host", enrol.host) + fact("From", enrol.source);
  const reason = enrol.note
    ? `<div class="member-enrol-reason">
        <span class="member-enrol-label">Asked to join</span>
        <p class="member-enrol-note">${esc(enrol.note)}</p>
      </div>`
    : "";
  const id = esc(member.id);
  return `<form class="member-approve" data-action="member-approve" data-id="${id}" aria-labelledby="member-approve-title" novalidate>
    <h2 class="member-detail-label" id="member-approve-title">Approval</h2>
    ${reason}
    <label class="member-create-field">
      <span class="member-create-label">Member name</span>
      <input class="member-create-input member-create-id" name="member" value="${id}" required autocomplete="off" spellcheck="false" aria-describedby="member-approve-hint member-approve-error">
      <span id="member-approve-error" class="field-error member-create-error" style="display:none"></span>
      <span id="member-approve-hint" class="member-create-hint">Rename it before you approve if the host name is not what you want. It cannot change later.</span>
    </label>
    ${asked ? `<dl class="member-facts">${asked}</dl>` : ""}
    <p class="member-approve-note" data-approve-note>${esc(approveNote(member.id))}</p>
    <div class="member-create-actions">
      <button class="primary member-create-submit" type="submit">Approve</button>
      <button type="button" class="btn-outline member-create-cancel" data-action="member-refuse" data-id="${id}">Refuse</button>
    </div>
  </form>`;
}

// The count over a member's live sessions, as the rows below it show them:
// "2", or "2 · 1 subagent" when some are folded under their parents.
function liveHead(sessions) {
  const { sessions: n, subagents } = liveCounts(sessions);
  return subagents ? `${n} · ${subagents} ${subagents === 1 ? "subagent" : "subagents"}` : String(n);
}

// One live session on a member's stage: its name, where it is and which agents
// of this member are in it, with the verb that ends it. A parent carries its
// subagents folded under it.
function liveSessionRows(sessions, memberId) {
  return foldSessions(sessions, { live: true })
    .map(({ session, folded }) => {
      const mine = (session.participants || []).filter((p) => p.member === memberId);
      const who = participantsText(mine, { solo: true });
      const project = session.project_display_name || session.project_id || "";
      const meta = [project, who, folded].filter(Boolean).join(" · ");
      const name = session.session_name || session.id;
      const href = `#/projects/${encodeURIComponent(session.project_id)}/sessions?id=${encodeURIComponent(session.id)}`;
      return `<div class="row member-session-row">
        <span class="member-session-text">
          <a class="member-session-name" href="${esc(href)}">${esc(name)}</a>
          <span class="member-session-meta mono">${esc(meta)}</span>
        </span>
        <button type="button" class="btn-outline member-end-btn" data-action="end" data-id="${esc(session.id)}" data-name="${esc(name)}" aria-label="End ${esc(name)}">End</button>
      </div>`;
    })
    .join("");
}

// The agents a member has run, from every session it took part in and from the
// agent that asked to join.
export function agentsSeen(member, sessions) {
  const seen = new Set();
  const first = member.enrol && member.enrol.agent;
  if (first) seen.add(first);
  for (const session of sessions || []) {
    for (const p of session.participants || []) if (p.member === member.id && p.agent) seen.add(p.agent);
  }
  return [...seen].sort();
}

function memberStageBody(member, projects, grants, sessions, seenSessions) {
  const pending = member.state === "pending";
  const liveToken = member.has_live_token !== false;
  if (pending) {
    return `<div class="member-stage-content member-detail-body">
      <div class="member-detail-column">${approvalCardHTML(member)}</div>
    </div>`;
  }
  const projectRows = grants.length
    ? grants
        .map((grant) => {
          const proj = projects.find((p) => p.id === grant.project_id);
          const projName = proj?.display_name || grant.project_id;
          const confidential = proj ? proj.confidential === true : true;
          const lead = confidential
            ? `<span class="member-project-lock" role="img" aria-label="confidential">${glyphSvg("lock", { size: 16 })}</span>`
            : `<span class="member-project-lead" aria-hidden="true"></span>`;
          return `
              <div class="row member-project-row">
                ${lead}
                <span class="member-project-name">${esc(projName)}</span>
                <span class="member-row-menu">
                  <button type="button" class="hub-btn-glyph" data-action="member-project-menu" aria-label="More actions for ${esc(projName)}" aria-haspopup="menu" aria-expanded="false">${glyphSvg("overflow", { size: 18 })}</button>
                  <div class="shell-group-menu" role="menu" hidden>
                    <button type="button" role="menuitem" data-action="member-ungrant" data-id="${esc(member.id)}" data-project="${esc(grant.project_id)}">Remove access</button>
                  </div>
                </span>
              </div>`;
        })
        .join("")
    : `<div class="member-projects-empty">No confidential projects granted. Every public project is open to this member.</div>`;

  const grantRow = `
      <button type="button" class="member-grant-row member-grant-open" data-action="member-grant-open" data-id="${esc(member.id)}" data-name="${esc(member.display_name || member.id)}">
        <span class="member-grant-glyph" aria-hidden="true"><svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true"><path d="M 12 5v14M 5 12h14"></path></svg></span>
        Grant a project
      </button>`;

  // `sessions` is null when the hub would not list them: that is said, in ink,
  // and is not the empty state.
  const live = sessions ? foldSessions(sessions, { live: true }) : [];
  const liveBody = !sessions
    ? `<div class="member-projects-empty member-list-failed">Could not list live sessions.</div>`
    : live.length
      ? `<div class="member-session-list">${liveSessionRows(sessions, member.id)}</div>`
      : `<div class="member-projects-empty">No live sessions. A session appears here while one of this member's agents is working in it.</div>`;
  const liveBlock = `
          <div class="member-detail-label">Live sessions${sessions ? ` <span class="mono">${liveHead(sessions)}</span>` : ""}</div>
          ${liveBody}`;

  const agents = agentsSeen(member, seenSessions);
  const agentsBlock = `
          <div class="member-detail-label">Agents seen <span class="mono">${agents.length}</span></div>
          ${agents.length ? `<p class="member-agents mono">${esc(agents.join(", "))}</p>` : `<div class="member-projects-empty">No agents seen yet.</div>`}`;

  return `
      <div class="member-stage-content member-detail-body">
        <div class="member-detail-column">
          ${liveBlock}
          <div class="member-detail-gap"></div>
          ${agentsBlock}
          <div class="member-detail-gap"></div>
          <div class="member-detail-label">Projects <span class="mono">${grants.length}</span></div>
          ${projectRows}
          ${grantRow}
          <div class="member-detail-gap"></div>
          ${
            liveToken
              ? `<button type="button" class="member-revoke-btn" data-action="member-revoke" data-id="${esc(member.id)}" data-name="${esc(member.display_name || member.id)}">Revoke token</button>`
              : ""
          }
        </div>
      </div>`;
}

function renderDesktopMembers(members, projects, grantsByMember, selectedMember = null, isCreating = false, detail = {}) {
  // Members and tokens is a list-and-item screen on desktop.
  // When creating:
  // - Add member button takes aria-pressed="true" and background var(--surface-2)
  // - The index selection clears
  // - Stage 52 header "New member", 40 control row "It gets its token once, when you create it."
  // - Stage body carries form with Member name, Display name, Create member and Cancel.
  // The form's column is the shared 640 every other form is, so round 13's 440
  // no longer holds; the forms gate reads the column from the page.
  const confidentialProjects = projects.filter((p) => p.confidential);
  const confidentialCount = confidentialProjects.length;

  const addMemberBtn = `<button type="button" class="btn-outline members-add-btn" data-action="toggle-add-member"${isCreating ? ' aria-pressed="true"' : ""}><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" aria-hidden="true"><path d="M 12 5v14M 5 12h14"></path></svg>Add member</button>`;

  const indexHead = `
    <div class="shell-head members-index-head">
      <b class="shell-title-line members-index-title">Members and tokens</b>
      ${addMemberBtn}
    </div>`;

  const indexControls = `
    <div class="shell-controls members-index-controls">
      <span class="members-count">${confidentialCount} confidential projects</span>
      ${
        members.length > 8
          ? `<div class="form-column"><label class="sr-only" for="members-filter">Filter members</label><input id="members-filter" type="search" class="index-filter" placeholder="Filter members" data-filter="members"></div>`
          : ""
      }
    </div>`;

  const adminRow = `
    <div class="row members-admin-row">
      <span class="members-admin-text">
        <span class="members-admin-title">Admin token</span>
        <span class="members-admin-sub">set by <span class="mono members-env">HUB_ADMIN_TOKEN</span></span>
      </span>
      <span class="token-pill pill ok"><span class="pill-dot"></span>live</span>
    </div>`;

  const memberRows = members
    .map((member) => {
      const pending = member.state === "pending";
      const isSelected = !isCreating && selectedMember && selectedMember.id === member.id;
      const currentAttr = isSelected ? ' aria-current="true"' : "";
      return `
        <a class="row member-index-row" href="#/access?member=${encodeURIComponent(member.id)}"${currentAttr}>
          <span class="member-index-name">
            <span class="member-index-label">${esc(member.display_name || member.id)}</span>
            ${pending ? PENDING_PILL : ""}
          </span>
          <span class="member-index-meta">${esc(memberMeta(member))}</span>
        </a>`;
    })
    .join("");

  const indexBody = `
    <div class="members-index">
      <div class="members-section-label">HUB</div>
      ${adminRow}
      <div class="members-section-label">MEMBERS · ${members.length}</div>
      <div class="members-list">${memberRows || '<p class="empty members-empty-note">No members yet. Add one, or approve a machine that asks to join.</p>'}</div>
    </div>`;

  let stageHead = "";
  let stageControls = "";
  let stageBody = "";

  if (isCreating) {
    stageHead = `
      <div class="shell-head member-stage-head">
        <b class="shell-title-line member-stage-title">New member</b>
      </div>`;

    stageControls = `
      <div class="shell-controls member-create-controls">
        It gets its token once, when you create it.
      </div>`;

    stageBody = `
      <div class="member-stage-content member-create-body">
        <div class="member-create-column">
          <form class="member-create-stage-form" data-action="desktop-member-create">
            <label class="member-create-field">
              <span class="member-create-label">Member name</span>
              <input class="member-create-input member-create-id" id="stage-member-id" name="id" required autocomplete="off" placeholder="laptop">
              <span id="stage-member-id-error" class="field-error member-create-error" style="display:none"></span>
              <span class="member-create-hint">Usually the machine's host name. Lowercase letters, digits, dots and dashes. Cannot change later.</span>
            </label>
            <label class="member-create-field">
              <span class="member-create-label">Display name</span>
              <input class="member-create-input" id="stage-member-name" name="display_name" required placeholder="Work laptop">
              <span id="stage-member-name-error" class="field-error member-create-error" style="display:none"></span>
            </label>
            <div class="member-create-actions">
              <button class="primary member-create-submit" type="submit">Create member</button>
              <button type="button" class="btn-outline member-create-cancel" data-action="member-cancel-create">Cancel</button>
            </div>
          </form>
        </div>
      </div>`;
  } else if (selectedMember) {
    const grants = grantsByMember[selectedMember.id] || [];
    const pending = selectedMember.state === "pending";
    const lastCall = selectedMember.last_seen_at
      ? `last call ${relative(selectedMember.last_seen_at)}`
      : "no calls yet";
    const stageMeta = `${selectedMember.id} · ${lastCall}`;
    // The hub sends when the member was added, not when its token was issued,
    // so the note says what it knows: a token's own time is used when sent.
    const tokenNote = selectedMember.token_issued_at
      ? `Token issued ${formatTokenIssued(selectedMember.token_issued_at)}`
      : `Added ${formatTokenIssued(selectedMember.created_at)} · live token`;

    stageHead = `
      <div class="shell-head member-stage-head">
        <b class="shell-title-line member-stage-title">${esc(selectedMember.display_name || selectedMember.id)}</b>
        ${pending ? PENDING_PILL : ""}
        <span class="member-stage-meta">${esc(stageMeta)}</span>
      </div>`;

    const liveToken = selectedMember.has_live_token !== false;

    stageControls = pending
      ? `<div class="shell-controls member-detail-controls"><span class="member-token-note">Waiting for your decision</span></div>`
      : `
      <div class="shell-controls member-detail-controls">
        <span class="member-token-note">${liveToken ? esc(tokenNote) : "No live token"}</span>
        <button type="button" class="btn-outline member-reissue-btn" data-action="${liveToken ? "member-token" : "member-issue"}" data-id="${esc(selectedMember.id)}" data-name="${esc(selectedMember.display_name || selectedMember.id)}">${liveToken ? "Reissue token" : "Issue token"}</button>
      </div>`;

    stageBody = memberStageBody(selectedMember, projects, grants, detail.live, detail.seen || []);
  } else {
    stageHead = `
      <div class="shell-head member-empty-head">
        <h1 class="shell-title-line member-stage-title">Members and tokens</h1>
      </div>`;
    stageControls = `
      <div class="shell-controls member-empty-controls">
        <span class="shell-meta mono member-empty-meta">no member selected</span>
      </div>`;
    stageBody = `
      <div class="shell-pad member-empty-pad">
        <p class="empty">Select a member from the list.</p>
      </div>`;
  }

  return shellHTML({
    segment: "access",
    hasSelection: Boolean(selectedMember) || isCreating,
    indexHead,
    indexControls,
    indexBody,
    stageHead,
    stageControls,
    stageBody,
  });
}

function renderMobileMemberDetail(member, projects, grants, detail = {}) {
  const pending = member.state === "pending";
  const meta = `${pending ? "waiting to join · " : ""}first seen ${relative(member.created_at)}${member.last_seen_at ? " · active " + relative(member.last_seen_at) : ""}${member.personal_project_id ? ` · ${member.personal_project_id}` : ""}`;
  const stageHead = shellStageHead(member.display_name || member.id, meta, "", "#/access");
  const liveToken = member.has_live_token !== false;
  const stageControls = pending
    ? `<div class="shell-controls access-tools"><span class="member-token-note">Waiting for your decision</span></div>`
    : `
    <div class="shell-controls access-tools">
      ${
        liveToken
          ? `<button type="button" class="btn-hairline access-token-btn" data-action="member-token" data-id="${esc(member.id)}" data-name="${esc(member.display_name || member.id)}">Reissue token</button>
      <button type="button" class="btn-hairline access-token-btn" data-action="member-revoke" data-id="${esc(member.id)}" data-name="${esc(member.display_name || member.id)}">Revoke token</button>`
          : `<span class="member-token-note">No live token</span>
      <button type="button" class="btn-hairline access-token-btn" data-action="member-issue" data-id="${esc(member.id)}" data-name="${esc(member.display_name || member.id)}">Issue token</button>`
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
            ? `<span class="member-project-lock" role="img" aria-label="confidential">${glyphSvg("lock", { size: 16 })}</span>`
            : `<span class="member-project-lead" aria-hidden="true"></span>`;
          return `
        <div class="row access-grant-row">
          ${lead}
          <span class="member-project-name">${esc(projName)}</span>
          <button type="button" class="btn-hairline access-ungrant-btn" data-action="member-ungrant" data-id="${esc(member.id)}" data-project="${esc(grant.project_id)}">Remove access</button>
        </div>
      `;
        })
        .join("")
    : '<div class="members-empty-note">No confidential projects granted. Every public project is open to this member.</div>';

  const grantable = projects.filter((p) => p.confidential && !grantedIds.has(p.id));
  const projectOptions = grantable.length
    ? grantable
        .map(
          (p) =>
            `<option value="${esc(p.id)}">${esc(p.display_name && p.display_name !== p.id ? `${p.display_name} (${p.id})` : p.id)}</option>`,
        )
        .join("")
    : '<option value="" disabled>No confidential projects to grant</option>';

  const liveRows = detail.live;
  const live = liveRows ? foldSessions(liveRows, { live: true }) : [];
  const agents = agentsSeen(member, detail.seen || []);

  const content = pending
    ? `<div class="access-screen">${approvalCardHTML(member)}</div>`
    : `
    <div class="access-screen">
      <div class="access-section-label">LIVE SESSIONS${liveRows ? ` · ${liveHead(liveRows)}` : ""}</div>
      <div class="member-session-list">
        ${!liveRows ? '<div class="members-empty-note member-list-failed">Could not list live sessions.</div>' : live.length ? liveSessionRows(liveRows, member.id) : '<div class="members-empty-note">No live sessions.</div>'}
      </div>
      <div class="access-section-label">AGENTS SEEN · ${agents.length}</div>
      <p class="member-agents mono">${agents.length ? esc(agents.join(", ")) : "none yet"}</p>
      <div class="access-section-label">PROJECT GRANTS</div>
      <div class="member-grants-list">
        ${grantRows}
      </div>
      <div class="access-section-label">ADD GRANT</div>
      <form class="card access-form-card access-grant-form" data-action="member-grant">
        <input type="hidden" name="member" value="${esc(member.id)}">
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

function renderMobileMembersList(members, projects) {
  let latestSeen = null;
  for (const m of members) {
    const t = m.last_seen_at || m.created_at;
    if (t && (!latestSeen || t > latestSeen)) latestSeen = t;
  }
  const memberCount = members.length;
  const metaText = `${memberCount} ${memberCount === 1 ? "member" : "members"}${
    latestSeen ? ` · last call ${relative(latestSeen)}` : " · no calls yet"
  }`;

  const confidentialProjects = projects.filter((p) => p.confidential);
  const confidentialCount = confidentialProjects.length;

  const stageHead = shellStageHead("Members and tokens", metaText, "", "#/more");
  const stageControls = `
    <div class="shell-controls access-tools">
      <span class="access-count">${confidentialCount} confidential projects</span>
      <button type="button" class="btn-outline access-add-btn" data-action="toggle-add-member">Add member</button>
    </div>
  `;

  const memberRows = members
    .map((member) => {
      const pending = member.state === "pending";
      return `
      <a class="row member-row" href="#/access?member=${encodeURIComponent(member.id)}">
        <span class="members-list-text">
          <span class="title member-row-name">
            <span class="member-index-label">${esc(member.display_name || member.id)}</span>
            ${pending ? PENDING_PILL : ""}
          </span>
          <span class="meta members-admin-sub">${esc(memberMeta(member))}</span>
        </span>
        <span class="member-row-chevron">${glyphSvg("chevronRight", { size: 18 })}</span>
      </a>
    `;
    })
    .join("");

  const content = `
    <div class="access-screen">
      <div id="mobile-add-member-form" class="access-add-form" style="display:none">
        <form class="access-add-fields" data-action="member-create">
          <label class="access-field-label" for="mobile-member-id">Member name</label>
          <input class="access-field" id="mobile-member-id" name="id" required autocomplete="off" placeholder="laptop">
          <label class="access-field-label" for="mobile-member-name">Display name</label>
          <input class="access-field" id="mobile-member-name" name="display_name" required placeholder="Work laptop">
          <button class="primary access-create-submit" type="submit">Create member</button>
        </form>
      </div>

      <div class="access-section-label">HUB</div>
      <div class="row members-list-admin">
        <span class="members-list-text">
          <span class="members-list-admin-title">Admin token</span>
          <span class="members-admin-sub">set at startup by <span class="mono members-env">HUB_ADMIN_TOKEN</span></span>
        </span>
        <span class="token-pill pill ok"><span class="pill-dot"></span>live</span>
      </div>

      <div class="access-section-label">MEMBERS · ${members.length}</div>
      <div class="members-list">
        ${memberRows || '<p class="empty members-empty-note">No members yet. Add one, or approve a machine that asks to join.</p>'}
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
    for (const menu of document.querySelectorAll(".member-row-menu [role='menu']:not([hidden])")) {
      if (menu === except) continue;
      /** @type {HTMLElement} */ (menu).hidden = true;
      menu
        .closest(".member-row-menu")
        ?.querySelector("[aria-haspopup='menu']")
        ?.setAttribute("aria-expanded", "false");
    }
  };
  document.addEventListener("click", (event) => {
    const toggle = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest?.("[data-action='member-project-menu']"));
    if (toggle) {
      const menu = /** @type {HTMLElement} */ (toggle.closest(".member-row-menu")?.querySelector("[role='menu']"));
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
    if (!/** @type {HTMLElement} */ (event.target).closest?.(".member-row-menu")) closeAll();
    else closeAll(/** @type {HTMLElement} */ (event.target).closest(".member-row-menu")?.querySelector("[role='menu']"));
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") closeAll();
  });
}

function repaintDesktop(isCreating) {
  if (!desktopState) return;
  paint(
    currentGen,
    renderDesktopMembers(
      desktopState.members,
      desktopState.projects,
      desktopState.grantsByMember,
      isCreating ? null : desktopState.selectedMember,
      isCreating,
      desktopState.detail,
    ),
  );
  installShellLayout(main);
  setupMembersEvents();
}

function showFieldError(node, message) {
  if (!node) return;
  node.textContent = message;
  node.style.display = "block";
}

// The name an approval or a new member goes by, checked before the hub is asked.
// Returns the message to show, or "" when the name will do.
export function memberNameProblem(name) {
  if (!name) return "Member name is required.";
  if (!MEMBER_NAME.test(name)) return "Lowercase letters, digits, dots and dashes only.";
  return "";
}

function setupMembersEvents() {
  const root = /** @type {HTMLElement} */ (main.querySelector(".shell[data-segment='access']") || main.querySelector(".access-screen") || main);
  if (!root || root.dataset.membersEventsBound === "on") return;
  root.dataset.membersEventsBound = "on";
  wireRowMenus();

  root.addEventListener("keydown", (event) => {
    if (/** @type {KeyboardEvent} */ (event).key === "Enter") {
      const row = document.activeElement?.closest(".member-index-row");
      if (row) {
        requestAnimationFrame(() => {
          const reissueBtn = /** @type {HTMLElement} */ (main.querySelector('.shell-stage [data-action="member-token"]') || main.querySelector('.shell-stage'));
          if (reissueBtn) reissueBtn.focus();
        });
      }
    } else if (/** @type {KeyboardEvent} */ (event).key === "Escape") {
      if (desktopState && desktopState.isCreating) {
        event.preventDefault();
        desktopState.isCreating = false;
        repaintDesktop(false);
        const target = desktopState.previousSelected
          ? main.querySelector(`.member-index-row[href*="${encodeURIComponent(desktopState.previousSelected)}"]`)
          : main.querySelector(".member-index-row");
        /** @type {HTMLElement} */ (target)?.focus();
        return;
      }
      if (document.activeElement?.closest(".shell-stage")) {
        event.preventDefault();
        const selected = /** @type {HTMLElement} */ (main.querySelector('.member-index-row[aria-current="true"]') || main.querySelector('.member-index-row'));
        if (selected) selected.focus();
      }
    }
  });

  root.addEventListener("click", (event) => {
    const grantBtn = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("[data-action='member-grant-open']"));
    if (grantBtn && desktopState) {
      event.preventDefault();
      openGrantPicker(
        grantBtn.dataset.id,
        grantBtn.dataset.name,
        desktopState.projects,
        desktopState.grantsByMember[grantBtn.dataset.id] || [],
      );
      return;
    }
    const toggleBtn = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("[data-action='toggle-add-member']"));
    if (toggleBtn) {
      event.preventDefault();
      if (desktopState && window.matchMedia("(min-width: 720px)").matches) {
        desktopState.isCreating = !desktopState.isCreating;
        if (desktopState.isCreating) {
          desktopState.previousSelected = desktopState.selectedMember?.id || null;
        }
        repaintDesktop(desktopState.isCreating);
        if (desktopState.isCreating) {
          requestAnimationFrame(() => {
            document.getElementById("stage-member-id")?.focus();
          });
        } else {
          const target = desktopState.previousSelected
            ? main.querySelector(`.member-index-row[href*="${encodeURIComponent(desktopState.previousSelected)}"]`)
            : main.querySelector(".member-index-row");
          /** @type {HTMLElement} */ (target)?.focus();
        }
        return;
      }
      const form = document.getElementById("mobile-add-member-form");
      if (form) {
        form.style.display = form.style.display === "none" ? "block" : "none";
        if (form.style.display === "block") {
          form.querySelector("input")?.focus();
        }
      }
      return;
    }

    const cancelBtn = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("[data-action='member-cancel-create']"));
    if (cancelBtn && desktopState) {
      event.preventDefault();
      desktopState.isCreating = false;
      repaintDesktop(false);
      const target = desktopState.previousSelected
        ? main.querySelector(`.member-index-row[href*="${encodeURIComponent(desktopState.previousSelected)}"]`)
        : main.querySelector(".member-index-row");
      /** @type {HTMLElement} */ (target)?.focus();
      return;
    }

    const refuseBtn = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("[data-action='member-refuse']"));
    if (refuseBtn) {
      event.preventDefault();
      refuseMember(refuseBtn.dataset.id).catch((error) => toast(`Nothing changed: ${error.message}`));
    }
  });

  // The note under the rename field names the machine as it is being named.
  root.addEventListener("input", (event) => {
    const field = /** @type {HTMLInputElement} */ (event.target);
    const form = field.closest?.("[data-action='member-approve']");
    const note = form && form.querySelector("[data-approve-note]");
    if (note && field.name === "member") note.textContent = approveNote(field.value.trim() || /** @type {HTMLElement} */ (form).dataset.id);
  });

  root.addEventListener("submit", async (event) => {
    const approval = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("[data-action='member-approve']"));
    if (approval) {
      event.preventDefault();
      await approveMember(/** @type {HTMLFormElement} */ (approval));
      return;
    }
    const form = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("[data-action='desktop-member-create']"));
    if (!form) return;
    event.preventDefault();

    const idInput = /** @type {HTMLInputElement} */ (form.querySelector("#stage-member-id"));
    const nameInput = /** @type {HTMLInputElement} */ (form.querySelector("#stage-member-name"));
    const idError = /** @type {HTMLElement} */ (form.querySelector("#stage-member-id-error"));
    const nameError = /** @type {HTMLElement} */ (form.querySelector("#stage-member-name-error"));

    if (idError) idError.style.display = "none";
    if (nameError) nameError.style.display = "none";

    const idVal = (idInput?.value || "").trim();
    const nameVal = (nameInput?.value || "").trim();

    const problem = memberNameProblem(idVal);
    if (problem) {
      showFieldError(idError, problem);
      idInput?.focus();
      return;
    }
    if (!nameVal) {
      showFieldError(nameError, "Display name is required.");
      nameInput?.focus();
      return;
    }

    try {
      await createMember(idVal, nameVal, () => {
        if (desktopState) desktopState.isCreating = false;
      });
    } catch (err) {
      showFieldError(idError, err.message || "Could not create the member.");
      idInput?.focus();
    }
  });
}

// Create a member and issue its token in one step. The token is shown once,
// straight away, because the form promises it: there is nothing to reissue yet.
export async function createMember(id, displayName, beforeReveal = () => {}) {
  await api("/api/v1/members", {
    method: "POST",
    body: JSON.stringify({ id, display_name: displayName }),
  });
  await primeMembers({ force: true });
  beforeReveal();
  location.hash = `#/access?member=${encodeURIComponent(id)}`;
  let issued = null;
  try {
    issued = await api(`/api/v1/members/${encodeURIComponent(id)}/token`, { method: "POST" });
  } catch {}
  await revealIssuedToken(issued?.token || null, displayName || id, id);
}

// Approve a pending member under the name the operator left in the field. The
// hub answers with the member as admitted, which may carry a new id, and that
// is where the screen goes.
async function approveMember(form) {
  const id = form.dataset.id;
  const field = /** @type {HTMLInputElement} */ (form.querySelector("input[name='member']"));
  const error = /** @type {HTMLElement} */ (form.querySelector("#member-approve-error"));
  if (error) error.style.display = "none";
  const name = (field?.value || "").trim();
  const problem = memberNameProblem(name);
  if (problem) {
    showFieldError(error, problem);
    field?.focus();
    return;
  }
  const submit = /** @type {HTMLButtonElement} */ (form.querySelector("button[type='submit']"));
  if (submit) submit.disabled = true;
  try {
    const admitted = await api(`/api/v1/enrol/${encodeURIComponent(id)}/approve`, {
      method: "POST",
      body: JSON.stringify({ member: name }),
    });
    await primeMembers({ force: true });
    const landed = (admitted && (admitted.id || admitted.member)) || name;
    toast(landed === id ? `Approved ${landed}.` : `Approved as ${landed}.`);
    const next = `#/access?member=${encodeURIComponent(landed)}`;
    if (location.hash === next) await render();
    else location.hash = next;
  } catch (err) {
    if (submit) submit.disabled = false;
    showFieldError(error, err.message || "Could not approve the member.");
    field?.focus();
  }
}

async function refuseMember(id) {
  const confirmed = await confirmAction({
    title: `Refuse ${id}?`,
    body: "The machine is told no and its request is removed. It can ask again.",
    safe: "Keep",
    danger: "Refuse",
    tone: "primary",
    commit: async () => {
      await api(`/api/v1/enrol/${encodeURIComponent(id)}/refuse`, { method: "POST" });
      await primeMembers({ force: true });
      location.hash = "#/access";
      await render();
    },
  });
  if (confirmed) toast(`Refused ${id}.`);
}

export async function accessScreen(gen, params = null) {
  currentGen = gen;
  let members = [];
  let projects = [];
  const grantsByMember = {};
  try {
    const res = await Promise.all([api("/api/v1/members"), api("/api/v1/projects")]);
    members = res[0].members || [];
    projects = res[1].projects || [];
    await Promise.all(
      members.map(async (member) => {
        // A pending member holds no grants yet, so there is nothing to read.
        if (member.state === "pending") {
          grantsByMember[member.id] = [];
          return;
        }
        try {
          grantsByMember[member.id] = (
            await api(`/api/v1/members/${encodeURIComponent(member.id)}/grants`)
          ).grants || [];
        } catch {
          grantsByMember[member.id] = [];
        }
      }),
    );
  } catch (error) {
    paint(gen, errorCard("Access", error));
    return;
  }

  const memberParam =
    params?.get("member") || new URLSearchParams(location.hash.split("?")[1] || "").get("member");
  const selected = memberParam ? members.find((m) => m.id === memberParam) || null : null;

  // The selected member's sessions: the live ones for End, every one for the
  // agents it has run. A hub that cannot list them still draws the member.
  const detail = { live: [], seen: [] };
  if (selected && selected.state !== "pending") {
    const query = `member=${encodeURIComponent(selected.id)}`;
    const [live, seen] = await Promise.all([
      api(`/api/v1/sessions?${query}&live=1`).then((a) => a.sessions || []).catch(() => null),
      api(`/api/v1/sessions?${query}`).then((a) => a.sessions || []).catch(() => []),
    ]);
    detail.live = live;
    detail.seen = seen;
  }

  const isDesktop = window.matchMedia("(min-width: 720px)").matches;
  if (isDesktop) {
    desktopState = {
      members,
      projects,
      grantsByMember,
      selectedMember: selected,
      detail,
      isCreating: false,
      previousSelected: selected?.id || null,
    };
    paint(gen, renderDesktopMembers(members, projects, grantsByMember, selected, false, detail));
    installShellLayout(main);
    setupMembersEvents();
  } else {
    desktopState = null;
    if (selected) {
      paint(gen, renderMobileMemberDetail(selected, projects, grantsByMember[selected.id] || [], detail));
      setupMembersEvents();
    } else {
      paint(gen, renderMobileMembersList(members, projects));
      setupMembersEvents();
    }
  }
}

// The Settings card that points here: how many members, and what a token is.
export async function membersSection() {
  let members = [];
  try {
    members = (await api("/api/v1/members")).members || [];
  } catch (error) {
    return errorCard("Access", error);
  }

  const memberRows = members
    .map(
      (member) => `
    <div class="row member-record-row">
      <div class="grow">
        <div class="title">${esc(member.display_name || member.id)} <span class="record-badge">record</span></div>
        <div class="meta">first seen ${relative(member.created_at)}${member.last_seen_at ? " · active " + relative(member.last_seen_at) : ""} · <span class="mono">${esc(member.personal_project_id)}</span></div>
      </div>
    </div>`,
    )
    .join("");

  return `
    <div class="card access-card">
      <div class="row access-row">
        <div class="grow">
          <div class="title">Access</div>
          <div class="meta">One token per member. Every agent on that machine uses it.</div>
        </div>
      </div>
      <p><a class="button access-link" href="#/access">Manage access</a></p>
      <div class="meta access-card-lead">Members:</div>
      ${memberRows || '<p class="empty">No members yet.</p>'}
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
export function revealIssuedToken(token, memberName = "", memberId = "") {
  const opener = document.activeElement;
  const el = document.createElement("dialog");
  el.className = "dialog dialog-reveal";
  const titleId = "reveal-title";
  el.setAttribute("aria-labelledby", titleId);

  let state = 1;
  // A token passed in (a just-created member) opens straight at the reveal;
  // otherwise state 1 confirms before issuing.
  let visible = token || null;

  const form = document.createElement("form");
  form.method = "dialog";
  form.className = "dialog-form";

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
  commit.className = "dialog-commit action";

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

  // State 2 only, built on paint and cleared on close. The token is embedded in
  // what goes on the machine, so a reader copying it never splices it in by
  // hand. What goes in a repository holds no token at all. Each snippet is a row
  // that owns its string, with its own copy glyph.
  const setupWrap = document.createElement("div");
  setupWrap.className = "reveal-setup reveal-split";
  setupWrap.hidden = true;
  let snippets = new Map();

  function paintSetup(url, member, tokenValue) {
    snippets = new Map();
    setupWrap.textContent = "";
    if (!tokenValue) {
      setupWrap.hidden = true;
      return;
    }
    const groups = memberSetup({ url, member, token: tokenValue });
    for (const group of groups) {
      const section = document.createElement("section");
      section.className = "reveal-group";
      const heading = document.createElement("h3");
      heading.className = "dialog-label reveal-group-title";
      heading.textContent = group.title;
      const hint = document.createElement("p");
      hint.className = "dialog-note reveal-group-hint";
      hint.textContent = group.hint;
      section.append(heading, hint);
      for (const snippet of group.snippets) {
        snippets.set(snippet.id, snippet.text);
        const head = document.createElement("div");
        head.className = "reveal-setup-head";
        const label = document.createElement("span");
        label.className = "dialog-label";
        label.textContent = snippet.label;
        const copy = document.createElement("button");
        copy.type = "button";
        copy.className = "reveal-copy";
        copy.dataset.snippet = snippet.id;
        copy.setAttribute("aria-label", `Copy ${snippet.label}`);
        copy.innerHTML = glyphSvg("copy", { size: 18 });
        copy.addEventListener("click", () => {
          const text = snippets.get(snippet.id);
          if (text) copyToken(text, `${snippet.label} copied.`);
        });
        const pre = document.createElement("pre");
        pre.className = "reveal-setup-text";
        pre.textContent = snippet.text;
        head.append(label, copy);
        section.append(head, pre);
      }
      setupWrap.append(section);
    }
    setupWrap.hidden = false;
  }

  function paintState(next) {
    state = next;
    const name = memberName || memberId || "this member";
    if (state === 1) {
      heading.textContent = `Reissue the token for ${name}?`;
      body.textContent =
        "The current token stops working now. Use this when the member moves to a new machine; its sessions carry over. Anything still using the old token is refused until it is given the new one.";
      note.textContent = "Reissuing cannot be undone.";
      safe.textContent = "Cancel";
      commit.textContent = "Reissue";
      commit.className = "dialog-commit action";
      fieldWrap.hidden = true;
      value.textContent = "";
      paintSetup("", "", null);
      body.hidden = false;
    } else {
      heading.textContent = `Token for ${name}`;
      body.textContent = "Copy it now. It is shown once and cannot be read again.";
      body.hidden = false;
      note.textContent = "Done closes this and drops the token from the page.";
      safe.hidden = true;
      commit.textContent = "Done";
      // Done deletes nothing, so it takes the action fill, not the danger one.
      commit.className = "dialog-commit action";
      fieldWrap.hidden = false;
      value.textContent = visible || "";
      paintSetup(hubUrl(), memberId || memberName, visible);
    }
  }

  paintState(visible ? 2 : 1);

  safe.addEventListener("click", () => el.close("cancel"));
  commit.addEventListener("click", async () => {
    if (state === 1) {
      commit.disabled = true;
      try {
        const issued = await api(`/api/v1/members/${encodeURIComponent(memberId || memberName)}/token`, {
          method: "POST",
        });
        visible = issued.token;
        value.textContent = visible;
        commit.disabled = false;
        paintState(2);
        /** @type {HTMLElement} */ (valueRow.querySelector(".reveal-copy"))?.focus();
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
  if (visible) /** @type {HTMLElement} */ (valueRow.querySelector(".reveal-copy"))?.focus();
  else safe.focus();

  el.addEventListener(
    "close",
    () => {
      // CHECK 12.1.C, kept by removal: the string leaves the DOM here, so it
      // cannot be read out of the page once the dialog is gone.
      visible = null;
      value.textContent = "";
      snippets = new Map();
      setupWrap.textContent = "";
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
// listed: a public project is already open to every member, so there is nothing
// to grant. This mirrors the shared dialog's shape (a platform `<dialog>`, the
// safe control focused first, focus kept inside its own ring).
function openGrantPicker(memberId, memberName, projects, grants) {
  const granted = new Set((grants || []).map((grant) => grant.project_id));
  const options = (projects || []).filter((p) => p.confidential && !granted.has(p.id));

  const opener = document.activeElement;
  const el = document.createElement("dialog");
  el.className = "dialog";
  const titleId = "grant-title";
  el.setAttribute("aria-labelledby", titleId);
  el.setAttribute("aria-label", `Grant a project to ${memberName || memberId}`);

  const form = document.createElement("form");
  form.method = "dialog";
  form.className = "dialog-form";

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
      "Only confidential projects without a grant are listed. A public project is already open to this member.";
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
      await api(`/api/v1/members/${encodeURIComponent(memberId)}/grants`, {
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
    /** @type {HTMLElement} */ ((event.shiftKey ? ring[ring.length - 1] : ring[0])).focus();
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
  const issued = await api(`/api/v1/members/${encodeURIComponent(id)}/token`, { method: "POST" });
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
      await api(`/api/v1/members/${encodeURIComponent(id)}/token`, { method: "DELETE" });
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
    body: `The member will lose access to project ${project}.`,
    note: "Removing a grant cannot be undone.",
    safe: "Keep",
    danger: "Remove grant",
    tone: "primary",
    commit: async () => {
      await api(`/api/v1/members/${encodeURIComponent(id)}/grants/${encodeURIComponent(project)}`, {
        method: "DELETE",
      });
      await render();
    },
  });
  if (confirmed) {
    toast(`Grant removed on ${project}.`);
  }
}

registerScreen("access", { rows: ".member-index-row" });
