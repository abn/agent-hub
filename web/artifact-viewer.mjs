// The public artifact host page owns the reader: theme toggle, version
// picker, unlock form, and markdown rendering. The hub app embeds this page
// in a frame; every behavior below binds one of the frozen shell ids the
// server renders, and nothing else. No inline scripts: the shell loads this
// module plus the classic vendor scripts.
import { decrypt, UNSUPPORTED_ENVELOPE } from "./crypto.mjs";

const THEME_KEY = "hub-artifact-theme";

// localStorage is unavailable inside the opaque-origin viewer frame, so the
// theme falls back to memory when the store throws.
function themeStore() {
  let memory = null;
  return {
    read() {
      try {
        return window.localStorage.getItem(THEME_KEY);
      } catch {
        return memory;
      }
    },
    write(value) {
      memory = value;
      try {
        window.localStorage.setItem(THEME_KEY, value);
      } catch {}
    },
  };
}

function preferredTheme() {
  try {
    if (window.matchMedia("(prefers-color-scheme: dark)").matches) return "dark";
  } catch {}
  return "light";
}

function escHtml(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function readJson(id) {
  const el = document.getElementById(id);
  if (!el) return null;
  try {
    return JSON.parse(el.textContent);
  } catch {
    return null;
  }
}

// marked passes raw HTML through by default, so the html renderer is
// overridden to escape it. This matches the server renderer's total-escape
// contract: authored angle brackets stay text, never markup.
function parseMarkdown(source) {
  const lib = globalThis.marked;
  if (!lib || typeof lib.parse !== "function" || typeof lib.Marked !== "function") {
    return null;
  }
  const engine = new lib.Marked();
  engine.use({
    renderer: {
      html(token) {
        return escHtml(token.raw != null ? token.raw : token.text || "");
      },
    },
  });
  return engine.parse(source);
}

// A blockquote whose first paragraph starts with a marker becomes a callout
// aside; the marker line is dropped and any trailing paragraphs are kept.
function renderCallouts(html) {
  return html.replace(
    /<blockquote>\s*<p>\[!(NOTE|TIP|WARNING|CAUTION)\]([\s\S]*?)<\/p>([\s\S]*?)<\/blockquote>/g,
    (match, marker, first, tail) => {
      const body = first.replace(/^\s*(<br\s*\/?>)?/, "").trim();
      const head = body ? `<p>${body}</p>` : "";
      return `<aside class="hub-callout ${marker.toLowerCase()}">${head}${tail}</aside>`;
    },
  );
}

// Fenced mermaid blocks become placeholders the frame loader upgrades. The
// fence content is already escaped by the parser, so it passes through.
function renderMermaidPlaceholders(html) {
  return html.replace(
    /<pre><code class="language-mermaid">([\s\S]*?)<\/code><\/pre>/g,
    (match, body) => `<pre class="mermaid">${body}</pre>`,
  );
}

// Frame loader tag. The shared file also sizes the frame to its body,
// which the host cannot measure across the opaque origin.
function frameLoader() {
  return `<script src="/frame-loader.js"></script>`;
}

// Mirror of web/tokens.css values for the srcdoc frame: an opaque origin
// cannot load the file, so the frame carries the values inline. Body copy
// follows the foundation prose scale: 15px/1.6 ink-2, headings ink, 640px
// measure. The static check fails on any colour here that tokens.css does not
// declare, so the mirror cannot drift into a second palette.
function frameStyle() {
  return (
    `<style>` +
    `html[data-theme="light"]{color-scheme:light;background:#F5F3EE;color:#1D1C19}` +
    `html[data-theme="dark"]{color-scheme:dark;background:#141311;color:#ECE8E0}` +
    `body{font-family:"Avenir Next","Seravek","Segoe UI Variable Text","Segoe UI",Ubuntu,Cantarell,system-ui,sans-serif;` +
    `font-size:15px;line-height:1.6;color:#5C584F;max-width:640px;margin:0 auto;padding:1.5rem 1.25rem 4rem}` +
    `html[data-theme="dark"] body{color:#B3ADA2}` +
    `h1,h2,h3{color:#1D1C19;line-height:1.3;text-wrap:balance}` +
    `html[data-theme="dark"] h1,html[data-theme="dark"] h2,html[data-theme="dark"] h3{color:#ECE8E0}` +
    `a{color:#2F5FA8}` +
    `html[data-theme="dark"] a{color:#8AAAE8}` +
    `pre{background:#EDEAE3;border:1px solid #E4E0D8;border-radius:6px;padding:.75rem 1rem;overflow:auto}` +
    `html[data-theme="dark"] pre{background:#26241F;border-color:#2F2C26}` +
    `code{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:.9em}` +
    `:not(pre)>code{background:#EDEAE3;border:1px solid #E4E0D8;border-radius:4px;padding:.1em .35em}` +
    `html[data-theme="dark"] :not(pre)>code{background:#26241F;border-color:#2F2C26}` +
    `table{border-collapse:collapse;display:block;overflow-x:auto}` +
    `th,td{border:1px solid #E4E0D8;padding:.4rem .7rem;text-align:left}` +
    `html[data-theme="dark"] th,html[data-theme="dark"] td{border-color:#2F2C26}` +
    `blockquote{margin:0;padding-left:1rem;border-left:3px solid #E4E0D8;color:#6F6A61}` +
    `html[data-theme="dark"] blockquote{border-color:#2F2C26;color:#948E83}` +
    `.hub-callout{background:#EDEAE3;border-left:.25rem solid #CFC9BE;border-radius:6px;` +
    `padding:.75rem 1rem;margin:1rem 0}` +
    `html[data-theme="dark"] .hub-callout{background:#26241F;border-color:#44403A}` +
    `.hub-callout.note{border-color:#2F5FA8}` +
    `html[data-theme="dark"] .hub-callout.note{border-color:#8AAAE8}` +
    `.hub-callout.tip{border-color:#2E7D4F}` +
    `html[data-theme="dark"] .hub-callout.tip{border-color:#6FC08C}` +
    `.hub-callout.warning{border-color:#95590B}` +
    `html[data-theme="dark"] .hub-callout.warning{border-color:#DDA44E}` +
    `.hub-callout.caution{border-color:#9E3B2B}` +
    `html[data-theme="dark"] .hub-callout.caution{border-color:#E08373}` +
    `</style>`
  );
}

// The srcdoc frame is sandboxed by the host iframe attribute. The meta
// policy restates the frame directives so the document is self-describing;
// the inherited host policy applies on top, and their intersection governs.
// `frame-ancestors` is deliberately absent: it is ignored in a meta element
// and the sandbox plus host headers own framing instead.
function buildSrcdoc({ title, body, theme, withMermaid }) {
  const origin = window.location.origin;
  const csp =
    "default-src 'none'; script-src " +
    `${origin}; style-src 'unsafe-inline'; img-src data: blob:; ` +
    "font-src data:; media-src data: blob:; connect-src 'none'; " +
    "form-action 'none'; base-uri 'none'";
  const loader =
    (withMermaid ? `<script src="/vendor/mermaid.runtime.js"></script>` : "") +
    frameLoader();
  return (
    `<!doctype html><html lang="en" data-theme="${theme}"><head>` +
    `<meta charset="utf-8">` +
    `<meta name="viewport" content="width=device-width, initial-scale=1">` +
    `<meta http-equiv="Content-Security-Policy" content="${csp}">` +
    `<title>${escHtml(title)}</title>${frameStyle()}</head>` +
    `<body>${body}${loader}</body></html>`
  );
}

function showMarkdown(frame, meta, source, theme) {
  const parsed = parseMarkdown(source);
  if (parsed === null) {
    frame.srcdoc = buildSrcdoc({
      title: meta.title,
      body: `<pre>${escHtml(source)}</pre>`,
      theme,
      withMermaid: false,
    });
    return;
  }
  const body = renderMermaidPlaceholders(renderCallouts(parsed));
  frame.srcdoc = buildSrcdoc({
    title: meta.title,
    body,
    theme,
    withMermaid: body.includes('<pre class="mermaid">'),
  });
}

function frameUrl(meta, theme) {
  const base = `/artifacts/${encodeURIComponent(meta.id)}/frame`;
  const params = new URLSearchParams();
  if (meta.version != null) params.set("version", String(meta.version));
  params.set("theme", theme);
  return `${base}?${params.toString()}`;
}

// Rebuild artifact state for the theme, per contract. Reloading the frame
// or rebuilding the srcdoc wipes in-artifact script state such as form
// input; acceptable for v1.
function renderForTheme(state, theme) {
  const { frame, meta, unlocked } = state;
  document.documentElement.setAttribute("data-theme", theme);
  if (!frame || !meta) return;
  if (unlocked != null) {
    if (meta.kind === "markdown") showMarkdown(frame, meta, unlocked, theme);
    else frame.srcdoc = unlocked;
    return;
  }
  const source = readJson("hub-markdown-body");
  if (typeof source === "string") {
    showMarkdown(frame, meta, source, theme);
    return;
  }
  if (meta.kind === "html") frame.src = frameUrl(meta, theme);
}

// Whether this document may keep anything at all. Inside the viewer frame the
// origin is opaque and every access throws, so the probe runs once and the
// gate offers remembering only where it can actually happen.
function storageWorks() {
  const probe = "hub-artifact-probe";
  try {
    window.localStorage.setItem(probe, "1");
    window.localStorage.removeItem(probe);
    return true;
  } catch {
    return false;
  }
}

// Passwords remembered per project, device-local, never sent anywhere, and
// held as typed: any key that could encrypt them would sit in the same store.
// One JSON object under a single key; storage failures drop the write
// rather than breaking unlock.
function passwordStore() {
  const key = "hub-artifact-passwords";
  const readAll = () => {
    try {
      const raw = window.localStorage.getItem(key);
      const parsed = raw ? JSON.parse(raw) : {};
      return parsed && typeof parsed === "object" ? parsed : {};
    } catch {
      return {};
    }
  };
  return {
    read(projectId) {
      const passwords = readAll();
      const saved = passwords[projectId];
      return typeof saved === "string" ? saved : null;
    },
    write(projectId, secret) {
      const passwords = readAll();
      passwords[projectId] = secret;
      try {
        window.localStorage.setItem(key, JSON.stringify(passwords));
      } catch {}
    },
    remove(projectId) {
      const passwords = readAll();
      delete passwords[projectId];
      try {
        window.localStorage.setItem(key, JSON.stringify(passwords));
      } catch {}
    },
  };
}

function init() {
  const meta = readJson("hub-meta");
  if (!meta) return;
  const store = themeStore();
  const stored = store.read();
  const state = {
    frame: document.getElementById("hub-frame"),
    meta,
    unlocked: null,
    theme: stored === "light" || stored === "dark" ? stored : preferredTheme(),
  };
  document.documentElement.setAttribute("data-theme", state.theme);

  const back = document.getElementById("hub-back");
  if (back) {
    if (window.history.length > 1) back.hidden = false;
    back.addEventListener("click", () => window.history.back());
  }

  const toggle = document.getElementById("hub-theme-toggle");
  const showThemeIcon = () => {
    const svgs = toggle ? toggle.querySelectorAll("svg") : [];
    // The first glyph names the active theme: sun in light, moon in dark.
    svgs.forEach((svg, index) => {
      svg.hidden = state.theme === "dark" ? index === 0 : index !== 0;
    });
    if (toggle) {
      toggle.setAttribute(
        "aria-label",
        state.theme === "dark" ? "Switch to light theme" : "Switch to dark theme",
      );
    }
  };
  if (toggle) {
    toggle.addEventListener("click", () => {
      state.theme = state.theme === "dark" ? "light" : "dark";
      store.write(state.theme);
      showThemeIcon();
      renderForTheme(state, state.theme);
    });
    showThemeIcon();
  }

  // Size the frame to its body. The frame is opaque, so the host cannot
  // measure it; the shared frame loader posts its height instead. Only
  // the frame element's own messages are honored, and the height is
  // clamped to a sane band.
  window.addEventListener("message", (event) => {
    if (!state.frame || event.source !== state.frame.contentWindow) return;
    const height = event.data && event.data.hubFrameHeight;
    if (typeof height !== "number" || !isFinite(height)) return;
    const clamped = Math.min(Math.max(Math.round(height), 120), 12000);
    state.frame.style.height = `${clamped}px`;
  });

  const pickerWrap = document.getElementById("hub-picker-wrap");
  const picker = document.getElementById("hub-version-select");
  if (pickerWrap && picker) {
    picker.addEventListener("change", () => {
      const version = picker.value;
      const target = version
        ? `${window.location.pathname}?version=${encodeURIComponent(version)}`
        : window.location.pathname;
      window.location.href = target;
    });
  }

  const form = document.getElementById("hub-unlock-form");
  const password = document.getElementById("hub-password");
  const errorLine = document.getElementById("hub-unlock-error");
  const forget = document.getElementById("hub-forget");
  const forgetNote = document.getElementById("hub-forget-note");
  const passwords = passwordStore();
  let remember = document.getElementById("hub-remember");
  // An option that cannot work is not offered: where the store throws, the
  // checkbox leaves the page rather than sitting there doing nothing.
  if (remember && !storageWorks()) {
    (remember.closest(".hub-remember") || remember).remove();
    remember = null;
  }
  // The gate that remembered a password is hidden once it unlocks by itself,
  // so forgetting needs its own control in the chrome.
  if (forget) {
    forget.addEventListener("click", () => {
      if (meta.project_id) passwords.remove(meta.project_id);
      forget.hidden = true;
      if (forgetNote) {
        forgetNote.textContent =
          "Password forgotten on this device. This artifact will ask for it again.";
      }
    });
  }
  // Returns "ok", "retry" when another attempt could work, or "unsupported"
  // when no password can open this envelope.
  const unlock = async (secret) => {
    const envelope = readJson("hub-envelope");
    const ciphertext = readJson("hub-ciphertext");
      if (!envelope || typeof ciphertext !== "string") {
        errorLine.textContent = "This artifact has no unlock data.";
        errorLine.hidden = false;
        password.focus();
        return "retry";
      }
    try {
      // Nothing is fetched; the ciphertext already on the page decrypts
      // locally, so connect-src stays closed.
      state.unlocked = await decrypt(secret, envelope, ciphertext);
      form.hidden = true;
      const copy = form.closest(".hub-gate")?.querySelector(".hub-gate-copy");
      if (copy) copy.hidden = true;
      renderForTheme(state, state.theme);
      return "ok";
    } catch (error) {
      if (error?.code === UNSUPPORTED_ENVELOPE) {
        // Leave the field as it is: focusing it would read as an invitation
        // to try a password again, and no password opens this envelope.
        errorLine.textContent =
          "This artifact was encrypted with settings this viewer does not accept. Retyping the password will not open it.";
        errorLine.hidden = false;
        return "unsupported";
      }
      errorLine.textContent = "Wrong password. Nothing was sent anywhere.";
      errorLine.hidden = false;
      password.focus();
      return "retry";
    }
  };
  if (form && password && errorLine) {
    password.addEventListener("input", () => {
      errorLine.textContent = "";
      errorLine.hidden = true;
    });
    const attempt = async () => {
      errorLine.textContent = "";
      errorLine.hidden = true;
      const secret = password.value;
      const result = await unlock(secret);
      if (remember && meta.project_id) {
        if (result === "ok" && remember.checked) {
          passwords.write(meta.project_id, secret);
          if (forget) forget.hidden = false;
        } else if (!remember.checked) passwords.remove(meta.project_id);
      }
      if (result !== "unsupported") password.value = "";
      if (result === "ok") password.blur();
    };
    // The app embeds this page in a frame sandboxed without form permission,
    // where a submission is blocked before any submit event fires. Unlocking
    // hangs off the button's activation, which Enter in the field reaches
    // too, and the form itself never submits anywhere.
    form.addEventListener("submit", (event) => event.preventDefault());
    const unlockButton = form.querySelector('button[type="submit"]');
    if (unlockButton) {
      unlockButton.addEventListener("click", (event) => {
        event.preventDefault();
        attempt();
      });
    }
    // A remembered password unlocks without asking again. A stale one
    // fails silently back to the form and is forgotten; an envelope this
    // viewer will not open says so and keeps the password, which is not
    // what failed.
    if (meta.project_id) {
      const saved = passwords.read(meta.project_id);
      if (saved) {
        if (remember) remember.checked = true;
        unlock(saved).then((result) => {
          if (result === "retry") {
            passwords.remove(meta.project_id);
            password.value = "";
            password.focus();
            return;
          }
          // Opened, or refused for a reason the password cannot fix: either
          // way it is still remembered, so offer to forget it.
          if (forget) forget.hidden = false;
        });
      }
    }
  }

  renderForTheme(state, state.theme);
}

if (typeof document !== "undefined") {
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
}
