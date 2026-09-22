// Settings: Appearance, Alerts, Access, and This browser.
// Seven controls total.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, paint } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { applyPrefs, prefs, savePrefs, saveToken } from "./prefs.mjs";
import { render } from "./router.mjs";
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

function checkGlyph() {
  return glyphSvg("resolve", { size: 13, strokeWidth: 2.2, className: "settings-check" });
}

function switchControl(id, labelId, checked, action = "", extra = "") {
  return `<button type="button" id="${esc(id)}" role="switch" aria-labelledby="${esc(labelId)}" aria-checked="${checked ? "true" : "false"}" class="settings-switch ${checked ? "on" : "off"}"${action ? ` data-action="${esc(action)}"` : ""}${extra}>
    <span class="settings-switch-track"><span class="settings-switch-thumb"></span></span>
  </button>`;
}

function alertsContent() {
  const state = notificationState();
  if (state === "unsupported") {
    return `
      <div class="settings-row row alerts-info-row">
        <span class="alerts-glyph unsupported" aria-hidden="true">${glyphSvg("bellStruck", { size: 20 })}</span>
        <div class="alerts-body">
          <div class="title">Not available here</div>
          <div class="meta helper">This browser does not support notifications. The inbox still shows everything.</div>
        </div>
      </div>
    `;
  }

  if (state === "blocked") {
    return `
      <div class="settings-row row alerts-blocked-row">
        <div class="alerts-info-line">
          <span class="alerts-glyph blocked" aria-hidden="true">${glyphSvg("bellStruck", { size: 20 })}</span>
          <div class="alerts-body">
            <div class="title">Blocked in this browser</div>
            <div class="meta helper">We cannot ask again - the browser only asks once. Allow notifications for this site in your browser's site settings, then check again.</div>
          </div>
        </div>
        <div class="alerts-act-line">
          <button type="button" class="hairline settings-btn-36" data-action="check-alerts">Check again</button>
        </div>
        <div class="meta footnote">The Inbox still shows everything. Notifications only change when you hear about it.</div>
      </div>
    `;
  }

  if (state === "granted") {
    const masterOn = readAlertPref("master", "on") === "on";
    const waitingOn = readAlertPref("waiting", "on") === "on";
    const questionsOn = readAlertPref("questions", "on") === "on";
    const finishedOn = readAlertPref("finished", "on") === "on";

    return `
      <div class="settings-row row switch-row">
        <div class="grow">
          <div class="title" id="alerts-master-label">On for this browser</div>
          <div class="meta helper">Only while the tab is closed or in the background</div>
        </div>
        ${switchControl("alerts-master", "alerts-master-label", masterOn, "toggle-alert-master")}
      </div>
      <div class="settings-row row switch-row settings-kind-row">
        <div class="grow">
          <div class="title" id="alert-waiting-label">Waiting on you</div>
        </div>
        ${switchControl("alert-waiting", "alert-waiting-label", waitingOn && masterOn, "toggle-alert-kind", ' data-kind="waiting"')}
      </div>
      <div class="settings-row row switch-row settings-kind-row">
        <div class="grow">
          <div class="title" id="alert-questions-label">Questions from an agent</div>
        </div>
        ${switchControl("alert-questions", "alert-questions-label", questionsOn && masterOn, "toggle-alert-kind", ' data-kind="questions"')}
      </div>
      <div class="settings-row row switch-row settings-kind-row">
        <div class="grow">
          <div class="title" id="alert-finished-label">Finished work</div>
        </div>
        ${switchControl("alert-finished", "alert-finished-label", finishedOn && masterOn, "toggle-alert-kind", ' data-kind="finished"')}
      </div>
    `;
  }

  // State 1: default (Not asked yet)
  return `
    <div class="settings-row row alerts-off-row">
      <div class="alerts-info-line">
        <span class="alerts-glyph" aria-hidden="true">${glyphSvg("bell", { size: 20 })}</span>
        <div class="alerts-body">
          <div class="title">Notifications are off</div>
          <div class="meta helper">Your browser will ask first. Nothing is sent until you pick which kinds.</div>
        </div>
      </div>
      <div class="alerts-act-line">
        <button type="button" class="primary settings-btn-44" data-action="notification-enable">Turn on notifications</button>
      </div>
    </div>
  `;
}

