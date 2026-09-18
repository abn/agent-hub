// Inbox: the queue the human acts on, and the two actions that clear it.

import { api } from "./api.mjs";
import { composer } from "./composer.mjs";
import { confirmAction } from "./dialog.mjs";
import { actionFor, esc, glyph, paint } from "./dom.mjs";
import { render } from "./router.mjs";
import { toast } from "./toast.mjs";

function inboxRow(item) {
  const action =
    item.status === "action" || item.status === "waiting" ? actionFor(item) : "";
  return `<div class="row">
    ${glyph(item.kind)}
    <div class="grow">
      <div class="title">${esc(item.summary)}</div>
      <div class="meta">${esc(item.project_id)} · ${esc(item.actor)} · ${esc(item.status)}</div>
    </div>
    ${action}
  </div>`;
}

// Group open items by actor, preserving the incoming order, so one agent's
// queue reads as one block. The count makes a runaway agent visible without
// opening every row.
function byActor(items) {
  const groups = new Map();
  for (const item of items) {
    if (!groups.has(item.actor)) groups.set(item.actor, []);
    groups.get(item.actor).push(item);
  }
  return groups;
}

function actorGroups(items) {
  return [...byActor(items)]
    .map(
      ([actor, rows]) =>
        `<h3 class="group">${esc(actor)} <span class="group-n">${rows.length}</span></h3>
         <div class="card">${rows.map(inboxRow).join("")}</div>`,
    )
    .join("");
}

export async function inbox(gen) {
  // Read the queue the human acts on apart from unread finished work. Unread
  // items accumulate without expiry, so a single shared page could crowd the
  // waiting items off the end; the three sets are fetched independently. Each
  // actor in the waiting group gets its own block, so one agent's queue reads
  // as a block rather than a run of rows scattered through the list.
  const [action, waitingItems, unread] = await Promise.all([
    api("/api/v1/inbox?status=action&limit=500"),
    api("/api/v1/inbox?status=waiting&limit=500"),
    api("/api/v1/inbox?status=unread&limit=500"),
  ]);
  const waiting = [...action.items, ...waitingItems.items].sort(
    (a, b) => new Date(b.updated_at) - new Date(a.updated_at),
  );
  const waitingSection = waiting.length
    ? `<h2>Waiting on you</h2>${actorGroups(waiting)}`
    : "";
  const unreadSection = unread.items.length
    ? `<h2>Unread</h2><div class="card">${unread.items.map(inboxRow).join("")}</div>`
    : "";
  paint(
    gen,
    `<h1>Inbox</h1>${
      waitingSection + unreadSection ||
      '<p class="empty">Inbox is clear. Finished work, questions, and approvals will land here.</p>'
    }`,
  );
}

// Reply opens a composer under the row it belongs to, so the question stays
// readable while the answer is written and a failed send keeps the words.
// Pressing Reply again on an open composer moves back into it rather than
// stacking a second one.
export async function answer(id, button) {
  const row = button.closest(".row") || button;
  const open = row.nextElementSibling;
  if (open && open.classList.contains("composer")) {
    open.querySelector(".composer-field").focus();
    return;
  }
  const reply = composer({
    label: "Your answer",
    send: async (body) => {
      await api(`/api/v1/questions/${encodeURIComponent(id)}/answer`, {
        method: "POST",
        body: JSON.stringify({ body }),
      });
      await render();
      toast("Answer sent, recorded on the feed.");
    },
  });
  row.after(reply.element);
  button.setAttribute("aria-expanded", "true");
  reply.focus();
}

// An approval is a decision. It is recorded on the feed and leaves the waiting
// queue, so the dialog names what is approved and the toast states the result.
export async function approve(id, summary) {
  const confirmed = await confirmAction({
    title: summary ? `Approve "${summary}"?` : "Approve this action?",
    body: "Your decision is recorded on the feed and resolves the waiting item.",
    safe: "Not now",
    danger: "Approve",
    tone: "action",
  });
  if (!confirmed) return;
  try {
    await api(`/api/v1/approvals/${encodeURIComponent(id)}/decision`, {
      method: "POST",
      body: JSON.stringify({ decision: "approve" }),
    });
  } catch (error) {
    // The item may have been decided elsewhere, so the queue is reread before
    // the failure is reported.
    await render();
    throw error;
  }
  await render();
  toast("Approved, recorded on the feed.");
}
