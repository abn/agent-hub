// The reader's own settings: the control-surface token, the theme, and the
// density. All three live in local storage, so the app opens where it was left.

export const prefs = {
  token: localStorage.getItem("hub.token") || "",
  theme: localStorage.getItem("hub.theme") || "system",
  density: localStorage.getItem("hub.density") || "comfortable",
};

export function applyPrefs() {
  const resolved =
    prefs.theme === "system"
      ? matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light"
      : prefs.theme;
  document.documentElement.dataset.theme = resolved;
  document.documentElement.dataset.density = prefs.density;
  document.querySelector('meta[name="theme-color"]').content =
    resolved === "dark" ? "#141311" : "#F5F3EE";
}

export function savePrefs(values) {
  prefs.token = String(values.token || "");
  prefs.theme = String(values.theme || "system");
  prefs.density = String(values.density || "comfortable");
  localStorage.setItem("hub.token", prefs.token);
  localStorage.setItem("hub.theme", prefs.theme);
  localStorage.setItem("hub.density", prefs.density);
}
