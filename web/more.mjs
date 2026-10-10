// The More screen. Tab root on a phone for storage, agents and tokens,
// settings, and the permanent sync line.

import { api } from "./api.mjs";
import { esc, paint } from "./dom.mjs";
import { shellHTML, shellStageHead } from "./shell-layout.mjs";
import { formatBytes } from "./storage.mjs";
import { relative } from "./time.mjs";
import { refreshSyncDisplay } from "./shell.mjs";

export async function moreScreen(gen) {
  let usage = {};
  let agents = [];
  try {
    const [storageRes, agentsRes] = await Promise.all([
      api("/api/v1/storage"),
      api("/api/v1/agents"),
    ]);
    usage = storageRes || {};
    agents = agentsRes?.agents || [];
  } catch {}

  const nodeLine = [usage.node?.host, usage.node?.mode]
    .filter((part) => typeof part === "string" && part)
    .join(" · ") || "demo · local";

  const storageVal = formatBytes(usage.used_bytes ?? 0);
  const prunableBytes = usage.prunable?.bytes ?? 0;
  const pruneSub = prunableBytes > 0 ? `${formatBytes(prunableBytes)} can be pruned` : "";

  const agentCount = agents.length;
  const agentVal = `${agentCount} ${agentCount === 1 ? "agent" : "agents"}`;
  let newestCall = 0;
  for (const a of agents) {
    const t = a.last_seen_at || a.created_at;
    if (t) {
      const ms = new Date(t).getTime();
      if (ms > newestCall) newestCall = ms;
    }
  }
  const agentSub = newestCall ? `last call ${relative(newestCall)}` : "no calls yet";

  const storageGlyph = `<svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><ellipse cx="12" cy="6" rx="7" ry="3"></ellipse><path d="M5 6v12c0 1.7 3.1 3 7 3s7-1.3 7-3V6"></path><path d="M5 12c0 1.7 3.1 3 7 3s7-1.3 7-3"></path></svg>`;
  const keyGlyph = `<svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="8" cy="15" r="4"></circle><path d="M11 12l9-9M17 6l3 3M15 8l2 2"></path></svg>`;
  const gearGlyph = `<svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="3"></circle><path d="M12 3v2.4M12 18.6V21M3 12h2.4M18.6 12H21M5.6 5.6l1.7 1.7M16.7 16.7l1.7 1.7M18.4 5.6l-1.7 1.7M7.3 16.7l-1.7 1.7"></path></svg>`;
  const chevronRight = `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 6l6 6-6 6"></path></svg>`;

  const content = `
    <div class="more-screen">
      <div class="more-gap"></div>
      <a class="row more-row" href="#/storage">
        <span class="more-glyph" aria-hidden="true">${storageGlyph}</span>
        <span class="more-main grow">
          <span class="title">Storage</span>
          ${pruneSub ? `<span class="more-sub more-sub-prune">${esc(pruneSub)}</span>` : ""}
        </span>
        <span class="more-val mono">${esc(storageVal)}</span>
        <span class="more-chevron" aria-hidden="true">${chevronRight}</span>
      </a>
      <a class="row more-row" href="#/access">
        <span class="more-glyph" aria-hidden="true">${keyGlyph}</span>
        <span class="more-main grow">
          <span class="title">Agents and tokens</span>
          <span class="more-sub">${esc(agentSub)}</span>
        </span>
        <span class="more-val">${esc(agentVal)}</span>
        <span class="more-chevron" aria-hidden="true">${chevronRight}</span>
      </a>
      <a class="row more-row" href="#/settings">
        <span class="more-glyph" aria-hidden="true">${gearGlyph}</span>
        <span class="more-main grow">
          <span class="title">Settings</span>
          <span class="more-sub">theme, rows, alerts, sign out</span>
        </span>
        <span class="more-val"></span>
        <span class="more-chevron" aria-hidden="true">${chevronRight}</span>
      </a>
      <div class="more-sync-line" id="more-sync" role="status" hidden>
        <span class="more-sync-dot" aria-hidden="true"></span>
        <span class="more-sync-text mono"></span>
        <button type="button" class="more-refresh-btn" data-action="more-refresh">Refresh</button>
      </div>
    </div>
  `;

  paint(
    gen,
    shellHTML({
      noIndex: true,
      stageHead: shellStageHead("More", nodeLine),
      // More has nothing to put in the control row, but the frame is reserved
      // whether or not a pane has content for it: the row is there so a screen
      // that grows one keeps its place.
      stageControls: '<div class="shell-controls"></div>',
      stageBody: content,
    }),
  );
  // More shows the same sync state the rail does, and no more: healthy means
  // the line is absent here too, with no space reserved for it.
  refreshSyncDisplay();
}
