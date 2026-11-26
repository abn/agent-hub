// Notifications: where the hub sends a notice when something needs the reader
// and the app is closed. One row per target, with its switch, a test send and
// delete; Add target opens the one dialog that asks for a service, a URL and how
// much the notice may say. The URL and token are written once and never read
// back: the hub answers with the URL's origin only.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { esc, main, paint } from "./dom.mjs";
import { render } from "./router.mjs";
import { shellHTML, shellStageHead } from "./shell-layout.mjs";
import { relative } from "./time.mjs";
import { toast } from "./toast.mjs";

export const CONTENT_LABELS = {
  none: "A ping only",
  title: "Kind, project and session",
  full: "The question text too",
};

const EVENT_LABELS = { ask: "Asks", deadline: "Deadlines", ended: "Ended sessions" };

// The row's second line: the service, how much it says, and what it takes.
export function targetSummary(target, presets) {
  const preset = presets.find((p) => p.preset === target.preset);
  const events = target.events.length ? target.events.map((e) => EVENT_LABELS[e] || e).join(", ") : "Tests only";
  return [preset ? preset.label : target.preset, CONTENT_LABELS[target.content] || target.content, events].join(" · ");
}

/** The raw failure reason, folded behind a disclosure; empty when the last send did not fail. */
function rawDetails(target) {
  const raw = target.status && target.status.last_error;
  return raw ? `<details class="notify-raw"><summary>Details</summary><code class="mono">${esc(raw)}</code></details>` : "";
}

// A send failure in plain words. The hub's raw reason stays behind a
// disclosure on the row for whoever needs the exact text.
export function plainError(raw) {
  const text = String(raw || "");
  const answered = text.match(/answered (\d{3})/);
  if (answered) return `The target refused it (HTTP ${answered[1]})`;
  if (/timed out/i.test(text)) return "No answer in time";
  if (/connection refused/i.test(text)) return "Could not connect (refused)";
  if (/dns|failed to lookup|name or service not known/i.test(text)) return "Could not find the host";
  if (/certificate|tls|handshake/i.test(text)) return "The secure connection failed";
  if (/connect/i.test(text)) return "Could not connect";
  return "Could not send";
}

// The row's third line: what the last send did, in ink with words.
export function targetStatus(target) {
  const status = target.status || {};
  if (status.last_error && status.last_attempt_at) {
    return `Failed ${relative(status.last_attempt_at)}: ${plainError(status.last_error)}`;
  }
  if (status.last_delivered_at) return `Delivered ${relative(status.last_delivered_at)}`;
  return "Nothing sent since the hub started";
}

function switchControl(target) {
  const id = `notify-switch-${target.id}`;
  return `<button type="button" id="${esc(id)}" role="switch" aria-labelledby="notify-name-${esc(target.id)}" aria-checked="${
    target.enabled ? "true" : "false"
  }" class="settings-switch ${target.enabled ? "on" : "off"}" data-action="notify-toggle">
    <span class="settings-switch-track"><span class="settings-switch-thumb"></span></span>
  </button>`;
}

function targetRow(target, presets) {
  const fromConfig = target.source === "config";
  const sub = fromConfig ? `${targetSummary(target, presets)} · From config.toml` : targetSummary(target, presets);
  return `<div class="form-row notify-row" data-target-id="${esc(target.id)}" data-target-name="${esc(target.name)}">
    <div class="form-row-main">
      <span class="form-row-title" id="notify-name-${esc(target.id)}">${esc(target.name)}</span>
      <span class="form-row-sub">${esc(sub)}</span>
      <span class="form-row-sub notify-status" data-notify-status>${esc(targetStatus(target))}</span>${rawDetails(target)}
      <span class="notify-row-actions">
        <button type="button" class="btn-outline" data-action="notify-test" aria-label="Send test to ${esc(target.name)}">Send test</button>
        ${fromConfig ? "" : `<button type="button" class="danger" data-action="notify-delete" aria-label="Delete ${esc(target.name)}">Delete</button>`}
      </span>
    </div>
    ${fromConfig ? "" : switchControl(target)}
  </div>`;
}

const ADD_BUTTON = `<button type="button" class="btn-outline notify-add" data-action="notify-add">Add target</button>`;

export async function notificationsScreen(gen) {
  const { targets = [], presets = [] } = await api("/api/v1/notify/targets");
  const isDesktop = window.matchMedia("(min-width: 720px)").matches;
  const meta = targets.length ? `${targets.length} ${targets.length === 1 ? "target" : "targets"}` : "Off";
  const rows = targets.length
    ? `<section class="form-group">
        <div class="form-group-label">TARGETS</div>
        ${targets.map((target) => targetRow(target, presets)).join("")}
      </section>`
    : `<p class="notify-empty">Nothing is sent while there are no targets. Add one to hear about asks with the app closed.</p>`;
  const body = `<div class="form-column notify-column">${rows}${isDesktop ? "" : ADD_BUTTON}</div>`;

  paint(
    gen,
    shellHTML({
      segment: "notifications",
      noIndex: true,
      stageHead: shellStageHead("Notifications", meta, "", "#/settings"),
      stageControls: isDesktop ? `<div class="shell-controls">${ADD_BUTTON}</div>` : "",
      stageBody: `<div class="shell-pad settings-pad">${body}</div>`,
    }),
  );
  wire(targets, presets);
}

