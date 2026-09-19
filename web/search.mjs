// Search: one box over the feed, artifacts, and session brains. Results follow
// the field as it is typed in, the query lives in the route, and everything an
// agent wrote reaches the screen as a text node.

import { api } from "./api.mjs";
import { glyph, main, stale } from "./dom.mjs";
import { EMPTY_COPY, emptyState } from "./empty.mjs";
import { registerScreen } from "./keys.mjs";
import { relative } from "./time.mjs";

// The design asks for results as you type inside 50 ms.
const DEBOUNCE = 50;

// The scopes the search route can filter by, as its `type` parameter.
const SCOPES = [
  { type: "", label: "All" },
  { type: "feed", label: "Feed" },
  { type: "artifact", label: "Artifacts" },
  { type: "brain", label: "Sessions" },
];

// A corpus family as the reader knows it, and the kind badge its rows draw. A
// feed hit does not say which kind of event it is, so it draws the neutral
// mark and leaves the naming to its group header.
const FAMILIES = {
  feed: { label: "Feed", badge: "signal", named: false },
  artifact: { label: "Artifacts", badge: "artifact", named: true },
  brain: { label: "Sessions", badge: "session", named: true },
  kb: { label: "Wiki", badge: "artifact", named: false },
};

const NO_MATCH = {
  screen: "search",
  title: "Nothing matches {term}.",
  body: "Search matches whole words. Try another word or a wider scope.",
};

const SVG = "http://www.w3.org/2000/svg";

registerScreen("search", { rows: ".search-row" });

// The newest query asked. An answer to any other is dropped, so a slow one
// cannot paint over the results of what the reader typed after it.
let asked = 0;

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}

function icon(size, d) {
  const svg = document.createElementNS(SVG, "svg");
  for (const [name, value] of Object.entries({
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    "stroke-width": "1.8",
    "stroke-linecap": "round",
    "stroke-linejoin": "round",
    "aria-hidden": "true",
  })) {
    svg.setAttribute(name, value);
  }
  const path = document.createElementNS(SVG, "path");
  path.setAttribute("d", d);
  svg.appendChild(path);
  return svg;
}

// The words of a query. The index reads quotes, brackets and the upper-case
// operators as syntax and refuses a query that is only half of one, which is
// what a reader typing `"caddy` has written. The index splits text on the same
// boundaries, so the words are what it would have matched anyway.
export function terms(query) {
  return String(query ?? "")
    .toLowerCase()
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean);
}

