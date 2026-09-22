// Search: one box over the feed, artifacts, and session brains. Results follow
// the field as it is typed in, the query lives in the route, and everything an
// agent wrote reaches the screen as a text node.

import { api } from "./api.mjs";
import { installShellLayout } from "./shell-layout.mjs";
import { glyph, main, projectName, stale } from "./dom.mjs";
import { EMPTY_COPY, emptyState } from "./empty.mjs";
import { registerScreen } from "./keys.mjs";
import { formatBytes } from "./storage.mjs";
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
// feed hit draws the badge of the event it is and says its kind; one that
// does not say which kind it is draws the neutral mark and leaves the naming
// to its group header.
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

// The words of a query, for marking them in a snippet. The index splits text
// on the same boundaries, so these are the words it matched. They are not what
// is sent: the hub reads the query as typed.
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

const said = (value) => (typeof value === "string" && value ? value : "");

// Where the hit lives and what it is, in words: the project, then what only
// its family carries. A feed hit names who wrote it; an artifact its current
// version and size; a brain hit its session by name and whether that session
// is active or ended, as a word, since nothing else on the row says it. A key
// the hit does not carry adds nothing to the line. The line is set as text.
function whereLine(hit) {
  const parts = [projectName(hit)];
  if (hit.kind === "feed") {
    parts.push(said(hit.actor));
  } else if (hit.kind === "artifact") {
    if (Number.isFinite(hit.version)) parts.push(`v${hit.version}`);
    if (Number.isFinite(hit.size_bytes)) parts.push(formatBytes(hit.size_bytes));
  } else if (hit.kind === "brain" && hit.session_id) {
    parts.push(`session ${said(hit.session_name) || hit.session_id.slice(0, 8)}`, said(hit.session_status));
  }
  return parts.filter(Boolean).join(" · ");
}

// A family's badge is one of this module's own literals. A feed hit's is the
// event's kind, which an agent chose: `glyph` escapes it into the attribute
// and the label, and a kind it has no mark for draws the neutral one under
// the kind's own word.
function badge(hit, family) {
  const kind = hit.kind === "feed" ? said(hit.event_kind) : "";
  const holder = document.createElement("template");
  holder.innerHTML = glyph(kind || family.badge);
  if (!kind && !family.named) holder.content.querySelector(".sr-only")?.remove();
  return holder.content;
}

