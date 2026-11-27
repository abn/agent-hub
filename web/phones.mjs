// Settings > Phones: add a phone with a Tendril code, confirm the check string
// it shows, and unpair one.
//
// The hub's Tendril host makes the code, the QR picture of its signed pairing
// URI and the check string; this screen only shows them and sends the
// operator's decision back. A phone is pending until the check string is
// confirmed here, and nothing it asks for is answered before that.
//
// While the screen is open it follows the hub: every stream tick refetches the
// phones and patches Add a phone and the list in place, so a phone arriving or
// checking in moves the screen on without a reload.

import { api } from "./api.mjs";
import { confirmAction } from "./dialog.mjs";
import { confirmCopy, esc, main, paint, stale } from "./dom.mjs";
import { onTick, streamOpen, tickCount } from "./events.mjs";
import { glyphSvg } from "./glyphs.mjs";
import { render } from "./router.mjs";
import { installShellLayout, shellHTML, shellMobileBar, shellStageHead } from "./shell-layout.mjs";
import { clockTime, relative } from "./time.mjs";
import { toast } from "./toast.mjs";

// What Add a phone holds across repaints: the code on screen until it is
// closed, replaced, used or runs out; the time a code ran out unused; and the
// phone just confirmed, until Close.
let issued = null;
let ranOut = null;
let confirmed = null;

// The open screen: its render, the last listing, the pending phones it has
// already seen, and the subscription and timers it holds.
let mounted = null;

// The poll that stands in for the stream while a code is live and the stream
// is down, and how long a confirmed phone has to check in before the screen
// says so.
const POLL_MS = 3000;
const CHECK_IN_MS = 120_000;
const TITLE_COUNT = /^\(\d+\) /;

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
  return Date.parse(pairing.expires_at) <= Date.now();
}

const pending = (listing) => (listing.devices || []).filter((d) => d.state === "pending");

// A status line under a live wait: the pulsing dot and the words, announced
// politely when they change.
function statusLine(text, live) {
  const dot = live ? `<span class="phones-pulse" aria-hidden="true"></span>` : "";
  return `<p class="phones-status" role="status">${dot}${esc(text)}</p>`;
}

// Bring what Add a phone holds in line with the listing before it is drawn. A
// phone that arrives has used the code; a code past its time has run out; a
// confirmed phone that is gone or unpaired leaves nothing to wait for.
function settle(listing) {
  if (pending(listing).length) {
    issued = null;
    ranOut = null;
    confirmed = null;
  }
  if (issued && expired(issued)) {
    ranOut = issued.expires_at;
    issued = null;
  }
  if (confirmed) {
    const device = (listing.devices || []).find((d) => d.id === confirmed.id);
    if (!device || device.state !== "active") confirmed = null;
  }
}

// One line on how a phone reaches the hub away from home, when the relay
// route is on.
function relayLine(relay) {
  return relay
    ? `<p class="phones-prose">Away from home the phone connects through a public relay that sees only encrypted traffic.</p>`
    : "";
}