// The snippet as text nodes, with each matched word inside a mark. Nothing
// here is parsed as HTML, so a snippet that carries markup stays the text the
// agent wrote. The words are letters and digits by construction and are
// escaped for the pattern anyway, so a change to `terms` cannot make one a
// pattern of its own.
export function highlighted(snippet, words) {
  const text = String(snippet ?? "");
  const out = document.createDocumentFragment();
  if (!words.length) {
    out.append(text);
    return out;
  }
  const needle = [...words]
    .sort((a, b) => b.length - a.length)
    .map((word) => word.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
    .join("|");
  // A word matches from a word's start, as the index matches it, rather than
  // anywhere inside a longer one. The boundary is a captured group and not a
  // lookbehind, which older engines refuse to compile.
  const pattern = new RegExp(`(^|[^\\p{L}\\p{N}])(${needle})`, "giu");
  const starts = [];
  for (let match = pattern.exec(text); match; match = pattern.exec(text)) {
    starts.push([match.index + match[1].length, match[2].length]);
    // The boundary character may also be the one that opens the next word.
    pattern.lastIndex = match.index + match[1].length + match[2].length;
  }
  // The hub's snippet is the opening of the body. When the first hit sits far
  // into it, the excerpt starts a few words before the hit instead.
  let at = 0;
  if (starts.length && starts[0][0] > 60) {
    const first = starts[0][0];
    const space = text.indexOf(" ", first - 40);
    at = space >= 0 && space < first ? space + 1 : first;
    out.append("…");
  }
  for (const [index, length] of starts) {
    out.append(text.slice(at, index), el("mark", "", text.slice(index, index + length)));
    at = index + length;
  }
  out.append(text.slice(at));
  return out;
}

function destination(hit) {
  const project = encodeURIComponent(hit.project_id);
  if (hit.kind === "feed") return `#/projects/${project}/feed`;
  if (hit.kind === "artifact") return `#/artifacts/${encodeURIComponent(hit.ref_id)}?project=${project}`;
  if (hit.kind === "brain" && hit.session_id) {
    return `#/session?project=${project}&id=${encodeURIComponent(hit.session_id)}`;
  }
  return "";
}

function whereLine(hit) {
  if (hit.kind === "brain" && hit.session_id) {
    return `${hit.project_id} · session ${hit.session_id.slice(0, 8)}`;
  }
  return hit.project_id;
}

function badge(family) {
  // The kind is one of this module's own literals, never a field of the hit.
  const holder = document.createElement("template");
  holder.innerHTML = glyph(family.badge);
  if (!family.named) holder.content.querySelector(".sr-only")?.remove();
  return holder.content;
}

function resultRow(hit, words) {
  const family = FAMILIES[hit.kind] || FAMILIES.feed;
  const row = el("div", "row search-row");
  row.appendChild(badge(family));
  const href = destination(hit);
  const body = el(href ? "a" : "div", "search-link grow");
  if (href) body.href = href;
  const head = el("span", "search-head");
  head.appendChild(el("span", "title", hit.title || hit.ref_id));
  const changed = Date.parse(hit.updated_at);
  if (Number.isFinite(changed)) head.appendChild(el("span", "search-time mono", relative(changed)));
  const snippet = el("span", "search-snippet");
  snippet.appendChild(highlighted(hit.snippet, words));
  body.append(head, snippet, el("span", "search-where", whereLine(hit)));
  row.appendChild(body);
  return row;
}

// The count is the hub's own. A page the limit cut short is said to be the
// first of more, never printed as a total. The time is the hub's measurement
// of the index query, not a clock run here.
function resultsLine(data) {
  const count = data.truncated
    ? `First ${data.count} results`
    : `${data.count} ${data.count === 1 ? "result" : "results"}`;
  const took = data.took_ms > 0 ? `${data.took_ms} ms` : "under 1 ms";
  return `${count} · ${took} · local index`;
}

function resultsNode(state) {
  const box = el("div", "search-results");
  if (!state.term) {
    box.appendChild(emptyState(EMPTY_COPY.search));
  } else if (state.error) {
    box.appendChild(el("p", "error", state.error));
  } else if (!state.data.count) {
    box.appendChild(emptyState(NO_MATCH, { term: `“${state.term}”` }));
  } else {
    const words = terms(state.term);
    for (const group of state.data.groups) {
      const family = FAMILIES[group.kind];
      box.appendChild(el("h2", "section-label", `${family ? family.label : group.kind} · ${group.count}`));
      const card = el("div", "card search-group");
      for (const hit of group.hits) card.appendChild(resultRow(hit, words));
      box.appendChild(card);
    }
  }
  return box;
}

// What a query found, or why it found nothing. A query with no word in it is
// not sent: there is nothing the index could match, and no time to report.
async function find(term, type) {
  const state = { term, data: { count: 0, truncated: false, groups: [] }, line: "", error: "" };
  if (!term) return state;
  const words = terms(term);
  if (!words.length) {
    state.line = "0 results";
    return state;
  }
  const scope = type ? `&type=${encodeURIComponent(type)}` : "";
  try {
    state.data = await api(`/api/v1/search?q=${encodeURIComponent(words.join(" "))}${scope}`);
    state.line = resultsLine(state.data);
  } catch (error) {
    state.error = error.message;
  }
  return state;
}

function routeFor(term, type) {
  const params = new URLSearchParams();
  if (term) params.set("q", term);
  if (type) params.set("type", type);
  const query = params.toString();
  return query ? `#/search?${query}` : "#/search";
}

export async function searchScreen(term, gen) {
  const params = new URLSearchParams(location.hash.split("?")[1] || "");
  const asScope = SCOPES.find((scope) => scope.type === params.get("type"));
  let type = asScope ? asScope.type : "";
  // The first paint carries the results with the heading, so a route that
  // names a query arrives whole.
  asked += 1;
  const first = await find((term || "").trim(), type);
  if (stale(gen)) return;

  const title = el("h1", "", "Search");
  const form = el("form", "search-form");
  form.setAttribute("role", "search");
  const label = el("label", "sr-only", "Search the feed, artifacts and session brains");
  label.htmlFor = "q";
  const field = el("div", "search-field");
  const input = el("input");
  Object.assign(input, {
    id: "q",
    name: "q",
    type: "text",
    value: term || "",
    placeholder: "Search your own machine",
    autocomplete: "off",
    spellcheck: false,
    enterKeyHint: "search",
  });
  input.setAttribute("autocapitalize", "none");
  const clear = el("button", "search-clear");
  clear.type = "button";
  clear.setAttribute("aria-label", "Clear search");
  clear.appendChild(icon(16, "M 6 6l12 12 M 18 6L6 18"));
  clear.hidden = !input.value;
  field.append(icon(18, "M 11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14z M 16 16l4 4"), input, clear);
  form.append(label, field);

  const scopes = el("div", "search-scopes");
  scopes.setAttribute("role", "group");
  scopes.setAttribute("aria-label", "Search in");
  for (const scope of SCOPES) {
    const chip = el("button", "chip", scope.label);
    chip.type = "button";
    chip.dataset.scope = scope.type;
    chip.setAttribute("aria-pressed", String(scope.type === type));
    scopes.appendChild(chip);
  }

  // A live region that is on the page before its text changes, so the count is
  // read out after each query without focus leaving the field.
  const line = el("p", "search-line mono", first.line);
  line.setAttribute("role", "status");
  line.setAttribute("aria-live", "polite");
  let results = resultsNode(first);

  function show(state) {
    line.textContent = state.line;
    const next = resultsNode(state);
    // Replaced as a child of the region, which is what the keyboard map
    // watches to find the new rows.
    results.replaceWith(next);
    results = next;
  }

  let timer = 0;
  async function run() {
    clearTimeout(timer);
    // A keystroke's timer can outlive the screen. The route then belongs to
    // whichever screen replaced this one.
    if (!line.isConnected) return;
    const typed = input.value.trim();
    clear.hidden = !input.value;
    // Replaced rather than pushed: Back leaves the screen instead of walking
    // through every letter, and a reload or a return lands on this query.
    history.replaceState(null, "", routeFor(typed, type));
    const mine = ++asked;
    const state = await find(typed, type);
    if (mine !== asked || !line.isConnected) return;
    show(state);
  }

  input.addEventListener("input", () => {
    clearTimeout(timer);
    clear.hidden = !input.value;
    if (input.value.trim()) timer = setTimeout(run, DEBOUNCE);
    else run();
  });
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    run();
  });
  clear.addEventListener("click", () => {
    input.value = "";
    input.focus();
    run();
  });
  scopes.addEventListener("click", (event) => {
    const chip = event.target.closest("button[data-scope]");
    if (!chip) return;
    type = chip.dataset.scope;
    for (const other of scopes.children) {
      other.setAttribute("aria-pressed", String(other === chip));
    }
    run();
  });

  main.replaceChildren(title, form, scopes, line, results);
}