function wire(targets, presets) {
  const stage = main.querySelector('.shell[data-segment="notifications"]');
  if (!stage) return;
  stage.addEventListener("click", async (event) => {
    const button = /** @type {HTMLElement} */ (/** @type {HTMLElement} */ (event.target).closest("[data-action]"));
    if (!button) return;
    const action = button.dataset.action;
    if (action === "notify-add") {
      event.preventDefault();
      if (await openAddTarget(presets)) await render();
      return;
    }
    const row = /** @type {HTMLElement} */ (button.closest(".notify-row"));
    if (!row) return;
    const id = row.dataset.targetId;
    const name = row.dataset.targetName;
    const target = targets.find((t) => t.id === id);
    if (action === "notify-toggle") {
      event.preventDefault();
      const next = button.getAttribute("aria-checked") !== "true";
      try {
        await api(`/api/v1/notify/targets/${encodeURIComponent(id)}`, {
          method: "PATCH",
          body: JSON.stringify({ enabled: next }),
        });
      } catch (error) {
        toast(`Could not switch ${name}: ${error.message}`);
        return;
      }
      button.setAttribute("aria-checked", String(next));
      button.classList.toggle("on", next);
      button.classList.toggle("off", !next);
      if (target) target.enabled = next;
      return;
    }
    if (action === "notify-test") {
      event.preventDefault();
      const label = button.textContent;
      /** @type {HTMLButtonElement} */ (button).disabled = true;
      button.textContent = "Sending";
      try {
        const outcome = await api(`/api/v1/notify/targets/${encodeURIComponent(id)}/test`, { method: "POST" });
        const status = row.querySelector("[data-notify-status]");
        if (status && outcome.target) {
          status.textContent = targetStatus(outcome.target);
          row.querySelector(".notify-raw")?.remove();
          status.insertAdjacentHTML("afterend", rawDetails(outcome.target));
        }
        toast(outcome.delivered ? `Test sent to ${name}.` : `The test to ${name} failed: ${outcome.error}`);
      } catch (error) {
        toast(`Could not send a test to ${name}: ${error.message}`);
      } finally {
        /** @type {HTMLButtonElement} */ (button).disabled = false;
        button.textContent = label;
      }
      return;
    }
    if (action === "notify-delete") {
      event.preventDefault();
      const confirmed = await confirmAction({
        title: `Delete ${name}?`,
        body: "The hub stops sending to it, and its URL and token are removed.",
        note: "Add it again to send to it later.",
        safe: "Keep",
        danger: "Delete target",
      });
      if (!confirmed) return;
      try {
        await api(`/api/v1/notify/targets/${encodeURIComponent(id)}`, { method: "DELETE" });
      } catch (error) {
        toast(`Could not delete ${name}: ${error.message}`);
        return;
      }
      await render();
      toast(`Deleted ${name}.`);
    }
  });
}

function field(id, label, control, note = "") {
  return `<div class="dialog-field-wrap">
    <label class="dialog-label" for="${id}">${label}</label>
    ${control}
    ${note}
  </div>`;
}

