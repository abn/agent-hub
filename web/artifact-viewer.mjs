// The public artifact host page owns the reader: theme toggle, version
// picker, unlock form, and markdown rendering. The hub app embeds this page
// in a frame; every behavior below binds one of the frozen shell ids the
// server renders, and nothing else. No inline scripts: the shell loads this
// module plus the classic vendor scripts.
import { decrypt } from "./crypto.mjs";

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

function frameStyle() {
  return (
    `<style>` +
    `html[data-theme="light"]{color-scheme:light;background:#fff;color:#1d1c19}` +
    `html[data-theme="dark"]{color-scheme:dark;background:#141311;color:#ece8e0}` +
    `body{font-family:system-ui,sans-serif;line-height:1.5;max-width:44rem;margin:0 auto;padding:1rem}` +
    `pre{background:rgba(127,127,127,.12);padding:.75rem;overflow:auto;border-radius:.375rem}` +
    `.hub-callout{border-left:.25rem solid #888;background:rgba(127,127,127,.12);` +
    `padding:.75rem 1rem;margin:1rem 0;border-radius:.375rem}` +
    `.hub-callout.note{border-color:#2f5fa8}` +
    `.hub-callout.tip{border-color:#2e7d4f}` +
    `.hub-callout.warning{border-color:#95590b}` +
    `.hub-callout.caution{border-color:#9e3b2b}` +
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

  const toggle = document.getElementById("hub-theme-toggle");
  if (toggle) {
    toggle.addEventListener("click", () => {
      state.theme = state.theme === "dark" ? "light" : "dark";
      store.write(state.theme);
      renderForTheme(state, state.theme);
    });
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
  if (form && password && errorLine) {
    password.addEventListener("input", () => {
      errorLine.textContent = "";
      errorLine.hidden = true;
    });
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      errorLine.textContent = "";
      errorLine.hidden = true;
      const envelope = readJson("hub-envelope");
      const ciphertext = readJson("hub-ciphertext");
      if (!envelope || typeof ciphertext !== "string") {
        errorLine.textContent = "This artifact has no unlock data.";
        errorLine.hidden = false;
        return;
      }
      try {
        // Nothing is fetched; the ciphertext already on the page decrypts
        // locally, so connect-src stays closed.
        const plaintext = await decrypt(password.value, envelope, ciphertext);
        state.unlocked = plaintext;
        form.hidden = true;
        renderForTheme(state, state.theme);
      } catch {
        errorLine.textContent = "Could not decrypt: check the password.";
        errorLine.hidden = false;
      }
    });
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
