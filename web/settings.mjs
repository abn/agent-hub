// Settings: Appearance, Alerts, Access, and This browser.
// Seven controls total.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, paint } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { alertsEnabled, applyPrefs, prefs, savePrefs, saveToken, setAlertsEnabled } from "./prefs.mjs";
import { render } from "./router.mjs";
import { shellHTML, shellStageHead } from "./shell-layout.mjs";
import { formatBytes } from "./storage.mjs";
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

export function truncatePathLastTwo(fullPath) {
  if (!fullPath) return "";
  const parts = fullPath.replace(/\/+$/, "").split("/").filter(Boolean);
  if (parts.length <= 2) return fullPath;
  return `\u2026/${parts.slice(-2).join("/")}`;
}

const THEME_GLYPHS = {
  system:
    '<svg aria-hidden="true" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="18" height="12" rx="2"></rect><path d="M9 21h6M12 17v4"></path></svg>',
  light:
    '<svg aria-hidden="true" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" aria-hidden="true"><circle cx="12" cy="12" r="4"></circle><path d="M12 2v2M12 20v2M2 12h2M20 12h2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M19.1 4.9l-1.4 1.4M6.3 17.7l-1.4 1.4"></path></svg>',
  dark:
    '<svg aria-hidden="true" width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 14.5A8 8 0 1 1 9.5 4a6.5 6.5 0 0 0 10.5 10.5z"></path></svg>',
};

const COPY_PATH_SVG = glyphSvg("copy", { size: 17 });

const CHECK_SVG = `<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6L9 17l-5-5"></path></svg>`;

function themeSegment(value, current) {
  const labels = { system: "System", light: "Light", dark: "Dark" };
  const isCurrent = current === value;
  return `<button type="button" role="radio" class="settings-theme-btn" data-theme-val="${value}" aria-checked="${
    isCurrent ? "true" : "false"
  }" aria-label="${labels[value]}" style="width:36px;min-width:36px;height:30px;min-height:30px;border:0;border-radius:5px;${
    isCurrent ? "background:var(--surface);box-shadow:var(--shadow-1);color:var(--ink);" : "background:none;color:var(--ink-2);"
  }display:grid;place-items:center;cursor:pointer">${THEME_GLYPHS[value]}</button>`;
}

function desktopThemeSegment(value, current) {
  const labels = { system: "Follow the system", light: "Light", dark: "Dark" };
  return `<button type="button" class="settings-segment settings-glyph-seg" data-theme-val="${value}" aria-pressed="${
    current === value
  }" aria-label="${labels[value]}">${THEME_GLYPHS[value]}</button>`;
}

