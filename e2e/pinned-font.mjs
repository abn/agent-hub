// One sans face and one mono face for a check whose numbers or pixels depend
// on how text sets.
//
// The design's stack resolves to whatever the host has: Cantarell on the CI
// runner, Inter or DejaVu on a developer's machine. A screenshot baseline or a
// character count made on one is wrong on the other, so a check that holds
// either pins the stack to Cantarell from the dev-only `@fontsource/cantarell`
// package, the face CI already renders, and the mono stack to DejaVu Sans Mono
// from the dev-only `@fontsource/dejavu-mono` package. Nothing here reaches
// the shipped binary: the fonts are read from `node_modules` and answered by
// the test's own route under the page's origin, because the app's policy
// allows a font only from 'self', so a data URL would be refused.
//
// Each package carries 400 and 700 only, so the 500 and 600 the design uses set
// in the nearer of the two by the browser's own matching, the same way on
// every host.

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const FILES = path.join(ROOT, "node_modules", "@fontsource", "cantarell", "files");
const FAMILY = "Pinned Cantarell";
const WEIGHTS = [400, 700];
const ROUTE = "__pinned-font";
const MONO_FILES = path.join(ROOT, "node_modules", "@fontsource", "dejavu-mono", "files");
const MONO_FAMILY = "Pinned DejaVu Mono";

const face = (weight) =>
  `@font-face { font-family: "${FAMILY}"; font-style: normal; font-weight: ${weight}; src: url("/${ROUTE}/${weight}.woff2") format("woff2"); }`;

const monoFace = (weight) =>
  `@font-face { font-family: "${MONO_FAMILY}"; font-style: normal; font-weight: ${weight}; src: url("/${ROUTE}/mono-${weight}.woff2") format("woff2"); }`;

const CSS = `${WEIGHTS.map(face).join("\n")}\n${WEIGHTS.map(monoFace).join("\n")}\n:root { --font-sans: "${FAMILY}", sans-serif !important; --font-mono: "${MONO_FAMILY}", monospace !important; }`;

// What the spec's `test.use` takes alongside the pin. The shell's service worker
// fetches on the page's behalf, and a page route never sees a request a worker
// makes, so the worker is kept out of a pinned run.
export const pinnedFontOptions = { serviceWorkers: "block" };

// Every document the page opens from now on sets in the pinned face. Call it
// before the first navigation, in a spec that uses `pinnedFontOptions`.
export async function pinFont(page) {
  await page.route(`**/${ROUTE}/*.woff2`, (route) => {
    const name = path.basename(new URL(route.request().url()).pathname, ".woff2");
    const mono = name.startsWith("mono-");
    const weight = Number(mono ? name.slice("mono-".length) : name);
    if (!WEIGHTS.includes(weight)) return route.fulfill({ status: 404 });
    return route.fulfill({
      contentType: "font/woff2",
      body: readFileSync(
        mono
          ? path.join(MONO_FILES, `dejavu-mono-latin-${weight}-normal.woff2`)
          : path.join(FILES, `cantarell-latin-${weight}-normal.woff2`),
      ),
    });
  });
  await page.addInitScript((css) => {
    const style = document.createElement("style");
    style.dataset.pinnedFont = "";
    style.textContent = css;
    const place = () => (document.head || document.documentElement).append(style);
    if (document.documentElement) place();
    else document.addEventListener("DOMContentLoaded", place, { once: true });
  }, CSS);
}

// Resolves once the pinned faces have loaded for what the page has drawn, so a
// measurement or a screenshot never reads the fallback.
export async function fontsSettled(page) {
  await page.evaluate(
    async ({ families, weights }) => {
      for (const family of families) {
        await Promise.all(weights.map((weight) => document.fonts.load(`${weight} 15px "${family}"`)));
      }
      await document.fonts.ready;
      for (const family of families) {
        const loaded = [...document.fonts].filter((font) => font.family.includes(family) && font.status === "loaded");
        if (loaded.length !== weights.length) {
          throw new Error(`the pinned face ${family} did not load (${loaded.length} of ${weights.length}); is the service worker blocked?`);
        }
      }
    },
    { families: [FAMILY, MONO_FAMILY], weights: WEIGHTS },
  );
}

// What a pinned screenshot is compared with. Baselines are made on CI's own
// browser by the visual-baselines workflow, the same build that checks them,
// so the ratio only absorbs nondeterministic sub-pixel noise. It is small
// enough that a band moved or resized by 4px fails it.
export const SCREENSHOT = { maxDiffPixelRatio: 0.002, animations: "disabled" };
