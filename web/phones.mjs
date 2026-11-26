// Settings > Phones: add a phone with a Tendril code, confirm the check string
// it shows, and unpair one.
//
// The hub's Tendril host makes the code, the QR picture of its signed pairing
// URI and the check string; this screen only shows them and sends the
// operator's decision back. A phone is pending until the check string is
// confirmed here, and nothing it asks for is answered before that.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { confirmCopy, esc, main, paint } from "./dom.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { render } from "./router.mjs";
import { installShellLayout, shellHTML, shellMobileBar, shellStageHead } from "./shell-layout.mjs";
import { clockTime, relative } from "./time.mjs";
import { toast } from "./toast.mjs";

// The code on screen, kept across the repaints a tick causes until it is
// closed, replaced or runs out.
let issued = null;

const PHONE_GLYPH =
  '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="7" y="3" width="10" height="18" rx="2"/><path d="M 11 17.5h2"/></svg>';

function copyButton(label, value) {
  return `<button type="button" class="phones-copy" data-action="phones-copy" data-value="${esc(value)}" aria-label="${esc(label)}">${glyphSvg("copy", { size: 16 })}</button>`;
}

// The QR modules as one path over a size-by-size grid, with the four-module
// quiet zone a reader needs around it. Dark is ink, light is the page's
// surface, in both themes.
function qrSvg(qr) {
  const span = qr.size + 8;
  return `<svg class="phones-qr" viewBox="-4 -4 ${span} ${span}" role="img" aria-label="Pairing code as a QR picture" shape-rendering="crispEdges"><rect x="-4" y="-4" width="${span}" height="${span}" class="phones-qr-bg"/><path d="${esc(qr.path)}" class="phones-qr-ink"/></svg>`;
}

function expired(pairing) {
  return !pairing || Date.parse(pairing.expires_at) <= Date.now();
}

// One line on how a phone reaches the hub away from home, when the relay
// route is on.
function relayLine(relay) {
  return relay
    ? `<p class="phones-prose">Away from home the phone connects through a public relay that sees only encrypted traffic.</p>`
    : "";
}

function addSection(enabled, waiting, relay) {
  if (!enabled) {
    return `<section class="phones-add">
      <h2 class="form-group-label">Add a phone</h2>
      <p class="phones-prose">Phones are off on this hub. Its operator turns them on by removing <span class="mono">HUB_PHONES=off</span>.</p>
    </section>`;
  }
  if (waiting) {
    const claimed = waiting.paired_at ? ` at ${esc(clockTime(Date.parse(waiting.paired_at)))}` : "";
    return `<section class="phones-add phones-add-waiting" aria-labelledby="phones-add-label">
      <h2 class="form-group-label phones-label-waiting" id="phones-add-label">Add a phone</h2>
      <p class="phones-ask"><strong>${esc(waiting.name)}</strong> wants to pair <span class="phones-chip">Waiting on you</span></p>
      <p class="phones-meta mono">paired${claimed}</p>
      <p class="phones-check-label" id="phones-check-label">Check string</p>
      <p class="phones-check mono" aria-describedby="phones-check-label">${esc(waiting.check_string || "")}</p>
      <p class="phones-prose">The phone shows a check string too. Confirm only if the two are the same. If they differ, refuse: something stands between this hub and the phone.</p>
      <div class="phones-actions">
        <button type="button" class="primary" data-action="phones-confirm" data-id="${esc(waiting.id)}" data-check="${esc(waiting.check_string || "")}">${glyphSvg("resolve", { size: 16, strokeWidth: 2 })} They match, confirm</button>
        <button type="button" class="btn-outline" data-action="phones-refuse" data-id="${esc(waiting.id)}">They differ, refuse</button>
      </div>
    </section>`;
  }
  if (expired(issued)) {
    issued = null;
    return `<section class="phones-add">
      <h2 class="form-group-label">Add a phone</h2>
      <p class="phones-prose">Make a code, then scan it with Agent Hub on the phone.</p>
      ${relayLine(relay)}
      <div class="phones-actions"><button type="button" class="primary" data-action="phones-issue">Add a phone</button></div>
    </section>`;
  }
  const until = clockTime(Date.parse(issued.expires_at));
  const address = issued.address
    ? `<dt>Address</dt><dd><span class="mono">${esc(issued.address)}</span>${copyButton("Copy the address", issued.address)}</dd>`
    : "";
  return `<section class="phones-add">
    <h2 class="form-group-label">Add a phone</h2>
    <div class="phones-code">
      ${qrSvg(issued.qr)}
      <div class="phones-code-text">
        <p class="phones-lede">Scan with Agent Hub on the phone.</p>
        <dl class="phones-facts">
          <dt>Code</dt><dd><span class="mono phones-code-value">${esc(issued.code)}</span>${copyButton("Copy the code", issued.code)}</dd>
          ${address}
          <dt>Works until</dt><dd><span><span class="mono">${esc(until)}</span>, once</span></dd>
        </dl>
        <p class="phones-prose">${issued.address ? "The address is only for typing the code by hand. " : ""}Anyone who sees this code can try it until it is used or runs out; nothing is granted until you confirm a check string here.</p>
        ${relayLine(relay)}
        <div class="phones-actions">
          <button type="button" class="btn-outline" data-action="phones-issue">New code</button>
          <button type="button" class="btn-outline" data-action="phones-close">Close</button>
        </div>
      </div>
    </div>
  </section>`;
}

