// Settings: Appearance, Alerts, Access, and This browser.
// Seven controls total.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, paint } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { usedOfCapacity } from "./home.mjs";
import { applyPrefs, prefs, savePrefs, saveToken } from "./prefs.mjs";
import { render } from "./router.mjs";
import { shellHTML, shellStageHead } from "./shell-layout.mjs";import { formatBytes } from "./storage.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

function readAlertPref(key, fallback = "on") {
  try {
    const val = localStorage.getItem(`hub.alerts.${key}`);
    return val === "off" ? "off" : fallback;
  } catch {
    return fallback;
  }
}

function writeAlertPref(key, val) {
  try {
    localStorage.setItem(`hub.alerts.${key}`, val);
  } catch {}
}

export function notificationState() {
  if (!("Notification" in window) || !("serviceWorker" in navigator)) {
    return "unsupported";
  }
  if (Notification.permission === "granted") {
    return "granted";
  }
  if (Notification.permission === "denied") {
    return "blocked";
  }
  return "default";
}

export async function enableNotifications() {
  if (!("Notification" in window)) return;
  await Notification.requestPermission();
  render();
}

function switchControl(id, labelId, checked, action = "", extra = "") {
  return `<button type="button" id="${esc(id)}" role="switch" aria-labelledby="${esc(labelId)}" aria-checked="${checked ? "true" : "false"}" class="settings-switch ${checked ? "on" : "off"}"${action ? ` data-action="${esc(action)}"` : ""}${extra}>
    <span class="settings-switch-track"><span class="settings-switch-thumb"></span></span>
  </button>`;
}

const THEME_GLYPHS = {
  system:
    '<svg aria-hidden="true" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="18" height="12" rx="1.5"></rect><path d="M8 20h8M12 17v3"></path></svg>',
  light:
    '<svg aria-hidden="true" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="4"></circle><path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M19.1 4.9l-1.4 1.4M6.3 17.7l-1.4 1.4"></path></svg>',
  dark:
    '<svg aria-hidden="true" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z"></path></svg>',
};

function themeSegment(value, current) {
  const labels = { system: "Follow the system", light: "Light", dark: "Dark" };
  return `<button type="button" class="settings-segment settings-glyph-seg" data-theme-val="${value}" aria-pressed="${
    current === value
  }" aria-label="${labels[value]}">${THEME_GLYPHS[value]}</button>`;
}

// The alerts section is the design's one row while notifications are on: a
// switch and one line saying what it covers. The permission states stay,
// because a browser that refuses or cannot ask needs somewhere to say so.
function alertsContent() {
  const state = notificationState();
  if (state === "unsupported") {
    return `
      <div class="settings-row row">
        <span class="alerts-glyph unsupported" aria-hidden="true">${glyphSvg("bellStruck", { size: 20 })}</span>
        <div class="grow">
          <div class="title">Waiting on you</div>
          <div class="meta">This browser cannot notify. The inbox still shows everything.</div>
        </div>
      </div>`;
  }
  if (state === "blocked") {
    return `
      <div class="settings-row row">
        <span class="alerts-glyph blocked" aria-hidden="true">${glyphSvg("bellStruck", { size: 20 })}</span>
        <div class="grow">
          <div class="title">Waiting on you</div>
          <div class="meta">Blocked in this browser. Allow notifications for this site, then check again.</div>
        </div>
        <button type="button" class="hairline settings-btn-36" data-action="check-alerts">Check again</button>
      </div>`;
  }
  if (state === "granted") {
    const on = readAlertPref("master", "on") === "on";
    return `
      <div class="settings-row row switch-row">
        <div class="grow">
          <div class="title" id="alerts-master-label">Waiting on you</div>
          <div class="meta">Approvals and questions only</div>
        </div>
        ${switchControl("alerts-master", "alerts-master-label", on, "toggle-alert-master")}
      </div>`;
  }
  return `
    <div class="settings-row row">
      <span class="alerts-glyph" aria-hidden="true">${glyphSvg("bell", { size: 20 })}</span>
      <div class="grow">
        <div class="title">Waiting on you</div>
        <div class="meta">Your browser will ask first. Nothing is sent until you turn it on.</div>
      </div>
      <button type="button" class="primary settings-btn-44" data-action="notification-enable">Turn on</button>
    </div>`;
}

