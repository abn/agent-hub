// Home: what waits on you, above the latest events across every project.

import { api } from "./api.mjs";
import { eventRow, groupedEvents, paint } from "./dom.mjs";

export async function home(gen) {
  const data = await api("/api/v1/home");
  paint(gen, `
    <h1>Home</h1>
    <div class="card"><div class="counts">
      <a class="count" href="#/inbox"><span class="n">${data.waiting}</span><span class="l">waiting on you</span></a>
      <a class="count" href="#/inbox"><span class="n">${data.unread}</span><span class="l">unread</span></a>
    </div></div>
    <nav class="toolbar" aria-label="More">
      <a class="chip" href="#/sessions">Sessions</a>
      <a class="chip" href="#/storage">Storage</a>
    </nav>
    <h2>Recent</h2>
    ${groupedEvents(data.recent, eventRow) || '<p class="empty">Nothing has happened yet.</p>'}`);
}