// The add dialog. It resolves true once a target is stored.
export function openAddTarget(presets) {
  const opener = document.activeElement;
  const el = document.createElement("dialog");
  el.className = "dialog notify-dialog";
  el.setAttribute("aria-labelledby", "notify-add-title");
  const options = presets.map((p) => `<option value="${esc(p.preset)}">${esc(p.label)}</option>`).join("");
  const contents = Object.entries(CONTENT_LABELS)
    .map(([value, label]) => `<option value="${value}">${esc(label)}</option>`)
    .join("");
  const checks = Object.entries(EVENT_LABELS)
    .map(
      ([value, label]) =>
        `<label class="notify-check"><input type="checkbox" name="events" value="${value}"${value === "ended" ? "" : " checked"}>${esc(label)}</label>`,
    )
    .join("");
  el.innerHTML = `<form class="dialog-form" method="dialog">
    <h2 class="dialog-title" id="notify-add-title">Add a notification target</h2>
    ${field("notify-preset", "Service", `<select id="notify-preset" name="preset">${options}</select>`)}
    ${field("notify-name", "Name", `<input id="notify-name" name="name" autocomplete="off" required maxlength="64">`)}
    ${field(
      "notify-url",
      "URL",
      `<input id="notify-url" name="url" class="notify-url" autocomplete="off" spellcheck="false" required aria-describedby="notify-url-note">`,
      `<p class="dialog-note" id="notify-url-note">Kept on the hub. Only its origin is shown again.</p>`,
    )}
    <div class="dialog-field-wrap" data-token-field>
      <label class="dialog-label" for="notify-token">Token <span class="dialog-optional">(optional)</span></label>
      <input id="notify-token" name="token" type="password" autocomplete="off">
    </div>
    ${field(
      "notify-content",
      "What it sends",
      `<select id="notify-content" name="content" aria-describedby="notify-cloud-note">${contents}</select>`,
      `<p class="dialog-note" id="notify-cloud-note" hidden></p>`,
    )}
    <fieldset class="notify-events"><legend class="dialog-label">Send on</legend>${checks}</fieldset>
    <p class="dialog-error" role="alert" hidden></p>
    <div class="dialog-actions">
      <button type="button" class="dialog-safe" data-action="cancel">Cancel</button>
      <button type="submit" class="dialog-commit action">Add target</button>
    </div>
  </form>`;

  const form = /** @type {HTMLFormElement} */ (el.querySelector("form"));
  const preset = /** @type {HTMLSelectElement} */ (el.querySelector("#notify-preset"));
  const name = /** @type {HTMLInputElement} */ (el.querySelector("#notify-name"));
  const url = /** @type {HTMLInputElement} */ (el.querySelector("#notify-url"));
  const token = /** @type {HTMLInputElement} */ (el.querySelector("#notify-token"));
  const tokenField = /** @type {HTMLElement} */ (el.querySelector("[data-token-field]"));
  const content = /** @type {HTMLSelectElement} */ (el.querySelector("#notify-content"));
  const cloudNote = /** @type {HTMLElement} */ (el.querySelector("#notify-cloud-note"));
  const problem = /** @type {HTMLElement} */ (el.querySelector(".dialog-error"));
  const commit = /** @type {HTMLButtonElement} */ (el.querySelector(".dialog-commit"));
  let named = false;

  const chosen = () => presets.find((p) => p.preset === preset.value) || presets[0];
  const sayCloud = () => {
    const info = chosen();
    const leaves = info?.vendor_cloud && content.value !== "none";
    cloudNote.hidden = !leaves;
    cloudNote.textContent = leaves ? `${info.label} is a vendor's cloud: these details leave your network.` : "";
  };
  const applyPreset = () => {
    const info = chosen();
    if (!info) return;
    url.placeholder = info.url_hint;
    tokenField.hidden = !info.takes_token;
    if (!info.takes_token) token.value = "";
    content.value = info.default_content;
    if (!named) name.value = info.label;
    sayCloud();
  };
  preset.addEventListener("change", applyPreset);
  content.addEventListener("change", sayCloud);
  name.addEventListener("input", () => {
    named = name.value.trim() !== "";
  });
  applyPreset();

  el.querySelector('[data-action="cancel"]').addEventListener("click", () => el.close("cancel"));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (commit.disabled) return;
    commit.disabled = true;
    problem.hidden = true;
    const events = [...form.querySelectorAll('input[name="events"]:checked')].map((box) => /** @type {HTMLInputElement} */ (box).value);
    try {
      await api("/api/v1/notify/targets", {
        method: "POST",
        body: JSON.stringify({
          name: name.value.trim(),
          preset: preset.value,
          url: url.value.trim(),
          token: token.value.trim() || null,
          content: content.value,
          events,
        }),
      });
    } catch (error) {
      problem.textContent = error.message;
      problem.hidden = false;
      commit.disabled = false;
      return;
    }
    el.close("added");
  });

  el.addEventListener("keydown", (event) => {
    if (event.key !== "Tab") return;
    const ring = [...el.querySelectorAll("select, input, button:not([disabled])")].filter((node) => !node.closest("[hidden]"));
    if (!ring.length) return;
    const edge = event.shiftKey ? ring[0] : ring[ring.length - 1];
    if (document.activeElement !== edge) return;
    event.preventDefault();
    /** @type {HTMLElement} */ ((event.shiftKey ? ring[ring.length - 1] : ring[0])).focus();
  });

  document.body.appendChild(el);
  document.documentElement.classList.add("has-dialog");
  el.showModal();
  preset.focus();

  return new Promise((resolve) => {
    el.addEventListener(
      "close",
      () => {
        const added = el.returnValue === "added";
        el.remove();
        document.documentElement.classList.remove("has-dialog");
        if (opener instanceof HTMLElement && opener.isConnected) opener.focus({ preventScroll: true });
        if (added) toast("Target added. Send a test to check it.");
        resolve(added);
      },
      { once: true },
    );
  });
}