function resultRow(hit, words) {
  const family = FAMILIES[hit.kind] || FAMILIES.feed;
  const row = el("div", "row search-row");
  row.appendChild(badge(hit, family));
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

// What a query found, or why it found nothing. The query goes to the hub as it
// was typed: the hub keeps a balanced phrase as a phrase and drops whatever
// else the index would read as syntax, so nothing typed here is refused. Only
// an empty field is not sent.
async function find(term, type, project) {
  const trimmed = (term ?? "").trim();
  const state = { term: trimmed, data: { count: 0, truncated: false, groups: [] }, line: "", error: "" };
  if (!trimmed) return state;
  const scope = type ? `&type=${encodeURIComponent(type)}` : "";
  const proj = project ? `&project=${encodeURIComponent(project)}` : "";
  try {
    state.data = await api(`/api/v1/search?q=${encodeURIComponent(trimmed)}${scope}${proj}`);
    state.line = resultsLine(state.data);
  } catch (error) {
    state.error = error.message;
  }
  return state;
}

function routeFor(term, type, project) {
  const params = new URLSearchParams();
  if (term) params.set("q", term);
  if (type) params.set("type", type);
  if (project) params.set("project", project);
  const query = params.toString();
  return query ? `#/search?${query}` : "#/search";
}

export function previewHighlighted(text, words) {
  const str = String(text ?? "");
  const frag = document.createDocumentFragment();
  if (!words.length || !str) {
    frag.append(str);
    return frag;
  }
  const needle = [...words]
    .sort((a, b) => b.length - a.length)
    .map((word) => word.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
    .join("|");
  const pattern = new RegExp(`(^|[^\\p{L}\\p{N}])(${needle})`, "giu");
  let at = 0;
  for (let match = pattern.exec(str); match; match = pattern.exec(str)) {
    const start = match.index + match[1].length;
    const len = match[2].length;
    frag.append(str.slice(at, start));
    const m = el("mark", "search-match", str.slice(start, start + len));
    frag.append(m);
    at = start + len;
    pattern.lastIndex = start + len;
  }
  frag.append(str.slice(at));
  return frag;
}

function previewPath(hit) {
  const p = projectName(hit) || hit.project_id;
  const fam = FAMILIES[hit.kind] ? FAMILIES[hit.kind].label.toLowerCase() : (hit.kind || "feed");
  const name = hit.title || hit.session_name || hit.ref_id || "untitled";
  return `${p} / ${fam} / ${name}`;
}

function renderPreview(stagePane, hit, words) {
  stagePane.innerHTML = "";
  if (!hit) {
    const empty = el("div", "search-preview-empty", "Select a result to preview.");
    stagePane.appendChild(empty);
    return;
  }

  const head = el("div", "search-stage-head");
  const path = el("span", "search-stage-path", previewPath(hit));
  const counter = el("span", "search-match-counter", "");

  const prevBtn = el("button", "search-step-btn");
  prevBtn.type = "button";
  prevBtn.setAttribute("aria-label", "Previous match");
  prevBtn.appendChild(icon(14, "M 18 15 l -6 -6 -6 6"));

  const nextBtn = el("button", "search-step-btn");
  nextBtn.type = "button";
  nextBtn.setAttribute("aria-label", "Next match");
  nextBtn.appendChild(icon(14, "M 6 9 l 6 6 6 -6"));

  const href = destination(hit);
  const openBtn = el(href ? "a" : "button", "button primary search-open-btn", "Open");
  if (href) openBtn.href = href;

  head.append(path, counter, prevBtn, nextBtn, openBtn);

  const content = el("div", "search-stage-content");
  const article = el("article", "search-preview-article prose");

  const titleText = hit.title || hit.ref_id || hit.session_name || "Untitled";
  const title = el("h2", "search-preview-title");
  title.appendChild(previewHighlighted(titleText, words));

  const metaText = whereLine(hit);
  const meta = el("div", "search-preview-meta mono", metaText);

  const body = el("div", "search-preview-body");
  if (hit.snippet) {
    const p = el("p", "");
    p.appendChild(previewHighlighted(hit.snippet, words));
    body.appendChild(p);
  }

  article.append(title, meta, body);
  content.appendChild(article);
  stagePane.append(head, content);

  const matchElements = [...article.querySelectorAll(".search-match")];
  const totalMatches = matchElements.length;
  let activeIndex = 0;

  function updateMatch(idx) {
    if (totalMatches === 0) {
      counter.textContent = "0 matches";
      prevBtn.disabled = true;
      nextBtn.disabled = true;
      return;
    }
    prevBtn.disabled = false;
    nextBtn.disabled = false;
    activeIndex = (idx + totalMatches) % totalMatches;
    counter.textContent = `match ${activeIndex + 1} of ${totalMatches}`;
    matchElements.forEach((m, i) => {
      if (i === activeIndex) {
        m.classList.add("active");
        m.setAttribute("aria-current", "true");
      } else {
        m.classList.remove("active");
        m.removeAttribute("aria-current");
      }
    });
    matchElements[activeIndex].scrollIntoView({ block: "nearest", behavior: "smooth" });
  }

  prevBtn.addEventListener("click", () => updateMatch(activeIndex - 1));
  nextBtn.addEventListener("click", () => updateMatch(activeIndex + 1));

  updateMatch(0);

  if (hit.kind === "artifact" && hit.ref_id) {
    api(`/api/v1/artifacts/${encodeURIComponent(hit.ref_id)}?project=${encodeURIComponent(hit.project_id)}`)
      .then((art) => {
        if (!art || !art.content || stagePane.querySelector(".search-stage-path")?.textContent !== previewPath(hit)) return;
        body.innerHTML = "";
        const lines = String(art.content).split(/\n+/);
        for (const line of lines) {
          if (!line.trim()) continue;
          const p = el("p", "");
          p.appendChild(previewHighlighted(line, words));
          body.appendChild(p);
        }
        const updatedMatches = [...article.querySelectorAll(".search-match")];
        if (updatedMatches.length) {
          matchElements.length = 0;
          matchElements.push(...updatedMatches);
          updateMatch(0);
        }
      })
      .catch(() => {});
  }
}

export async function searchScreen(term, gen) {
  const params = new URLSearchParams(location.hash.split("?")[1] || "");
  const asScope = SCOPES.find((scope) => scope.type === params.get("type"));
  let type = asScope ? asScope.type : "";
  let project = params.get("project") || "";

  asked += 1;
  const first = await find((term || "").trim(), type, project);
  if (stale(gen)) return;

  const title = el("h1", "search-title", "Search");
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

  function renderProjectChip() {
    scopes.querySelector(".search-scope-divider")?.remove();
    scopes.querySelector(".search-project-chip")?.remove();
    if (!project) return;
    const divider = el("span", "search-scope-divider");
    divider.setAttribute("aria-hidden", "true");
    const pChip = el("button", "chip search-project-chip");
    pChip.type = "button";
    pChip.setAttribute("aria-label", `Remove filter: project ${project}`);
    const textSpan = el("span", "", `in ${project}`);
    const closeIcon = icon(12, "M 6 6l12 12 M 18 6L6 18");
    pChip.append(textSpan, closeIcon);
    pChip.addEventListener("click", () => {
      project = "";
      renderProjectChip();
      run();
    });
    scopes.append(divider, pChip);
  }
  renderProjectChip();

  const line = el("p", "search-line mono", first.line);
  line.setAttribute("role", "status");
  line.setAttribute("aria-live", "polite");

  // The one shell: the results are the index, the preview is the stage.
  const indexHead = el("div", "shell-head");
  const headTitle = el("div", "shell-title");
  title.classList.add("shell-title-line");
  headTitle.append(title, line);
  indexHead.append(el("span", "shell-slot"), headTitle, form);

  const indexControls = el("div", "shell-controls");
  indexControls.append(scopes);

  const indexBody = el("div", "shell-body");
  const indexCol = el("div", "shell-index");
  indexCol.append(indexHead, indexControls, indexBody);
  // The results container keeps the name the rest of the screen uses for it.
  const indexPane = indexBody;

  const stagePane = el("div", "shell-stage");
  const split = el("div", "shell-split");
  split.setAttribute("data-split", "index");
  split.setAttribute("role", "separator");
  split.setAttribute("aria-orientation", "vertical");
  split.setAttribute("aria-label", "Resize list");
  split.tabIndex = 0;

  const layout = el("div", "shell");
  layout.append(indexCol, split, stagePane);

  let selectedHitIndex = 0;
  let allHits = [];

  function selectHit(idx, focus = false) {
    if (!allHits.length) {
      renderPreview(stagePane, null, []);
      return;
    }
    selectedHitIndex = Math.max(0, Math.min(allHits.length - 1, idx));
    const rows = [...indexPane.querySelectorAll(".search-row")];
    rows.forEach((row, i) => {
      const isCur = i === selectedHitIndex;
      row.classList.toggle("is-selected", isCur);
      if (isCur) {
        row.setAttribute("aria-current", "page");
        row.tabIndex = 0;
        if (focus) row.focus();
      } else {
        row.removeAttribute("aria-current");
        row.tabIndex = -1;
      }
    });
    const currentWords = terms(input.value);
    renderPreview(stagePane, allHits[selectedHitIndex], currentWords);
  }

  function updateResults(state) {
    line.textContent = state.line;
    const nextResults = resultsNode(state);
    indexPane.innerHTML = "";
    indexPane.appendChild(nextResults);

    allHits = (state.data?.groups || []).flatMap((g) => g.hits);
    const rows = [...indexPane.querySelectorAll(".search-row")];
    rows.forEach((row, i) => {
      const isDesktop = window.innerWidth >= 1100;
      row.dataset.index = String(i);
      if (i === 0) {
        row.tabIndex = 0;
        if (isDesktop) {
          row.classList.add("is-selected");
          row.setAttribute("aria-current", "page");
        }
      } else {
        row.tabIndex = -1;
      }
    });

    if (allHits.length > 0) {
      selectedHitIndex = 0;
      renderPreview(stagePane, allHits[0], terms(state.term));
    } else {
      renderPreview(stagePane, null, []);
    }
    // The preview is a desktop pane. A phone shows the results and opens the
    // hit where it lives, so the shell keeps its index.
    layout.classList.toggle("has-selection", allHits.length > 0 && window.innerWidth >= 1100);
  }

  updateResults(first);

  let clickedWasSelected = false;

  indexPane.addEventListener("pointerdown", (event) => {
    const row = event.target.closest(".search-row");
    if (!row) return;
    const idx = parseInt(row.dataset.index, 10);
    clickedWasSelected = idx === selectedHitIndex;
  });

  indexPane.addEventListener("click", (event) => {
    const row = event.target.closest(".search-row");
    if (!row) return;
    const idx = parseInt(row.dataset.index, 10);
    if (isNaN(idx)) return;
    const isDesktop = window.innerWidth >= 1100;
    if (isDesktop && event.detail > 0) {
      if (clickedWasSelected) {
        return;
      }
      event.preventDefault();
      selectHit(idx, true);
    }
  });

  indexPane.addEventListener("focusin", (event) => {
    const row = event.target.closest(".search-row");
    if (!row) return;
    const idx = parseInt(row.dataset.index, 10);
    if (!isNaN(idx) && idx !== selectedHitIndex) {
      selectHit(idx, false);
    }
  });

  let timer = 0;
  async function run() {
    clearTimeout(timer);
    if (!line.isConnected || !location.hash.startsWith("#/search")) return;
    const typed = input.value.trim();
    clear.hidden = !input.value;
    history.replaceState(history.state, "", routeFor(typed, type, project));
    const mine = ++asked;
    const state = await find(typed, type, project);
    if (mine !== asked || !line.isConnected) return;
    updateResults(state);
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
    for (const other of scopes.querySelectorAll("button[data-scope]")) {
      other.setAttribute("aria-pressed", String(other === chip));
    }
    run();
  });

  main.replaceChildren(layout);
  installShellLayout(main);
}
