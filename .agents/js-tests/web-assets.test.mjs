// The build and asset checks: what the shell ships, what it may not carry, and
// what the vendor manifest promises. These read the files and assert a parsed
// or structural property, so a rename or a move does not fail a check with no
// defect behind it.

import { describe, expect, it } from "vitest";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { basename, dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { GLYPH_PATHS } from "../../web/glyphs.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const WEB = join(ROOT, "web");
const VENDOR = join(WEB, "vendor");
// The table the hub serves the shell from. Every first-party script has to be
// in it, or the browser asks for a module the binary does not carry.
const ASSET_TABLE = join(ROOT, "src", "http", "web.rs");

/** Every file under `dir`, the vendor bundle excluded. */
function walk(dir) {
  const found = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (full === VENDOR) continue;
      found.push(...walk(full));
    } else if (entry.isFile()) {
      found.push(full);
    }
  }
  return found;
}

/** Every script under web/ this project wrote, vendor bytes excluded. */
function firstPartyScripts() {
  return walk(WEB).filter((path) => /\.(?:mjs|js)$/.test(path));
}

const toPosix = (path) => path.split(sep).join("/");

// Every asset the shell needs. A missing one is a screen that renders nothing
// or a 404 the browser only meets at runtime.
const REQUIRED = [
  "index.html",
  "app.js",
  "app.css",
  "tokens.css",
  "artifact-shell.css",
  "crypto.mjs",
  "artifact-viewer.mjs",
  "frame-loader.js",
  "vendor/marked.js",
  "vendor/mermaid.runtime.js",
  "vendor/MANIFEST.json",
  "manifest.webmanifest",
  "sw.js",
  "icon.svg",
];

// Vendored bytes are reviewed at vendoring time, not on every check, but the
// licence the manifest claims has to be in the file it ships.
const LICENSE_MARKERS = [
  ["vendor/marked.js", "MIT Licensed"],
  ["vendor/mermaid.runtime.js", "Bundled license information"],
];

// A URL in an XML namespace declaration is not a fetched asset.
const EXTERNAL = /https?:\/\/(?!www\.w3\.org)/;
// True emoji ranges. Typographic marks the design mandates (a check, a bullet,
// a question mark) are allowed and live outside these.
const EMOJI = /[\u{1F000}-\u{1FAFF}\u{2600}-\u{26FF}]/u;
const INLINE_HANDLER = /\son[a-z]+\s*=/i;

describe("the shell ships what it serves", () => {
  it("carries every required asset", () => {
    const missing = REQUIRED.filter((name) => !existsSync(join(WEB, name)));
    expect(missing).toEqual([]);
  });

  it("keeps the licence marker the manifest claims for a vendored script", () => {
    const missing = LICENSE_MARKERS.filter(
      ([name, marker]) =>
        existsSync(join(WEB, name)) && !readFileSync(join(WEB, name), "utf8").includes(marker),
    );
    expect(missing.map(([name]) => name)).toEqual([]);
  });

  it("carries no external asset origin and no emoji", () => {
    const errors = [];
    for (const path of walk(WEB)) {
      const lines = readFileSync(path, "utf8").split("\n");
      lines.forEach((line, index) => {
        if (EXTERNAL.test(line)) {
          errors.push(`${relative(ROOT, path)}:${index + 1}: external URL is not allowed (${line.trim()})`);
        }
        if (EMOJI.test(line)) {
          errors.push(`${relative(ROOT, path)}:${index + 1}: emoji is not allowed`);
        }
      });
    }
    expect(errors).toEqual([]);
  });

  it("ships a shell with the accessibility basics", () => {
    const index = readFileSync(join(WEB, "index.html"), "utf8");
    const errors = [];
    if (!/<html[^>]*\blang=/.test(index)) errors.push("web/index.html: <html> has no lang attribute");
    if (!index.includes("viewport")) errors.push("web/index.html: no viewport meta");
    if (!index.includes("skip-link")) errors.push("web/index.html: no skip link");
    if (!index.includes('rel="manifest"') || !index.includes("manifest.webmanifest")) {
      errors.push("web/index.html: does not link the manifest");
    }
    if (/(?:href|src)="\/[^"]/.test(index)) {
      errors.push(
        "web/index.html: an absolute path from the origin root 404s once the shell " +
          "is served behind a path-stripping proxy",
      );
    }
    if (INLINE_HANDLER.test(index)) {
      errors.push("web/index.html: inline event handlers are not allowed");
    }
    expect(errors).toEqual([]);
  });

  it("registers the service worker from the entry", () => {
    const entry = readFileSync(join(WEB, "app.js"), "utf8");
    expect(
      /navigator\.serviceWorker\s*\.\s*register\(/.test(entry),
      "web/app.js does not register the service worker",
    ).toBe(true);
  });

  it("keeps the placeholders the server stamps into the worker", () => {
    // The hub fills these in from the assets it embeds. Hand-writing either one
    // back pins the installed shell to whatever the last edit said.
    const worker = readFileSync(join(WEB, "sw.js"), "utf8");
    const missing = ["{{version}}", "{{assets}}", "{{on_demand}}"].filter(
      (placeholder) => !worker.includes(placeholder),
    );
    expect(missing).toEqual([]);
  });
});

