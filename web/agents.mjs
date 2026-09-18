// Agents and access: the Settings section that lists agents, their trust, their
// tokens, and their grants.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { errorCard, esc, main } from "./dom.mjs";
import { render } from "./router.mjs";

export async function agentsSection() {
  let agents;
  try {
    agents = (await api("/api/v1/agents")).agents;
  } catch (error) {
    return errorCard("Agents and access", error);
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
            `<div class="meta mono">${esc(grant.project_id)} · ${esc(grant.access)} ` +
            `<button type="button" data-action="agent-ungrant" data-id="${esc(agent.id)}" data-project="${esc(grant.project_id)}" aria-label="Remove grant on ${esc(grant.project_id)}">Remove</button></div>`,
        )
        .join("");
      const promote = agent.trust === "trusted" ? "untrusted" : "trusted";
      return `<div class="row">
        <div class="grow">
          <div class="title">${esc(agent.display_name)} <span class="pill">${esc(agent.trust)}</span></div>
          <div class="meta mono">${esc(agent.id)} · ${esc(agent.personal_project_id)}</div>
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

export async function setAgentTrust(id, trust) {
  await api(`/api/v1/agents/${encodeURIComponent(id)}`, {
    method: "PATCH",
    body: JSON.stringify({ trust }),
  });
  await render();
}

export async function reissueToken(id) {
  const issued = await api(`/api/v1/agents/${encodeURIComponent(id)}/token`, {
    method: "POST",
  });
  await render();
  showToken(issued.token);
}

export async function revokeToken(id) {
  const confirmed = await confirmAction({
    title: `Revoke the token for ${id}?`,
    body: "The agent loses access immediately. A new token can be issued, but this one is gone.",
    note: "Revoking cannot be undone.",
    safe: "Keep",
    danger: "Revoke token",
  });
  if (!confirmed) return;
  await api(`/api/v1/agents/${encodeURIComponent(id)}/token`, { method: "DELETE" });
  await render();
}

export async function ungrant(id, project) {
  await api(`/api/v1/agents/${encodeURIComponent(id)}/grants/${encodeURIComponent(project)}`, {
    method: "DELETE",
  });
  await render();
}