export async function settingsScreen(gen) {
  let agents = [];
  try {
    const res = await api("/api/v1/agents");
    agents = res.agents || [];
  } catch {}

  const agentCount = agents.length;
  const agentText = agentCount === 1 ? "1 agent" : `${agentCount} agents`;
  let latestSeen = null;
  for (const a of agents) {
    if (a.last_seen_at) {
      if (!latestSeen || a.last_seen_at > latestSeen) {
        latestSeen = a.last_seen_at;
      }
    }
  }
  const lastCallText = latestSeen ? `last call ${relative(latestSeen)}` : "";
  const tokenStateLine = `1 live token · ${agentText}${lastCallText ? ` · ${lastCallText}` : ""}`;

  const currentTheme = prefs.theme || "system";
  const currentDensity = prefs.density || "comfortable";
  const shortcutsOn = prefs.shortcuts === "on";

  paint(
    gen,
    `
    <form class="settings" data-action="prefs" onsubmit="event.preventDefault();">
      <header class="settings-header">
        <div class="settings-header-top">
          <a class="settings-back-btn" href="#/home" aria-label="Back">
            ${glyphSvg("chevronBack", { size: 20 })}
          </a>
          <h1 class="settings-title">Settings</h1>
        </div>
        <div class="settings-subline">Everything here describes you or this browser. Projects are created and deleted on Projects.</div>
      </header>

      <!-- Group 1: Appearance -->
      <section class="settings-group" aria-labelledby="group-appearance">
        <div class="settings-group-label" id="group-appearance">APPEARANCE</div>
        <div class="settings-group-card">
          <!-- Theme -->
          <div class="settings-row row settings-row-stacked">
            <div class="settings-row-label">
              <div class="title" id="label-theme">Theme</div>
            </div>
            <div class="settings-control">
              <div class="settings-segmented" role="group" aria-label="Theme">
                <button type="button" class="settings-segment" role="button" aria-pressed="${currentTheme === "system"}" data-theme-val="system">
                  ${currentTheme === "system" ? checkGlyph() : ""}
                  <span>System</span>
                </button>
                <button type="button" class="settings-segment" role="button" aria-pressed="${currentTheme === "light"}" data-theme-val="light">
                  ${currentTheme === "light" ? checkGlyph() : ""}
                  <span>Light</span>
                </button>
                <button type="button" class="settings-segment" role="button" aria-pressed="${currentTheme === "dark"}" data-theme-val="dark">
                  ${currentTheme === "dark" ? checkGlyph() : ""}
                  <span>Dark</span>
                </button>
              </div>
            </div>
          </div>

          <!-- Density -->
          <div class="settings-row row settings-row-stacked">
            <div class="settings-row-label">
              <div class="title" id="label-density">Density</div>
              <div class="meta helper density-helper">
                <span class="coarse-only">Compact takes list rows to 40px. Hit areas stay 44px on touch either way.</span>
                <span class="fine-only">Compact takes list rows to 36px. Hit areas stay 44px either way.</span>
              </div>
            </div>
            <div class="settings-control">
              <div class="settings-segmented" role="group" aria-label="Density">
                <button type="button" class="settings-segment" role="button" aria-pressed="${currentDensity === "comfortable"}" data-density-val="comfortable">
                  ${currentDensity === "comfortable" ? checkGlyph() : ""}
                  <span class="density-label">Comfortable <span class="density-consequence coarse-only">· 48px rows</span><span class="density-consequence fine-only">· 44px rows</span></span>
                </button>
                <button type="button" class="settings-segment" role="button" aria-pressed="${currentDensity === "compact"}" data-density-val="compact">
                  ${currentDensity === "compact" ? checkGlyph() : ""}
                  <span class="density-label">Compact <span class="density-consequence coarse-only">· 40px</span><span class="density-consequence fine-only">· 36px</span></span>
                </button>
              </div>
            </div>
          </div>

          <!-- Single-key shortcuts -->
          <div class="settings-row row switch-row">
            <div class="grow">
              <label class="title" id="shortcuts-label" for="shortcuts">Single-key shortcuts</label>
              <div class="meta helper"><code>j</code>, <code>k</code>, <code>e</code>, <code>r</code> act with no modifier, suspended while a text field has focus.</div>
            </div>
            ${switchControl("shortcuts", "shortcuts-label", shortcutsOn, "toggle-shortcuts")}
          </div>
        </div>
      </section>

      <!-- Group 2: Alerts -->
      <section class="settings-group" aria-labelledby="group-alerts">
        <div class="settings-group-label" id="group-alerts">ALERTS</div>
        <div class="settings-group-card alerts-group-card">
          ${alertsContent()}
        </div>
      </section>

      <!-- Group 3: Access -->
      <section class="settings-group" aria-labelledby="group-access">
        <div class="settings-group-label" id="group-access">ACCESS</div>
        <div class="settings-group-card">
          <a class="settings-row settings-nav-row row" href="#/access">
            <span class="settings-nav-glyph" aria-hidden="true">${glyphSvg("idCard", { size: 17 })}</span>
            <div class="grow">
              <div class="title">Tokens and callers</div>
              <div class="meta">${esc(tokenStateLine)}</div>
            </div>
            <span class="settings-nav-chevron" aria-hidden="true">${glyphSvg("chevronRight", { size: 16 })}</span>
          </a>
        </div>
      </section>

      <!-- Group 4: This browser -->
      <section class="settings-group" aria-labelledby="group-this-browser">
        <div class="settings-group-label" id="group-this-browser">THIS BROWSER</div>
        <div class="settings-group-card">
          <div class="settings-row this-browser-info-row row">
            <div class="title">This browser is holding the access token</div>
            <div class="meta helper">Signing out forgets it here and nowhere else. Other browsers, and every agent, are unaffected.</div>
          </div>
          <div class="settings-row this-browser-act-row row">
            <div class="grow"></div>
            <button type="button" class="settings-signout-btn hairline" data-action="signout">
              ${glyphSvg("signOut", { size: 16 })}
              <span>Sign out</span>
            </button>
          </div>
        </div>
      </section>

      <footer class="settings-footer mono">
        Agent Hub 0.4.2 · build 8f21c6
      </footer>
    </form>
    `,
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
          const isSelected = b.dataset.themeVal === val;
          b.setAttribute("aria-pressed", isSelected ? "true" : "false");
          const existingCheck = b.querySelector(".settings-check");
          if (isSelected && !existingCheck) {
            b.insertAdjacentHTML("afterbegin", checkGlyph());
          } else if (!isSelected && existingCheck) {
            existingCheck.remove();
          }
        });
      }
      return;
    }

    // Density segment click
    const densityBtn = event.target.closest("button[data-density-val]");
    if (densityBtn) {
      event.preventDefault();
      const val = densityBtn.dataset.densityVal;
      savePrefs({ theme: prefs.theme, density: val, shortcuts: prefs.shortcuts });
      applyPrefs();
      const group = densityBtn.closest(".settings-segmented");
      if (group) {
        group.querySelectorAll("button[data-density-val]").forEach((b) => {
          const isSelected = b.dataset.densityVal === val;
          b.setAttribute("aria-pressed", isSelected ? "true" : "false");
          const existingCheck = b.querySelector(".settings-check");
          if (isSelected && !existingCheck) {
            b.insertAdjacentHTML("afterbegin", checkGlyph());
          } else if (!isSelected && existingCheck) {
            existingCheck.remove();
          }
        });
      }
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
