// The reader's own settings: the control-surface token, the theme, the
// density, and whether the single-key shortcuts fire. All of them live in
// local storage, so the app opens where it was left.

const THEMES = ["system", "light", "dark"];
const DENSITIES = ["comfortable", "compact"];
// A key that needs no modifier fires on whatever reaches the keyboard, which
// is not always the reader. It is on by default and theirs to turn off.
const SWITCH = ["on", "off"];

// Local storage is the reader's, and a browser can refuse it outright. A value
// that is not one of the ones this app writes is read as the default rather
// than put on the root element, where it would take every token with it.
function read(key, allowed, fallback) {
  let held = null;
  try {
    held = localStorage.getItem(key);
  } catch {
    return fallback;
  }
  if (allowed === null) return held || fallback;
  return allowed.includes(held) ? held : fallback;
}

function write(key, value) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // A browser that refuses storage keeps the session's own settings; there
    // is nothing further to do about the next one.
  }
}

function pick(value, allowed, fallback) {
  const wanted = String(value ?? "");
  return allowed.includes(wanted) ? wanted : fallback;
}

export const prefs = {
  token: read("hub.token", null, ""),
  theme: read("hub.theme", THEMES, "system"),
  density: read("hub.density", DENSITIES, "comfortable"),
  shortcuts: read("hub.shortcuts", SWITCH, "on"),
};

const systemDark = matchMedia("(prefers-color-scheme: dark)");

export function applyPrefs() {
  const resolved = prefs.theme === "system" ? (systemDark.matches ? "dark" : "light") : prefs.theme;
  document.documentElement.dataset.theme = resolved;
  document.documentElement.dataset.density = prefs.density;
  document.querySelector('meta[name="theme-color"]').content =
    resolved === "dark" ? "#141311" : "#F5F3EE";
}

// Following the system means following it while the app is open, not only at
// the paint that happened to come after sunset.
systemDark.addEventListener("change", () => {
  if (prefs.theme === "system") applyPrefs();
});

export function savePrefs(values) {
  prefs.token = String(values.token || "");
  prefs.theme = pick(values.theme, THEMES, "system");
  prefs.density = pick(values.density, DENSITIES, "comfortable");
  prefs.shortcuts = pick(values.shortcuts, SWITCH, "on");
  write("hub.token", prefs.token);
  write("hub.theme", prefs.theme);
  write("hub.density", prefs.density);
  write("hub.shortcuts", prefs.shortcuts);
}
