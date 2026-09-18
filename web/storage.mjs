// Storage: what the node holds, by project and by kind.

import { api } from "./api.mjs";
import { esc, main } from "./dom.mjs";

export async function storageScreen() {
  const usage = await api("/api/v1/storage");
  const mb = (bytes) => (bytes / (1024 * 1024)).toFixed(2) + " MB";
  const rows = usage.projects
    .map(
      (p) => `<div class="row"><div class="grow"><div class="title">${esc(p.project_id)}</div>
        <div class="meta mono">artifacts ${mb(p.artifact_bytes)} · sessions ${mb(p.session_bytes)} · knowledge ${mb(p.kb_bytes)}</div></div></div>`,
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
