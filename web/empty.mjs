// The empty state a screen shows when it has nothing: quiet, never a failure.
// Four parts and no more, in the design's order: the screen's own name, one
// line of what this is, one line of what to do, and at most one way to act.
// No illustration.

// The copy the design settles for each screen. A screen names its entry and
// supplies the values a line needs, so the strings live in one place rather
// than inside eight render functions.
//
// The design gives six of these. Storage and Project settings carry the copy
// the app already ships, split into the same two lines, because the design
// leaves those two empty states undrawn.
export const EMPTY_COPY = {
  home: {
    screen: "home",
    title: "Quiet night.",
    body: "Nothing is waiting on you.",
  },
  inbox: {
    screen: "inbox",
    title: "Inbox is clear.",
    body: "Finished work and questions from your agents will land here.",
    link: "Show read items",
  },
  feed: {
    screen: "project feed",
    title: "No events yet in {project}.",
    body: "Agents post here over MCP. The project slug is {project}.",
    link: "Copy MCP setup",
  },
  artifacts: {
    screen: "artifacts",
    title: "No artifacts.",
    body: "Documents your agents produce appear here, versioned.",
  },
  sessions: {
    screen: "sessions",
    title: "No sessions yet.",
    body: "A session starts when an agent first writes to its brain.",
  },
  search: {
    screen: "search",
    title: "Search your own machine.",
    body: "Feed, artifacts and session brains, indexed locally. Nothing leaves this machine.",
  },
  storage: {
    screen: "storage",
    title: "Nothing is stored yet.",
    body: "Session brains and artifact blobs appear here as agents work.",
  },
  settings: {
    screen: "settings",
    title: "No projects can be deleted.",
  },
};

// A line can name something the screen knows and the table cannot, such as the
// project the reader is looking at. A name with no value keeps its braces
// rather than printing a hole.
function fill(text, values) {
  return String(text ?? "").replace(/\{(\w+)\}/g, (whole, name) =>
    name in values ? String(values[name]) : whole,
  );
}

function action(label, where) {
  if (!where) return null;
  const element = document.createElement(where.href ? "a" : "button");
  element.className = "empty-link";
  element.textContent = label;
  if (where.href) element.href = where.href;
  else {
    element.type = "button";
    element.dataset.action = where.action;
    if (where.id) element.dataset.id = where.id;
  }
  return element;
}

// The node form. `where` is what the screen knows and the copy does not: the
// link's destination, or the delegated action its button carries. A label with
// nowhere to go is left out rather than drawn as a control that does nothing.
export function emptyState(copy, values = {}, where = null) {
  const box = document.createElement("div");
  box.className = "empty-state";
  const name = document.createElement("p");
  name.className = "empty-screen";
  name.textContent = copy.screen;
  const title = document.createElement("h2");
  title.className = "empty-title";
  title.textContent = fill(copy.title, values);
  box.append(name, title);
  if (copy.body) {
    const body = document.createElement("p");
    body.className = "empty-body";
    body.textContent = fill(copy.body, values);
    box.appendChild(body);
  }
  const link = copy.link && action(copy.link, where);
  if (link) box.appendChild(link);
  return box;
}

// The string form, for the screens that paint a card as one innerHTML string.
// Serialising the element is what escapes it: the browser's own serialiser
// quotes the attributes and the text it holds.
export function emptyStateHTML(copy, values = {}, where = null) {
  return emptyState(copy, values, where).outerHTML;
}