function alertsContent() {
  const state = notificationState();
  if (state === "unsupported") {
    return `
      <div class="settings-row row">
        <div class="grow">
          <div class="title">Waiting on you</div>
          <div class="meta">This browser cannot notify. The inbox still shows everything.</div>
        </div>
      </div>`;
  }
  if (state === "blocked") {
    return `
      <div class="settings-row row">
        <div class="grow">
          <div class="title">Waiting on you</div>
          <div class="meta">Blocked in this browser. Allow notifications for this site, then check again.</div>
        </div>
        <button type="button" class="hairline settings-btn-36" data-action="check-alerts">Check again</button>
      </div>`;
  }
  if (state === "granted") {
    const on = alertsEnabled();
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
      <div class="grow">
        <div class="title">Waiting on you</div>
        <div class="meta">Your browser will ask first. Nothing is sent until you turn it on.</div>
      </div>
      <button type="button" class="primary settings-btn-44" data-action="notification-enable">Turn on</button>
    </div>`;
}

const NAV_CHEVRON = `<span class="settings-nav-chevron" aria-hidden="true">${glyphSvg("chevronRight", { size: 16 })}</span>`;

function alertsRow() {
  const state = notificationState();
  let sub = "The browser asks first";
  let checked = false;
  if (state === "blocked") {
    sub = "Blocked in browser settings";
    checked = false;
  } else if (state === "granted") {
    checked = alertsEnabled();
    sub = checked ? "Approvals and questions only" : "Notifications off";
  }

  return `
    <div class="settings-flat-row row">
      <span class="settings-row-text" style="flex:1;min-width:0;display:flex;flex-direction:column;gap:3px">
        <span class="title settings-row-title" style="font-size:15px;font-weight:500" id="alerts-label">Alert when waiting on you</span>
        <span class="meta settings-row-sub" style="font-size:12px;color:var(--ink-3)">${esc(sub)}</span>
      </span>
      ${switchControl("alerts-master", "alerts-label", checked, "toggle-alerts-switch")}
    </div>`;
}

// The desktop alerts row: label and switch, with the sub as the one helper
// line the whole screen allows. It is the only place a consequence is
// invisible, so it is the only place a line may explain one.
function desktopAlertsRow() {
  const state = notificationState();
  let sub = "The browser asks first";
  let checked = false;
  if (state === "blocked") {
    sub = "Blocked in browser settings";
  } else if (state === "granted") {
    checked = alertsEnabled();
    sub = checked ? "Approvals and questions only" : "Notifications off";
  }

  return `
    <div class="form-row">
      <div class="form-row-main">
        <span class="form-row-title" id="alerts-label">Alert when waiting on you</span>
        <span class="form-row-sub">${esc(sub)}</span>
      </div>
      ${switchControl("alerts-master", "alerts-label", checked, "toggle-alerts-switch")}
    </div>`;
}

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
  const agentCountText = `${agentCount} ${agentCount === 1 ? "agent" : "agents"}`;
  const agentText = `${agentCountText}${latestSeen ? ` · last call ${relative(latestSeen)}` : ""}`;

  const currentTheme = prefs.theme || "system";
  const densityCompact = (prefs.density || "comfortable") === "compact";
  const shortcutsOn = prefs.shortcuts === "on";

  const used = storage?.used_bytes ?? 0;
  const storageValue = formatBytes(used);
  const dataPath = storage?.data_path || "";
  const lastTwoPath = truncatePathLastTwo(dataPath);
  const nodeLine = storage?.node ? `${storage.node.host} · ${storage.node.mode}` : "";
  // The row shows the hub's own version and the commit it was built from, both
  // read from the payload. If the hub could not be reached there is nothing
  // real to show, and the row says so rather than inventing a number.
  const versionValue = storage?.version
    ? `v${storage.version}${storage.commit ? ` · ${storage.commit}` : ""}`
    : "unknown";

  const isDesktop = window.matchMedia("(min-width: 720px)").matches;
  if (isDesktop) {
    // The round-11 row form: one 640px column of 48px rows under quiet mono
    // group labels, a row is label · value · control, a glyph only where it
    // names a destination. The alerts sub is the one helper line in the whole
    // screen. The reserved 40px control row carries the data path once with
    // the copy glyph on the row that owns the string; no footer repeats it.
    const desktopForm = `
    <form class="settings" data-action="prefs" onsubmit="event.preventDefault();">
      <style>
        .settings .settings-btn-row {
          width: 100%;
          border: 0;
          border-top: 0 !important;
          background: none;
          font: inherit;
          text-align: left;
          cursor: pointer;
          color: var(--ink);
        }
        .settings-control-path {
          flex: 1;
          min-width: 0;
          overflow: hidden;
          text-overflow: ellipsis;
          white-space: nowrap;
        }
        .shell-controls-path {
          display: flex;
          align-items: center;
          gap: 8px;
        }
        .settings-copy-path-btn {
          flex: none;
          width: 28px;
          height: 28px;
          display: grid;
          place-items: center;
          background: none;
          border: 0;
          border-radius: var(--r-1);
          color: var(--ink-2);
          cursor: pointer;
        }
        .settings-copy-path-btn.is-copied {
          color: var(--ok);
          border: 1px solid var(--ok);
        }
        @media (pointer: coarse) {
          .settings-copy-path-btn { width: 36px; height: 36px; }
        }
        /* The global 44px tap-target floor belongs to touch. On a fine pointer
           this glyph sits in the desktop control row, which is 40px and carries
           no control taller than 32 (CHECK 13.D), so the floor is lifted. */
        @media (pointer: fine) {
          .settings-copy-path-btn { min-height: 0; }
        }
      </style>
      <div class="form-column">
        <section class="form-group">
          <div class="form-group-label">THIS DEVICE</div>
          <div class="form-row">
            <div class="form-row-main">
              <span class="form-row-title" id="label-theme">Theme</span>
            </div>
            <div class="settings-segmented settings-theme-seg" role="group" aria-label="Theme">
              ${desktopThemeSegment("system", currentTheme)}
              ${desktopThemeSegment("light", currentTheme)}
              ${desktopThemeSegment("dark", currentTheme)}
            </div>
          </div>
          <div class="form-row">
            <div class="form-row-main">
              <span class="form-row-title" id="density-label">Compact rows</span>
            </div>
            ${switchControl("density", "density-label", densityCompact, "toggle-density")}
          </div>
          ${desktopAlertsRow()}
        </section>

        <section class="form-group form-group-keyboard settings-group-keyboard">
          <div class="form-group-label">KEYBOARD</div>
          <div class="form-row">
            <div class="form-row-main">
              <span class="form-row-title" id="shortcuts-label">Single-key shortcuts</span>
              <span class="form-row-sub">Press <span class="mono">?</span> for the list</span>
            </div>
            ${switchControl("shortcuts", "shortcuts-label", shortcutsOn, "toggle-shortcuts")}
          </div>
        </section>

        <section class="form-group">
          <div class="form-group-label">THIS HUB</div>
          <a class="form-row" href="#/storage">
            <div class="form-row-main">
              <span class="form-row-title">Storage</span>
            </div>
            <span class="form-row-value">${esc(storageValue)}</span>
            ${NAV_CHEVRON}
          </a>
          <a class="form-row" href="#/access">
            <div class="form-row-main">
              <span class="form-row-title">Agents and tokens</span>
            </div>
            <span class="form-row-value">${esc(agentCountText)}</span>
            ${NAV_CHEVRON}
          </a>
          <div class="form-row" style="padding-right: 36px">
            <div class="form-row-main">
              <span class="form-row-title">Version</span>
            </div>
            <span class="form-row-value" style="user-select: text">${esc(versionValue)}</span>
          </div>
          <button type="button" class="form-row settings-btn-row" data-action="signout" style="border-top: 0">
            <span class="form-row-main"><span class="form-row-title">Sign out of this browser</span></span>
          </button>
        </section>

        <div class="settings-helper-line">This device's settings are saved in this browser only.</div>
      </div>
    </form>`;

    paint(
      gen,
      shellHTML({
        noIndex: true,
        stageHead: shellStageHead("Settings", nodeLine),
        stageControls: `<div class="shell-controls shell-controls-path">
          <span class="shell-meta mono settings-control-path">${esc(dataPath)}</span>
          <button type="button" aria-label="Copy full data path" class="settings-copy-path-btn" data-action="copy-path" data-path="${esc(dataPath)}">${COPY_PATH_SVG}</button>
        </div>`,
        stageBody: `<div class="shell-pad settings-pad">${desktopForm}</div>`,
      }),
    );

    setupDesktopSettingsEvents();
    return;
  }

  const form = `
    <form class="settings" data-action="prefs" onsubmit="event.preventDefault();">
      <style>
        @media (pointer: coarse) {
          .settings-row-shortcuts { display: none !important; }
        }
        .settings-flat-group { margin-bottom: 8px; }
        .settings-group-title { padding: 20px 16px 8px; font: 600 12px/1 var(--font-mono); color: var(--ink-3); letter-spacing: .06em; }
        .settings-flat-row { display: flex; align-items: center; gap: 12px; min-height: 56px; padding: 8px 16px; border-bottom: 1px solid var(--line); background: var(--surface); color: var(--ink); text-decoration: none; box-sizing: border-box; }
        .settings-flat-row:first-of-type { border-top: 1px solid var(--line); }
        .settings-nav-row { padding: 8px 8px 8px 16px; }
        .settings-btn-row { width: 100%; border: 0; border-top: 0 !important; font: 500 15px/1 var(--font-sans); text-align: left; cursor: pointer; color: var(--ink); }
        .settings-footer-row { display: flex; align-items: center; gap: 4px; padding: 14px 8px 0 16px; }
        .settings-copy-btn { flex: none; width: 44px; height: 44px; display: grid; place-items: center; background: none; border: 0; border-radius: var(--r-1); color: var(--ink-2); cursor: pointer; }
        .settings-copy-btn:focus-visible { outline: 2px solid var(--focus); }
      </style>

      <div class="settings-flat-group">
        <div class="settings-group-title">THIS DEVICE</div>
        <div class="settings-flat-row row">
          <span class="title" style="flex:1;font-size:15px;font-weight:500">Theme</span>
          <div role="radiogroup" aria-label="Theme" style="flex:none;display:flex;height:34px;padding:2px;box-sizing:border-box;border-radius:var(--r-1);background:var(--surface-2)">
            ${themeSegment("system", currentTheme)}
            ${themeSegment("light", currentTheme)}
            ${themeSegment("dark", currentTheme)}
          </div>
        </div>
        <div class="settings-flat-row row">
          <span class="title" style="flex:1;font-size:15px;font-weight:500" id="density-label">Compact rows</span>
          ${switchControl("density", "density-label", densityCompact, "toggle-density")}
        </div>
        <div class="settings-flat-row row settings-row-shortcuts">
          <span style="flex:1;min-width:0;display:flex;flex-direction:column;gap:3px">
            <span class="title" style="font-size:15px;font-weight:500" id="shortcuts-label">Single-key shortcuts</span>
            <span class="meta" style="font-size:12px;color:var(--ink-3)">j, k, e, r act with no modifier, suspended while a text field has focus.</span>
          </span>
          ${switchControl("shortcuts", "shortcuts-label", shortcutsOn, "toggle-shortcuts")}
        </div>
        ${alertsRow()}
      </div>

      <div class="settings-flat-group">
        <div class="settings-group-title">THIS HUB</div>
        <a href="#/storage" class="settings-flat-row row settings-nav-row">
          <span class="title" style="flex:1;font-size:15px;font-weight:500">Storage</span>
          <span class="mono" style="font:500 13px/1 var(--font-mono);color:var(--ink-2)">${esc(storageValue)}</span>
          <span style="flex:none;width:32px;height:44px;display:grid;place-items:center;color:var(--ink-3)">${glyphSvg("chevronRight", { size: 18 })}</span>
        </a>
        <a href="#/access" class="settings-flat-row row settings-nav-row">
          <span class="title" style="flex:1;font-size:15px;font-weight:500">Agents and tokens</span>
          <span class="meta" style="font-size:13px;color:var(--ink-2)">${esc(agentCountText)}</span>
          <span style="flex:none;width:32px;height:44px;display:grid;place-items:center;color:var(--ink-3)">${glyphSvg("chevronRight", { size: 18 })}</span>
        </a>
        <div class="settings-flat-row row" style="padding-right: 36px">
          <span class="title" style="flex:1;font-size:15px;font-weight:500">Version</span>
          <span class="mono" style="font:500 13px/1 var(--font-mono);color:var(--ink-2);user-select:text">${esc(versionValue)}</span>
        </div>
        <button type="button" class="settings-flat-row row settings-btn-row" data-action="signout" style="border-top: 0">Sign out of this browser</button>
      </div>

      <footer class="settings-footer-row">
        <span class="settings-data-path mono" style="flex:1;min-width:0;font:500 12px/1.4 var(--font-mono);color:var(--ink-3);overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(lastTwoPath)}</span>
        <button type="button" aria-label="Copy full data path" class="settings-copy-btn settings-copy-path-btn" data-action="copy-path" data-path="${esc(dataPath)}">${COPY_PATH_SVG}</button>
      </footer>
    </form>`;

  paint(
    gen,
    shellHTML({
      segment: "settings",
      noIndex: true,
      stageHead: shellStageHead("Settings", nodeLine, "", "#/more"),
      stageControls: "",
      stageBody: `<div class="shell-pad settings-pad">${form}</div>`,
    }),
  );

  setupSettingsEvents();
}

function setupDesktopSettingsEvents() {
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
    const masterBtn = event.target.closest(
      "#alerts-master, [data-action='toggle-alerts-switch'], [data-action='toggle-alert-master']",
    );
    if (masterBtn) {
      event.preventDefault();
      const current = masterBtn.getAttribute("aria-checked") === "true";
      const next = !current;
      setAlertsEnabled(next);
      masterBtn.setAttribute("aria-checked", String(next));
      masterBtn.classList.toggle("on", next);
      masterBtn.classList.toggle("off", !next);
      return;
    }

    // Turn on notifications
    const enableBtn = event.target.closest("[data-action='notification-enable']");
    if (enableBtn) {
      event.preventDefault();
      await enableNotifications();
      return;
    }

    // Check alerts button
    const checkBtn = event.target.closest("[data-action='check-alerts']");
    if (checkBtn) {
      event.preventDefault();
      render();
      return;
    }

    // Copy path control on the reserved control row
    const copyPathBtn = event.target.closest("[data-action='copy-path']");
    if (copyPathBtn) {
      event.preventDefault();
      const fullPath = copyPathBtn.dataset.path || "";
      if (navigator.clipboard && navigator.clipboard.writeText) {
        try {
          await navigator.clipboard.writeText(fullPath);
        } catch {}
      }
      copyPathBtn.innerHTML = CHECK_SVG;
      copyPathBtn.classList.add("is-copied");
      copyPathBtn.setAttribute("aria-label", "Copied");
      setTimeout(() => {
        copyPathBtn.innerHTML = COPY_PATH_SVG;
        copyPathBtn.classList.remove("is-copied");
        copyPathBtn.setAttribute("aria-label", "Copy full data path");
      }, 1400);
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
      const group = themeBtn.closest("[role='radiogroup'], .settings-segmented");
      if (group) {
        group.querySelectorAll("button[data-theme-val]").forEach((b) => {
          const match = b.dataset.themeVal === val;
          b.setAttribute("aria-checked", match ? "true" : "false");
          b.style.background = match ? "var(--surface)" : "none";
          b.style.color = match ? "var(--ink)" : "var(--ink-2)";
          b.style.boxShadow = match ? "var(--shadow-1)" : "none";
        });
      }
      return;
    }

    // Copy path click
    const copyPathBtn = event.target.closest("[data-action='copy-path']");
    if (copyPathBtn) {
      event.preventDefault();
      const fullPath = copyPathBtn.dataset.path || "";
      if (navigator.clipboard && navigator.clipboard.writeText) {
        try {
          await navigator.clipboard.writeText(fullPath);
        } catch {}
      }
      copyPathBtn.innerHTML = CHECK_SVG;
      copyPathBtn.classList.add("is-copied");
      copyPathBtn.setAttribute("aria-label", "Copied");
      setTimeout(() => {
        copyPathBtn.innerHTML = COPY_PATH_SVG;
        copyPathBtn.classList.remove("is-copied");
        copyPathBtn.setAttribute("aria-label", "Copy full data path");
      }, 1400);
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
    const masterBtn = event.target.closest(
      "#alerts-master, [data-action='toggle-alerts-switch'], [data-action='toggle-alert-master']",
    );
    if (masterBtn) {
      event.preventDefault();
      const state = notificationState();
      const sub = masterBtn.closest(".settings-flat-row")?.querySelector(".settings-row-sub");
      if (state === "granted") {
        const current = masterBtn.getAttribute("aria-checked") === "true";
        const next = !current;
        setAlertsEnabled(next);
        masterBtn.setAttribute("aria-checked", String(next));
        masterBtn.classList.toggle("on", next);
        masterBtn.classList.toggle("off", !next);
        if (sub) sub.textContent = next ? "Approvals and questions only" : "Notifications off";
      } else if (state === "blocked" || state === "unsupported") {
        masterBtn.setAttribute("aria-checked", "false");
        masterBtn.classList.add("off");
        masterBtn.classList.remove("on");
        if (sub) sub.textContent = "Blocked in browser settings";
      } else {
        if ("Notification" in window) {
          const res = await Notification.requestPermission();
          if (res === "granted") {
            setAlertsEnabled(true);
            masterBtn.setAttribute("aria-checked", "true");
            masterBtn.classList.add("on");
            masterBtn.classList.remove("off");
            if (sub) sub.textContent = "Approvals and questions only";
          } else {
            masterBtn.setAttribute("aria-checked", "false");
            masterBtn.classList.add("off");
            masterBtn.classList.remove("on");
            if (sub) sub.textContent = "Blocked in browser settings";
          }
        }
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