function addSection(listing) {
  const relay = listing.relay;
  if (!listing.enabled) {
    return `<section class="phones-add" data-frame="off">
      <h2 class="form-group-label">Add a phone</h2>
      <p class="phones-prose">Phones are off on this hub. Its operator turns them on by removing <span class="mono">HUB_PHONES=off</span>.</p>
    </section>`;
  }
  const waiting = pending(listing)[0];
  if (waiting) {
    const claimed = waiting.paired_at ? ` at ${esc(clockTime(Date.parse(waiting.paired_at)))}` : "";
    return `<section class="phones-add phones-add-waiting" data-frame="waiting" aria-labelledby="phones-add-label">
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
  if (confirmed) {
    const device = listing.devices.find((d) => d.id === confirmed.id);
    const late = Date.now() - confirmed.at >= CHECK_IN_MS;
    const status = device.checked_in_at
      ? `<p class="phones-status is-ok" role="status">${esc(device.name)} is paired</p>`
      : late
        ? statusLine(`${device.name} has not checked in yet. Open Agent Hub on the phone.`, false)
        : statusLine(`Waiting for ${device.name} to pick it up`, true);
    const waitingLine = !device.checked_in_at && !late;
    return `<section class="phones-add" data-frame="confirmed">
      <h2 class="form-group-label">Add a phone</h2>
      <p class="phones-ask phones-ok">Confirmed</p>
      ${status}
      <p class="phones-prose"${waitingLine ? "" : " hidden"}>Usually a few seconds. The phone checks every few seconds while it waits.</p>
      <div class="phones-actions"><button type="button" class="btn-outline" data-action="phones-close">Close</button></div>
    </section>`;
  }
  if (ranOut) {
    return `<section class="phones-add" data-frame="ran-out">
      <h2 class="form-group-label">Add a phone</h2>
      <p class="phones-lede">The code ran out at ${esc(clockTime(Date.parse(ranOut)))} before a phone used it.</p>
      <p class="phones-prose">Make a new one, then scan it with Agent Hub on the phone.</p>
      ${relayLine(relay)}
      <div class="phones-actions">
        <button type="button" class="primary" data-action="phones-issue">New code</button>
        <button type="button" class="btn-outline" data-action="phones-close">Close</button>
      </div>
    </section>`;
  }
  if (!issued) {
    return `<section class="phones-add" data-frame="idle">
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
  return `<section class="phones-add" data-frame="code">
    <h2 class="form-group-label">Add a phone</h2>
    <div class="phones-code">
      ${qrSvg(issued.qr)}
      <div class="phones-code-text">
        <dl class="phones-facts">
          <dt>Code</dt><dd><span class="mono phones-code-value">${esc(issued.code)}</span>${copyButton("Copy the code", issued.code)}</dd>
          ${address}
          <dt>Works until</dt><dd><span><span class="mono">${esc(until)}</span>, once</span></dd>
        </dl>
        ${statusLine("Waiting for the phone", true)}
        <p class="phones-prose">Scan with Agent Hub on the phone. This page moves on by itself when the phone reaches the hub.</p>
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

function listSection(devices) {
  const shown = devices.filter((d) => d.state !== "revoked").concat(devices.filter((d) => d.state === "revoked"));
  const list = shown.length
    ? `<ul class="phones-list">${shown.map(deviceRow).join("")}</ul>
       <p class="phones-prose">An unpaired phone stops at once if it is connected, or on its next try if it is not.</p>`
    : `<p class="phones-prose">No phone is paired with this hub.</p>`;
  return `<section class="phones-paired" aria-labelledby="phones-list-label">
      <h2 class="form-group-label phones-list-label" id="phones-list-label"><span>Phones</span><span class="mono">${shown.length}</span></h2>
      ${list}
    </section>`;
}

function metaLine(devices) {
  const active = devices.filter((d) => d.state === "active").length;
  const waiting = devices.filter((d) => d.state === "pending").length;
  return `Settings · ${active} paired${waiting ? ` · ${waiting} waiting` : ""}`;
}

// "(1) " ahead of the tab's title while a phone waits on the operator.
function titleCount(count) {
  const base = document.title.replace(TITLE_COUNT, "");
  document.title = count ? `(${count}) ${base}` : base;
}

export async function phonesScreen(gen) {
  // A tick that lands while this read is out may mean the read is already
  // behind; it is counted here and answered once the screen is up.
  const ticked = tickCount();
  const listing = await api("/api/v1/devices");
  if (stale(gen)) return;
  unmount();
  settle(listing);
  const devices = listing.devices || [];
  const meta = metaLine(devices);
  const body = `<div class="shell-pad phones">
    ${addSection(listing)}
    ${listSection(devices)}
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
  const m = { gen, listing, seen: new Set(pending(listing).map((d) => d.id)), timers: [], fetching: false, queued: false };
  m.off = onTick(() => refresh(m));
  mounted = m;
  titleCount(pending(listing).length);
  arm(m);
  if (tickCount() !== ticked) refresh(m);
}

function unmount() {
  const m = mounted;
  if (!m) return;
  mounted = null;
  m.off();
  disarm(m);
  titleCount(0);
}

function disarm(m) {
  for (const stop of m.timers) stop();
  m.timers = [];
}

// The timers the current state needs: the code running out, the check-in
// taking too long, and the poll that covers a stream that is down.
function arm(m) {
  disarm(m);
  const later = (fn, ms) => {
    const id = setTimeout(fn, Math.max(0, ms));
    m.timers.push(() => clearTimeout(id));
  };
  // At the code's time the hub is asked, not the last listing: a phone may
  // have claimed it since, and the stream may be down.
  if (issued) later(() => refresh(m), Date.parse(issued.expires_at) - Date.now() + 50);
  const device = confirmed && (m.listing.devices || []).find((d) => d.id === confirmed.id);
  const checking = Boolean(device && !device.checked_in_at);
  if (checking) later(() => update(m), confirmed.at + CHECK_IN_MS - Date.now());
  if (issued || checking) {
    const id = setInterval(() => {
      if (!streamOpen()) refresh(m);
    }, POLL_MS);
    m.timers.push(() => clearInterval(id));
  }
}

// One fetch in flight and at most one queued behind it, so a burst of ticks
// costs two requests rather than one each.
async function refresh(m) {
  if (mounted !== m) return;
  if (stale(m.gen)) {
    unmount();
    return;
  }
  if (m.fetching) {
    m.queued = true;
    return;
  }
  m.fetching = true;
  try {
    const listing = await api("/api/v1/devices");
    if (mounted === m && !stale(m.gen)) {
      m.listing = listing;
      update(m);
    }
  } catch {
    // The hub did not answer: the screen goes on from what it last read, so a
    // code past its time still says so, and the next tick or poll tries again.
    if (mounted === m && !stale(m.gen)) update(m);
  } finally {
    m.fetching = false;
    if (m.queued) {
      m.queued = false;
      refresh(m);
    }
  }
}

// Repaint from the last listing without a full render: a node that did not
// change is left alone, so focus and anything open stay where they are.
function update(m) {
  if (mounted !== m || stale(m.gen)) return;
  settle(m.listing);
  const devices = m.listing.devices || [];
  const now = pending(m.listing);
  for (const device of now) {
    if (!m.seen.has(device.id)) notifyArrival(device.name);
  }
  m.seen = new Set(now.map((d) => d.id));
  const focused = document.activeElement;
  const section = focused instanceof HTMLElement ? focused.closest(".phones-add, .phones-paired") : null;
  const add = main.querySelector(".phones .phones-add");
  if (add) morph(add, fragment(addSection(m.listing)));
  const paired = main.querySelector(".phones .phones-paired");
  if (paired) morph(paired, fragment(listSection(devices)));
  const meta = main.querySelector(".shell-meta");
  if (meta) meta.textContent = metaLine(devices);
  if (section && focused instanceof HTMLElement && !focused.isConnected) refocus(focused, section);
  titleCount(now.length);
  arm(m);
}

// Focus that was on a node a repaint replaced goes to the same control for
// the same phone when it is still there, or else to its section's label.
function refocus(was, section) {
  const zone = section.classList.contains("phones-add") ? "phones-add" : "phones-paired";
  const same = [...main.querySelectorAll(`.phones .${zone} [data-action]`)].find(
    (node) =>
      node instanceof HTMLElement &&
      node.dataset.action === was.dataset.action &&
      (node.dataset.id || "") === (was.dataset.id || ""),
  );
  const target = /** @type {HTMLElement | null} */ (
    was.dataset.action && same ? same : main.querySelector(`.phones .${zone} h2`)
  );
  if (!target) return;
  if (target.tagName === "H2") {
    target.setAttribute("tabindex", "-1");
    target.dataset.routeFocus = "";
  }
  target.focus({ preventScroll: true });
}

function fragment(html) {
  const template = document.createElement("template");
  template.innerHTML = html.trim();
  return /** @type {Element} */ (template.content.firstElementChild);
}

// Patch `from` to read as `to`. A different frame or shape is replaced
// whole; a status line keeps its node and takes the new words, so it is
// announced as a change.
function morph(from, to) {
  if (from.isEqualNode(to)) return;
  if (from.nodeType === Node.TEXT_NODE && to.nodeType === Node.TEXT_NODE) {
    from.textContent = to.textContent;
    return;
  }
  if (!(from instanceof Element) || !(to instanceof Element) || from.nodeName !== to.nodeName) {
    from.replaceWith(to);
    return;
  }
  if (from.getAttribute("data-frame") !== to.getAttribute("data-frame")) {
    from.replaceWith(to);
    return;
  }
  const status = from.getAttribute("role") === "status";
  if (!status && from.childNodes.length !== to.childNodes.length) {
    from.replaceWith(to);
    return;
  }
  for (const name of from.getAttributeNames()) if (!to.hasAttribute(name)) from.removeAttribute(name);
  for (const name of to.getAttributeNames()) from.setAttribute(name, to.getAttribute(name) || "");
  if (status) {
    from.innerHTML = to.innerHTML;
    return;
  }
  const pairs = [...from.childNodes].map((node, index) => [node, to.childNodes[index]]);
  for (const [a, b] of pairs) morph(a, b);
}

// A desktop notification for a phone that started waiting while the screen
// was open. Permission is asked for only when a code is made.
function notifyArrival(name) {
  if (typeof Notification === "undefined" || Notification.permission !== "granted") return;
  try {
    const note = new Notification(`${name} wants to pair`, { body: "Compare the check string on Settings, Phones." });
    note.onclick = () => {
      window.focus();
      note.close();
    };
  } catch {
    // A browser that shows notifications only from a service worker throws.
  }
}

async function issue() {
  // The one place the page asks to notify: the operator is making a code and
  // a phone is about to ask for them.
  if (typeof Notification !== "undefined" && Notification.permission === "default") {
    try {
      Promise.resolve(Notification.requestPermission()).catch(() => {});
    } catch {
      // An older browser takes a callback and returns nothing; asking is enough.
    }
  }
  try {
    issued = await api("/api/v1/devices/pairing", { method: "POST" });
  } catch (error) {
    toast(`No code: ${error.message}`);
    return;
  }
  ranOut = null;
  confirmed = null;
  render();
}

// Close withdraws a live code on the hub too, so a photo of it is useless.
async function closeCode() {
  const live = issued;
  issued = null;
  ranOut = null;
  confirmed = null;
  if (live) {
    try {
      await api("/api/v1/devices/pairing", { method: "DELETE" });
    } catch (error) {
      toast(`The code may still work until it runs out: ${error.message}`);
    }
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
  // A refetch still in flight read the phone as pending; dropping the mount
  // keeps it from undoing the confirmed state before the render below.
  unmount();
  issued = null;
  confirmed = { id: button.dataset.id, at: Date.now() };
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
  // Leaving the screen drops its subscription, its timers and the title count.
  window.addEventListener("hashchange", () => {
    if (!/^#\/phones(\?|$)/.test(location.hash)) unmount();
  });
}
