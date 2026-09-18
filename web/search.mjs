// Search: one box over the feed, artifacts, and session brains.

import { api } from "./api.mjs";
import { esc, paint } from "./dom.mjs";

export async function searchScreen(term, gen) {
  let results = "";
  if (term) {
    const data = await api(`/api/v1/search?q=${encodeURIComponent(term)}`);
    results = data.groups.length
      ? data.groups
          .map(
            (group) =>
              `<h2>${esc(group.kind)}</h2><div class="card">${group.hits
                .map(
                  (hit) =>
                    `<div class="row"><div class="grow"><div class="title">${esc(hit.title || hit.ref_id)}</div><div class="meta">${esc(hit.snippet)}</div><div class="meta mono">${esc(hit.project_id)}</div></div></div>`,
                )
                .join("")}</div>`,
          )
          .join("")
      : '<p class="empty">No matches.</p>';
  }
  paint(
    gen,
    `
    <h1>Search</h1>
    <form class="toolbar" data-action="search">
      <label class="sr-only" for="q">Search</label>
      <input id="q" name="q" type="search" value="${esc(term || "")}" placeholder="Search your own machine" autocomplete="off">
      <button class="primary" type="submit">Search</button>
    </form>
    ${results}`,
  );
}