const NAV_CHEVRON = `<span class="settings-nav-chevron" aria-hidden="true">${glyphSvg("chevronRight", { size: 16 })}</span>`;

export async function settingsScreen(gen) {
  const [agentsRes, storage] = await Promise.all([
    api("/api/v1/agents").catch(() => ({ agents: [] })),
    api("/api/v1/storage").catch(() => null),
  ]);
  const agents = agentsRes.agents || [];
  let latestSeen = null;
  for (const a of agents) {
    if (a.last_seen_at && (!latestSeen || a.last_seen_at > latestSeen)) latestSeen = a.last_seen_at;
  }
  const agentCount = agents.length;
  const agentText = `${agentCount} ${agentCount === 1 ? "agent" : "agents"}${
    latestSeen ? ` · last call ${relative(latestSeen)}` : ""
  }`;

  const currentTheme = prefs.theme || "system";
  const densityCompact = (prefs.density || "comfortable") === "compact";
  const shortcutsOn = prefs.shortcuts === "on";

  const used = storage?.used_bytes ?? 0;
  const capacity = storage?.capacity_bytes ?? 0;
  const storageValue = capacity ? usedOfCapacity(used, capacity) : formatBytes(used);
  const dataPath = storage?.data_path || "";
  const nodeLine = storage?.node ? `${storage.node.host} · ${storage.node.mode}` : "";

  const form = `
    <form class="settings" data-action="prefs" onsubmit="event.preventDefault();">
      <section class="settings-group">
        <div class="settings-group-label">APPEARANCE</div>
        <div class="settings-group-card">
          <div class="settings-row row">
            <div class="grow"><div class="title" id="label-theme">Theme</div></div>
            <div class="settings-segmented settings-theme-seg" role="group" aria-label="Theme">
              ${themeSegment("system", currentTheme)}
              ${themeSegment("light", currentTheme)}
              ${themeSegment("dark", currentTheme)}
            </div>
          </div>
          <div class="settings-row row switch-row">
            <div class="grow"><div class="title" id="density-label">Compact rows</div></div>
            ${switchControl("density", "density-label", densityCompact, "toggle-density")}
          </div>
          <div class="settings-row row switch-row">
            <div class="grow">
              <div class="title" id="shortcuts-label">Single-key shortcuts</div>
              <div class="meta">j, k, e, r act with no modifier, suspended while a text field has focus.</div>
            </div>
            ${switchControl("shortcuts", "shortcuts-label", shortcutsOn, "toggle-shortcuts")}
          </div>
        </div>
      </section>

      <section class="settings-group">
        <div class="settings-group-label">ALERTS</div>
        <div class="settings-group-card alerts-group-card">${alertsContent()}</div>
      </section>

      <section class="settings-group">
        <div class="settings-group-label">THIS HUB</div>
        <div class="settings-group-card">
          <a class="settings-row settings-nav-row row" href="#/storage">
            <div class="grow">
              <div class="title">Storage</div>
              <div class="meta mono">${esc(storageValue)}</div>
            </div>
            ${NAV_CHEVRON}
          </a>
          <a class="settings-row settings-nav-row row" href="#/access">
            <div class="grow">
              <div class="title">Agents &amp; tokens</div>
              <div class="meta">${esc(agentText)}</div>
            </div>
            ${NAV_CHEVRON}
          </a>
          <div class="settings-row row">
            <div class="grow">
              <div class="title">This browser</div>
              <div class="meta">Holding the access token. Signing out forgets it here and nowhere else.</div>
            </div>
            <button type="button" class="settings-signout-btn hairline" data-action="signout">Sign out</button>
          </div>
        </div>
      </section>

      <footer class="settings-footer mono">${esc(dataPath)}${nodeLine ? ` · ${esc(nodeLine)}` : ""}</footer>
    </form>`;

  paint(
    gen,
    shellHTML({
      noIndex: true,
      stageHead: shellStageHead("Settings", nodeLine),
      stageControls: `<div class="shell-controls"><span class="shell-meta mono">${esc(dataPath)}</span></div>`,
      stageBody: `<div class="shell-pad settings-pad">${form}</div>`,
    }),
  );

  setupSettingsEvents();
}