describe("the scripts are parseable and served", () => {
  it("parses every first-party script", () => {
    // The text scans cannot catch a broken module, and a screen that fails to
    // parse renders nothing. The test runs under node, so the parser is here.
    const failures = [];
    for (const path of firstPartyScripts()) {
      try {
        execFileSync(process.execPath, ["--check", path], { stdio: "pipe" });
      } catch (error) {
        const detail = String(error.stderr ?? error.message).trim();
        failures.push(`${relative(ROOT, path)}: node cannot parse it (${detail})`);
      }
    }
    expect(failures).toEqual([]);
  });

  it("serves every first-party script from the asset table", () => {
    const source = readFileSync(ASSET_TABLE, "utf8");
    const embedded = new Set(
      [...source.matchAll(/include_(?:str|bytes)!\("\.\.\/\.\.\/web\/([^"]+)"\)/g)].map(
        (found) => found[1],
      ),
    );
    const unserved = firstPartyScripts()
      .map((path) => toPosix(relative(WEB, path)))
      .filter((name) => !embedded.has(name));
    expect(unserved).toEqual([]);
  });
});

// The three modals the browser owns. The app asks and reports in its own
// components, so none of them may come back. A name that merely ends in one of
// the words, such as `confirmAction(`, is this project's own and is left alone.
const NATIVE_MODAL = /(?<![.\w$])(?:window\s*\.\s*)?(alert|confirm|prompt)\s*\(/;

/**
 * Every line with its comments cut out, numbered from one. A word inside a
 * comment is prose about the code, not a call. This is a reader's pass rather
 * than a parser, and it errs towards leaving code in.
 */
function withoutComments(text) {
  const lines = [];
  let inBlock = false;
  for (const [index, line] of text.split("\n").entries()) {
    let kept = "";
    let rest = line;
    while (rest) {
      if (inBlock) {
        const close = rest.indexOf("*/");
        if (close === -1) break;
        rest = rest.slice(close + 2);
        inBlock = false;
        continue;
      }
      const opening = rest.indexOf("/*");
      const slashes = rest.indexOf("//");
      if (slashes >= 0 && (opening === -1 || slashes < opening)) {
        kept += rest.slice(0, slashes);
        break;
      }
      if (opening === -1) {
        kept += rest;
        break;
      }
      kept += rest.slice(0, opening);
      rest = rest.slice(opening + 2);
      inBlock = true;
    }
    lines.push([index + 1, kept]);
  }
  return lines;
}

describe("the app asks in its own components", () => {
  it("never calls a browser modal from a first-party script", () => {
    const errors = [];
    for (const path of firstPartyScripts()) {
      for (const [number, line] of withoutComments(readFileSync(path, "utf8"))) {
        const found = NATIVE_MODAL.exec(line);
        if (found) {
          errors.push(
            `${relative(ROOT, path)}:${number}: ${found[1]}() is the browser's own modal; ` +
              "the app asks and reports in its own components",
          );
        }
      }
    }
    expect(errors).toEqual([]);
  });
});

/** Every glyph name the app asks for, against the set it declares. */
function checkGlyphNames() {
  const errors = [];
  const known = new Set(Object.keys(GLYPH_PATHS));
  if (!known.size) {
    return ["web/glyphs.mjs declares no glyphs, so the table is not being read"];
  }
  const asked = new Set();
  for (const path of firstPartyScripts()) {
    if (basename(path) === "glyphs.mjs") continue;
    const lines = readFileSync(path, "utf8").split("\n");
    lines.forEach((line, index) => {
      for (const match of line.matchAll(/glyphSvg\(\s*["']([A-Za-z]+)["']/g)) {
        asked.add(match[1]);
        if (!known.has(match[1])) {
          errors.push(
            `${relative(ROOT, path)}:${index + 1}: asks for the glyph ${JSON.stringify(match[1])}, ` +
              "which web/glyphs.mjs does not have; it would render an empty control",
          );
        }
      }
    });
  }

  // And the other direction. A glyph nothing draws is how the set grew without
  // anyone looking at it, and how two names came to hold byte-identical paths.
  for (const name of [...known].sort()) {
    if (!asked.has(name)) {
      errors.push(
        `web/glyphs.mjs: the glyph ${JSON.stringify(name)} is declared and nothing draws it; ` +
          "retire it or draw it, because an unused entry is one no review sees",
      );
    }
  }

  // Two names for one drawing is two entries in the set and one motif in the
  // app. The reader learns a distinction the interface does not make.
  const byDrawing = new Map();
  for (const [name, drawing] of Object.entries(GLYPH_PATHS)) {
    const key = drawing.trim();
    if (!byDrawing.has(key)) byDrawing.set(key, []);
    byDrawing.get(key).push(name);
  }
  for (const names of byDrawing.values()) {
    if (names.length > 1) {
      errors.push(
        `web/glyphs.mjs: ${names.slice().sort().map((name) => JSON.stringify(name)).join(" and ")} ` +
          "are the same drawing under different names; keep one",
      );
    }
  }
  return errors;
}

describe("the glyph set is drawn and used", () => {
  it("has a glyph for every name asked for, and draws every glyph it declares", () => {
    expect(checkGlyphNames()).toEqual([]);
  });
});

const VENDOR_LICENSES = new Set(["MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "MPL-2.0"]);

/** Every vendored file against its manifest entry: name, release, licence, hash. */
function checkVendorManifest() {
  const manifestPath = join(VENDOR, "MANIFEST.json");
  if (!existsSync(manifestPath)) return ["web/vendor/MANIFEST.json is missing"];
  let data;
  try {
    data = JSON.parse(readFileSync(manifestPath, "utf8"));
  } catch (error) {
    return [`web/vendor/MANIFEST.json: invalid JSON (${error.message})`];
  }

  let entries;
  if (Array.isArray(data)) entries = data;
  else if (data && typeof data === "object") {
    entries = Array.isArray(data.files)
      ? data.files
      : Object.entries(data).map(([filename, value]) =>
          value && typeof value === "object" ? { filename, ...value } : value,
        );
  } else {
    return ["web/vendor/MANIFEST.json: expected JSON array or object"];
  }

  const errors = [];
  const manifestFiles = new Map();
  for (const item of entries) {
    if (!item || typeof item !== "object") {
      errors.push(`web/vendor/MANIFEST.json: invalid entry: ${item}`);
      continue;
    }
    const filename = item.filename;
    if (!filename || typeof filename !== "string") {
      errors.push("web/vendor/MANIFEST.json: entry missing 'filename'");
      continue;
    }
    manifestFiles.set(filename, item);
  }

  for (const [filename, item] of manifestFiles) {
    for (const field of ["version", "license", "sha256"]) {
      if (!item[field]) errors.push(`web/vendor/MANIFEST.json: '${filename}' missing '${field}'`);
    }
    // A field that is present and says nothing is still nothing: the point of
    // the manifest is that someone can find the release it names.
    const version = String(item.version ?? "");
    if (!/^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.]+)?$/.test(version)) {
      errors.push(`web/vendor/MANIFEST.json: '${filename}' version ${JSON.stringify(version)} is not a release number`);
    }
    if (!VENDOR_LICENSES.has(String(item.license ?? ""))) {
      errors.push(
        `web/vendor/MANIFEST.json: '${filename}' license ${JSON.stringify(item.license)} ` +
          "is not an SPDX id this project accepts",
      );
    }
    if (!/^[0-9a-f]{64}$/.test(String(item.sha256 ?? ""))) {
      errors.push(`web/vendor/MANIFEST.json: '${filename}' sha256 is not a digest`);
    }
    if (!(item.upstream_url || item.upstream || item.url)) {
      errors.push(`web/vendor/MANIFEST.json: '${filename}' missing upstream URL`);
    }
  }

  const diskFiles = new Map();
  for (const path of walk(VENDOR)) {
    if (basename(path) === "MANIFEST.json") continue;
    diskFiles.set(toPosix(relative(VENDOR, path)), path);
  }

  for (const name of [...diskFiles.keys()].sort()) {
    if (!manifestFiles.has(name)) errors.push(`web/vendor/${name} is not tracked in MANIFEST.json`);
  }
  for (const [name, item] of [...manifestFiles].sort(([a], [b]) => a.localeCompare(b))) {
    const target = diskFiles.get(name);
    if (!target) {
      errors.push(`web/vendor/${name} listed in MANIFEST.json is missing from disk`);
      continue;
    }
    const actual = createHash("sha256").update(readFileSync(target)).digest("hex");
    const expected = String(item.sha256).toLowerCase();
    if (actual !== expected) {
      errors.push(`web/vendor/${name}: sha256 mismatch (expected ${expected}, got ${actual})`);
    }
  }
  return errors;
}

describe("the vendored bytes match the manifest", () => {
  it("records and hashes every vendored file", () => {
    expect(checkVendorManifest()).toEqual([]);
  });
});