function stateWord(device) {
  if (device.state === "active") return `<span class="phones-state is-active"><span class="phones-dot" aria-hidden="true"></span>active</span>`;
  if (device.state === "pending") return `<span class="phones-state is-pending"><span class="phones-ring" aria-hidden="true"></span>pending</span>`;
  return `<span class="phones-state is-revoked">unpaired</span>`;
}

function deviceRow(device) {
  const meta =
    device.state === "pending"
      ? `paired ${esc(relative(device.paired_at))} · nothing works until confirmed`
      : device.state === "revoked"
        ? `paired ${esc(relative(device.paired_at))} · unpaired ${esc(relative(device.revoked_at))}`
        : `paired ${esc(relative(device.confirmed_at || device.paired_at))}${device.last_seen_at ? ` · seen ${esc(relative(device.last_seen_at))}` : ""}`;
  const action =
    device.state === "active"
      ? `<button type="button" class="btn-outline" data-action="phones-revoke" data-id="${esc(device.id)}" data-name="${esc(device.name)}">Unpair…</button>`
      : "";
  return `<li class="phones-row">
    <span class="phones-glyph">${PHONE_GLYPH}</span>
    <div class="phones-row-main">
      <div class="phones-row-title"><span class="phones-name">${esc(device.name)}</span> ${stateWord(device)}</div>
      <div class="phones-meta mono">${meta}</div>
    </div>
    ${action}
  </li>`;
}

export async function phonesScreen(gen) {
  const listing = await api("/api/v1/devices");
  const devices = listing.devices || [];
  const waiting = devices.find((d) => d.state === "pending") || null;
  const active = devices.filter((d) => d.state === "active").length;
  const pending = devices.filter((d) => d.state === "pending").length;
  const meta = `Settings · ${active} paired${pending ? ` · ${pending} waiting` : ""}`;
  const shown = devices.filter((d) => d.state !== "revoked").concat(devices.filter((d) => d.state === "revoked"));

  const list = shown.length
    ? `<ul class="phones-list">${shown.map(deviceRow).join("")}</ul>
       <p class="phones-prose">An unpaired phone stops at once if it is connected, or on its next try if it is not.</p>`
    : `<p class="phones-prose">No phone is paired with this hub.</p>`;
  const body = `<div class="shell-pad phones">
    ${addSection(listing.enabled, waiting, listing.relay)}
    <section class="phones-paired" aria-labelledby="phones-list-label">
      <h2 class="form-group-label phones-list-label" id="phones-list-label"><span>Phones</span><span class="mono">${shown.length}</span></h2>
      ${list}
    </section>
  </div>`;

  const isDesktop = window.matchMedia("(min-width: 720px)").matches;
  paint(
    gen,
    shellHTML({
      segment: "settings",
      noIndex: true,
      stageHead: isDesktop
        ? shellStageHead("Phones", meta, "", "#/settings")
        : shellMobileBar("Phones", meta, "#/settings"),
      stageControls: isDesktop
        ? `<div class="shell-controls"><nav class="phones-crumb" aria-label="Path"><a href="#/settings">Settings</a><span aria-hidden="true">/</span><span>Phones</span></nav></div>`
        : "",
      stageBody: body,
    }),
  );
  installShellLayout(main);
}

async function issue() {
  try {
    issued = await api("/api/v1/devices/pairing", { method: "POST" });
  } catch (error) {
    toast(`No code: ${error.message}`);
    return;
  }
  render();
}

// Close withdraws the code on the hub too, so a photo of it is useless.
async function closeCode() {
  issued = null;
  try {
    await api("/api/v1/devices/pairing", { method: "DELETE" });
  } catch (error) {
    toast(`The code may still work until it runs out: ${error.message}`);
  }
  render();
}

async function confirmPhone(button) {
  try {
    await api(`/api/v1/devices/${encodeURIComponent(button.dataset.id)}/confirm`, {
      method: "POST",
      body: JSON.stringify({ check_string: button.dataset.check }),
    });
  } catch (error) {
    toast(`Not confirmed: ${error.message}`);
    render();
    return;
  }
  issued = null;
  toast("Phone confirmed");
  render();
}

async function refuse(button) {
  try {
    await api(`/api/v1/devices/${encodeURIComponent(button.dataset.id)}/refuse`, { method: "POST" });
  } catch (error) {
    toast(`Nothing changed: ${error.message}`);
  }
  render();
}

async function revoke(button) {
  const id = button.dataset.id;
  const ok = await confirmAction({
    title: `Unpair ${button.dataset.name}?`,
    body: "It stops at once if it is connected, or on its next try if it is not. Pairing it again needs a new code.",
    safe: "Keep",
    danger: "Unpair",
    tone: "action",
    commit: () => api(`/api/v1/devices/${encodeURIComponent(id)}`, { method: "DELETE" }),
  });
  if (ok) render();
}

if (typeof document !== "undefined") {
  document.addEventListener("click", (event) => {
    const target = /** @type {HTMLElement} */ (event.target).closest?.("[data-action^='phones-']");
    if (!(target instanceof HTMLElement)) return;
    const action = target.dataset.action;
    if (action === "phones-issue") issue();
    else if (action === "phones-close") closeCode();
    else if (action === "phones-confirm") confirmPhone(target);
    else if (action === "phones-refuse") refuse(target);
    else if (action === "phones-revoke") revoke(target);
    else if (action === "phones-copy") confirmCopy(target, target.dataset.value || "");
  });
}