function setupSettingsEvents() {
  const root = document.querySelector(".settings");
  if (!root) return;

  root.addEventListener("click", async (event) => {
    // Theme segment click
    const themeBtn = event.target.closest("button[data-theme-val]");
    if (themeBtn) {
      event.preventDefault();
      const val = themeBtn.dataset.themeVal;
      savePrefs({ theme: val, density: prefs.density, shortcuts: prefs.shortcuts });
      applyPrefs();
      const group = themeBtn.closest(".settings-segmented");
      if (group) {
        group.querySelectorAll("button[data-theme-val]").forEach((b) => {
          b.setAttribute("aria-pressed", b.dataset.themeVal === val ? "true" : "false");
        });
      }
      return;
    }

    // Compact rows switch
    const densityBtn = event.target.closest("[data-action='toggle-density']");
    if (densityBtn) {
      event.preventDefault();
      const next = (prefs.density || "comfortable") === "compact" ? "comfortable" : "compact";
      savePrefs({ theme: prefs.theme, density: next, shortcuts: prefs.shortcuts });
      applyPrefs();
      const on = next === "compact";
      densityBtn.setAttribute("aria-checked", String(on));
      densityBtn.classList.toggle("on", on);
      densityBtn.classList.toggle("off", !on);
      return;
    }

    // Shortcuts toggle
    const shortcutsBtn = event.target.closest("#shortcuts, [data-action='toggle-shortcuts']");
    if (shortcutsBtn) {
      event.preventDefault();
      const next = prefs.shortcuts === "on" ? "off" : "on";
      savePrefs({ theme: prefs.theme, density: prefs.density, shortcuts: next });
      shortcutsBtn.setAttribute("aria-checked", next === "on" ? "true" : "false");
      shortcutsBtn.classList.toggle("on", next === "on");
      shortcutsBtn.classList.toggle("off", next === "off");
      return;
    }

    // Alerts master toggle
    const masterBtn = event.target.closest("#alerts-master, [data-action='toggle-alert-master']");
    if (masterBtn) {
      event.preventDefault();
      const current = masterBtn.getAttribute("aria-checked") === "true";
      const next = !current;
      writeAlertPref("master", next ? "on" : "off");
      masterBtn.setAttribute("aria-checked", String(next));
      masterBtn.classList.toggle("on", next);
      masterBtn.classList.toggle("off", !next);
      // Update sub kinds display
      const card = masterBtn.closest(".alerts-group-card");
      if (card) {
        card.querySelectorAll(".settings-kind-row .settings-switch").forEach((sw) => {
          const kind = sw.dataset.kind;
          const kindActive = next && (readAlertPref(kind, "on") === "on");
          sw.setAttribute("aria-checked", String(kindActive));
          sw.classList.toggle("on", kindActive);
          sw.classList.toggle("off", !kindActive);
        });
      }
      return;
    }

    // Alerts kind toggle
    const kindBtn = event.target.closest("[data-action='toggle-alert-kind']");
    if (kindBtn) {
      event.preventDefault();
      const kind = kindBtn.dataset.kind;
      const current = kindBtn.getAttribute("aria-checked") === "true";
      const next = !current;
      writeAlertPref(kind, next ? "on" : "off");
      kindBtn.setAttribute("aria-checked", String(next));
      kindBtn.classList.toggle("on", next);
      kindBtn.classList.toggle("off", !next);
      return;
    }

    // Check alerts button
    const checkBtn = event.target.closest("[data-action='check-alerts']");
    if (checkBtn) {
      event.preventDefault();
      render();
      return;
    }

    // Sign out button
    const signoutBtn = event.target.closest("[data-action='signout']");
    if (signoutBtn) {
      event.preventDefault();
      await signOut();
      return;
    }
  });
}

export async function signOut() {
  const confirmed = await confirmAction({
    title: "Sign out of this hub?",
    body: "This browser forgets the access token. The hub asks for it again, and nothing else is removed.",
    note: "You need the token to get back in.",
    safe: "Keep",
    danger: "Sign out",
  });
  if (!confirmed) return;
  const forgotten = saveToken("");
  location.hash = "#/connect";
  toast(
    forgotten
      ? "Signed out. This browser has forgotten the token."
      : "This browser would not forget the token, so it is still stored here.",
  );
}

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
