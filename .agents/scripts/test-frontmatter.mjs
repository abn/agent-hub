#!/usr/bin/env node
// Tests for web/frontmatter.mjs, the browser's frontmatter reader and patcher.
//
// Three layers, all seeded and none with a dependency:
//
// 1. The fixture corpus under tests/fixtures/frontmatter, the same manifest
//    and files tests/frontmatter_corpus.rs runs, compared as bytes. A case
//    that cannot be loaded, an unknown operation, a fixture file no case
//    names, a refusal with the wrong code and a shrunken corpus all fail.
//    Nothing is skipped.
// 2. Properties over generated input: the empty patch is the identity,
//    patching one key leaves every other byte alone, patch then read returns
//    what was set, nothing but a typed refusal is ever thrown, and the work
//    is linear. The generator is the xorshift tests/frontmatter_properties.rs
//    uses, so a seed names the same page on both sides.
// 3. With --differential, generated (page, operation) pairs are answered by
//    the Rust reference (tests/frontmatter_reference.rs, run through cargo)
//    and by the module, and the answers must be the same bytes or the same
//    refusal code.
//
// Usage: node .agents/scripts/test-frontmatter.mjs [--differential]
//          [--cases N] [--seed N] [--only corpus|properties|differential]
//          [--module path/to/frontmatter.mjs]

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isDeepStrictEqual } from "node:util";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const FIXTURES = path.join(ROOT, "tests/fixtures/frontmatter");
// The minimum tests/frontmatter_corpus.rs holds the corpus to.
const MIN_CASES = 60;
const BUDGET_MS = 10_000;

function parseArgs(argv) {
  const args = {
    module: path.join(ROOT, "web/frontmatter.mjs"),
    differential: false,
    cases: 6000,
    seed: 1,
    only: null,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const flag = argv[i];
    if (flag === "--differential") args.differential = true;
    else if (flag === "--module") args.module = path.resolve(argv[(i += 1)]);
    else if (flag === "--cases") args.cases = Number(argv[(i += 1)]);
    else if (flag === "--seed") args.seed = Number(argv[(i += 1)]);
    else if (flag === "--only") args.only = argv[(i += 1)];
    else throw new Error(`unknown argument ${flag}`);
  }
  if (!Number.isSafeInteger(args.cases) || args.cases < 1) throw new Error("--cases");
  if (!Number.isSafeInteger(args.seed) || args.seed < 0) throw new Error("--seed");
  if (args.only !== null && !["corpus", "properties", "differential"].includes(args.only)) {
    throw new Error("--only takes corpus, properties or differential");
  }
  if (args.only === "differential") args.differential = true;
  return args;
}

const args = parseArgs(process.argv.slice(2));
const fm = await import(pathToFileURL(args.module).href);
const { FrontmatterError, CODES } = fm;

// ---------------------------------------------------------------------------
// A small harness: every check runs, every failure is listed.

const results = [];

function check(name, body) {
  const started = Date.now();
  try {
    const note = body();
    results.push({ name, ok: true });
    console.log(`ok    ${name}${note ? ` (${note})` : ""} ${Date.now() - started} ms`);
  } catch (err) {
    results.push({ name, ok: false });
    console.log(`FAIL  ${name}\n      ${String(err && err.message ? err.message : err)}`);
  }
}

function fail(message) {
  throw new Error(message);
}

function show(value) {
  const text = JSON.stringify(value);
  return text.length > 400 ? `${text.slice(0, 400)}...` : text;
}

function assertEqual(got, want, context) {
  if (got !== want) fail(`${context}\n         got: ${show(got)}\n      wanted: ${show(want)}`);
}

// Run an operation to `{ ok }` or `{ error }`. Anything thrown that is not a
// typed refusal with a known code is a failure of the module.
function outcome(work, context) {
  try {
    const out = work();
    if (typeof out !== "string") fail(`${context}: returned ${typeof out}, not a string`);
    return { ok: out };
  } catch (err) {
    if (err instanceof FrontmatterError && CODES.includes(err.code)) {
      return { error: err.code };
    }
    fail(`${context}: threw ${err && err.stack ? err.stack : err}`);
  }
}

function refusal(work, code, context) {
  const got = outcome(work, context);
  if (got.error !== code) fail(`${context}: wanted refusal ${code}, got ${show(got)}`);
}

// ---------------------------------------------------------------------------
// The corpus

const OPERATION_FIELDS = {
  patch: ["operation", "changes"],
  review: ["operation", "actor", "time"],
  promote: [
    "operation",
    "type",
    "title",
    "description",
    "tags",
    "session_name",
    "session_id",
    "from_path",
  ],
};

function readFixture(name) {
  return fs.readFileSync(path.join(FIXTURES, name));
}

// Decoded strictly, and with a byte order mark kept: it is one of the cases.
function decode(bytes, context) {
  try {
    return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  } catch {
    fail(`${context}: not UTF-8`);
  }
}

function onlyFields(object, allowed, context) {
  if (typeof object !== "object" || object === null || Array.isArray(object)) {
    fail(`${context}: not an object`);
  }
  for (const field of Object.keys(object)) {
    if (!allowed.includes(field)) fail(`${context}: unknown field ${field}`);
  }
}

function runOperation(text, op, context) {
  onlyFields(op, Object.keys(OPERATION_FIELDS).flatMap((k) => OPERATION_FIELDS[k]), context);
  const allowed = OPERATION_FIELDS[op.operation];
  if (allowed === undefined) fail(`${context}: unknown operation ${show(op.operation)}`);
  onlyFields(op, allowed, context);
  if (op.operation === "patch") return fm.patch(text, op.changes);
  if (op.operation === "review") return fm.review(text, op.actor, op.time);
  return fm.promote(text, op);
}

function loadManifest() {
  const manifest = JSON.parse(decode(readFixture("manifest.json"), "manifest.json"));
  onlyFields(manifest, ["version", "cases"], "manifest.json");
  if (manifest.version !== 2) fail(`unknown manifest version ${manifest.version}`);
  if (!Array.isArray(manifest.cases)) fail("manifest.json: cases is not a list");
  return manifest;
}

function corpusChecks() {
  const manifest = loadManifest();

  check("corpus: every case matches byte for byte", () => {
    if (manifest.cases.length < MIN_CASES) {
      fail(`the corpus shrank to ${manifest.cases.length} cases`);
    }
    const failures = [];
    for (const meta of manifest.cases) {
      try {
        onlyFields(meta, ["id", "description", "input", "patch", "expected", "error"], "case");
        for (const field of ["id", "description", "input", "patch"]) {
          if (typeof meta[field] !== "string" || meta[field] === "") {
            fail(`${meta.id}: no ${field}`);
          }
        }
        if ((meta.expected === undefined) === (meta.error === undefined)) {
          fail(`${meta.id}: exactly one of expected and error is required`);
        }
        const text = decode(readFixture(meta.input), meta.input);
        const op = JSON.parse(decode(readFixture(meta.patch), meta.patch));
        const got = outcome(() => runOperation(text, op, meta.id), meta.id);
        if (meta.error !== undefined) {
          if (got.error !== meta.error) {
            fail(`${meta.id}: wanted refusal ${meta.error}, got ${show(got)}`);
          }
        } else {
          if (got.ok === undefined) fail(`${meta.id}: refused with ${got.error}`);
          const want = readFixture(meta.expected);
          if (!Buffer.from(got.ok, "utf8").equals(want)) {
            fail(
              `${meta.id} (${meta.description})\n         got: ${show(got.ok)}\n      wanted: ${show(want.toString("utf8"))}`,
            );
          }
        }
      } catch (err) {
        failures.push(err.message);
      }
    }
    if (failures.length > 0) {
      fail(`${failures.length} of ${manifest.cases.length} cases failed:\n      ${failures.join("\n      ")}`);
    }
    return `${manifest.cases.length} cases`;
  });

  check("corpus: every fixture file belongs to a case", () => {
    const referenced = new Set(["manifest.json", "README.md"]);
    const ids = new Set();
    for (const meta of manifest.cases) {
      if (ids.has(meta.id)) fail(`duplicate case id ${meta.id}`);
      ids.add(meta.id);
      for (const name of [meta.input, meta.patch, meta.expected]) {
        if (name !== undefined) referenced.add(name);
      }
    }
    const onDisk = new Set(fs.readdirSync(FIXTURES));
    const stray = [...onDisk].filter((name) => !referenced.has(name));
    const missing = [...referenced].filter((name) => !onDisk.has(name));
    if (stray.length > 0 || missing.length > 0) {
      fail(`fixture files and manifest disagree: stray ${show(stray)}, missing ${show(missing)}`);
    }
    return `${onDisk.size} files`;
  });

  check("corpus: every refusal code and operation is covered", () => {
    const refused = new Set(manifest.cases.map((meta) => meta.error));
    for (const code of CODES) {
      if (!refused.has(code)) fail(`no case refuses with ${code}`);
    }
    const operations = new Set(
      manifest.cases.map(
        (meta) => JSON.parse(decode(readFixture(meta.patch), meta.patch)).operation,
      ),
    );
    for (const operation of Object.keys(OPERATION_FIELDS)) {
      if (!operations.has(operation)) fail(`no case runs ${operation}`);
    }
  });

  return manifest;
}

// ---------------------------------------------------------------------------
// The generator shared with tests/frontmatter_properties.rs

const MASK = (1n << 64n) - 1n;

class Rng {
  constructor(seed) {
    this.state = ((BigInt(seed) * 0x9e3779b97f4a7c15n) & MASK) | 1n;
  }

  next() {
    let s = this.state;
    s ^= (s << 13n) & MASK;
    s ^= s >> 7n;
    s ^= (s << 17n) & MASK;
    this.state = s;
    return s;
  }

  below(n) {
    return Number(this.next() % BigInt(n));
  }

  pick(items) {
    return items[this.below(items.length)];
  }
}

const cp = (code) => String.fromCodePoint(code);

// Fragments that matter to a line-oriented YAML patcher.
const ALPHABET = [
  "-", "---", "...", ":", ": ", "#", " #", '"', "'", "\\", "\r", "\n", "\r\n", "\t", " ",
  "  ", "a", "key", "verified", "sources", "[", "]", "{", "}", ",", "&", "*", "!", "|", ">",
  "0", "true", "\0",
  ...[0xe9, 0x65e5, 0x1d11e, 0x2028, 0x85, 0xfeff].map(cp),
];

function noise(rng, alphabet, max) {
  const len = rng.below(max + 1);
  let out = "";
  for (let i = 0; i < len; i += 1) out += rng.pick(alphabet);
  return out;
}

// Arbitrary UTF-16 code units, lone surrogates included: input only a
// JavaScript caller can produce.
function codeUnits(rng, max) {
  const len = rng.below(max + 1);
  let out = "";
  for (let i = 0; i < len; i += 1) {
    const kind = rng.below(4);
    if (kind === 0) out += String.fromCharCode(0xd800 + rng.below(0x800));
    else if (kind === 1) out += String.fromCharCode(rng.below(0x10000));
    else out += rng.pick(ALPHABET);
  }
  return out;
}

function hasLoneSurrogate(s) {
  for (let i = 0; i < s.length; i += 1) {
    const code = s.charCodeAt(i);
    if (code < 0xd800 || code > 0xdfff) continue;
    const next = s.charCodeAt(i + 1);
    if (code > 0xdbff || !(next >= 0xdc00 && next <= 0xdfff)) return true;
    i += 1;
  }
  return false;
}

function generatedPage(rng) {
  const nl = rng.below(3) === 0 ? "\r\n" : "\n";
  const count = 2 + rng.below(6);
  const segments = [{ key: null, bytes: `---${nl}` }];
  for (let position = 0; position < count; position += 1) {
    if (rng.below(3) === 0) {
      const filler = rng.pick(["# a comment  ", "", "#", "   "]);
      segments.push({ key: null, bytes: `${filler}${nl}` });
    }
    const key = `key${position}`;
    const bytes = [
      `${key}: plain value  ${nl}`,
      `${key}: "quoted: # value" # trailing${nl}`,
      `${key}: |${nl}  para one${nl}${nl}  para two${nl}`,
      `${key}: >-${nl}  folded${nl}${nl}${nl}  more${nl}`,
      `${key}:${nl}  - one${nl}# inner comment${nl}  - two${nl}`,
      `${key}:${nl}- by: a${nl}  at: b${nl}${nl}- by: c${nl}`,
      `${key}:${nl}  nested:${nl}    deep: [1, 2]${nl}\t  tabbed: x${nl}`,
      `${key}: [a, "b"]${nl}`,
    ][rng.below(8)];
    segments.push({ key, bytes });
  }
  const body = rng.pick(["", "Body", "\n---\n\nkey0: not frontmatter\n", "\r\nBody\r\n"]);
  segments.push({ key: null, bytes: `---${nl}${body}` });
  return { segments, nl };
}

const join = (segments) => segments.map((segment) => segment.bytes).join("");

function count(haystack, needle) {
  return haystack.split(needle).length - 1;
}

// ---------------------------------------------------------------------------
// Properties

const AT = "2026-09-19T17:00:00Z";

function promoteParams(title, tags) {
  return {
    type: "concept",
    title,
    description: title,
    tags,
    session_name: title,
    session_id: "sess_1",
    from_path: "/fs/n.md",
  };
}

function propertyChecks(manifest) {
  check("property: the empty patch is the identity on every corpus input", () => {
    for (const meta of manifest.cases) {
      const text = decode(readFixture(meta.input), meta.input);
      assertEqual(fm.patch(text, []), text, meta.id);
    }
    return `${manifest.cases.length} inputs`;
  });

  check("property: the empty patch is the identity on arbitrary input", () => {
    for (let seed = 0; seed < 3000; seed += 1) {
      const rng = new Rng(seed);
      const text = noise(rng, ALPHABET, 40);
      assertEqual(fm.patch(text, []), text, `seed ${seed}`);
      const units = codeUnits(rng, 40);
      assertEqual(fm.patch(units, []), units, `seed ${seed}, code units`);
    }
    return "3000 seeds, twice";
  });

  check("property: nothing but a refusal is thrown, and output never reads worse", () => {
    let accepted = 0;
    for (let seed = 0; seed < 6000; seed += 1) {
      const rng = new Rng(seed);
      // Bias half the inputs towards pages that do open a block.
      let text = rng.below(2) === 0 ? "---\n" : "";
      text += noise(rng, ALPHABET, 40);
      if (rng.below(2) === 0) text += `\n---\n${noise(rng, ALPHABET, 8)}`;
      const value = noise(rng, ALPHABET, 6);
      const tags = [value, noise(rng, ALPHABET, 3)];
      const context = `seed ${seed}: ${show(text)}`;

      outcome(() => JSON.stringify(fm.read(text)), context);
      const outputs = [
        outcome(
          () => fm.patch(text, [["key", value], ["verified", null], ["tags", tags]]),
          context,
        ),
        outcome(() => fm.review(text, value, AT), context),
        outcome(() => fm.promote(text, promoteParams(value, tags)), context),
      ];
      // Whatever was accepted must still lay out.
      for (const output of outputs) {
        if (output.ok === undefined) continue;
        accepted += 1;
        const again = outcome(() => JSON.stringify(fm.read(output.ok)), context);
        if (again.error !== undefined) {
          fail(`${context} became unreadable (${again.error}): ${show(output.ok)}`);
        }
      }
    }
    if (accepted < 1000) fail(`only ${accepted} operations were accepted: the generator rotted`);
    return `6000 seeds, ${accepted} accepted`;
  });

  check("property: arbitrary code units never throw and never reach the page", () => {
    for (let seed = 0; seed < 3000; seed += 1) {
      const rng = new Rng(seed);
      let text = rng.below(2) === 0 ? "---\n" : "";
      text += codeUnits(rng, 30);
      if (rng.below(2) === 0) text += `\n---\n${codeUnits(rng, 6)}`;
      const value = codeUnits(rng, 5);
      const context = `seed ${seed}: ${show(text)} with ${show(value)}`;

      outcome(() => JSON.stringify(fm.read(text)), context);
      const patched = outcome(() => fm.patch(text, [["key", value], ["tags", [value]]]), context);
      const reviewed = outcome(() => fm.review(text, value, AT), context);
      const promoted = outcome(() => fm.promote(text, promoteParams(value, [value])), context);
      if (hasLoneSurrogate(value)) {
        for (const got of [patched, reviewed, promoted]) {
          if (got.error !== "invalid_value") {
            fail(`${context}: a lone surrogate was not refused: ${show(got)}`);
          }
        }
      }
    }
    return "3000 seeds";
  });

  check("property: patching one key leaves every other byte alone", () => {
    for (let seed = 0; seed < 4000; seed += 1) {
      const rng = new Rng(seed);
      const { segments, nl } = generatedPage(rng);
      const page = join(segments);
      const keyed = segments.map((s, i) => (s.key === null ? -1 : i)).filter((i) => i >= 0);
      const target = rng.pick(keyed);
      const key = segments[target].key;
      const before = join(segments.slice(0, target));
      const after = join(segments.slice(target + 1));

      const value = noise(rng, ALPHABET, 5);
      const replaced = fm.patch(page, [[key, value]]);
      if (
        !replaced.startsWith(before) ||
        !replaced.endsWith(after) ||
        replaced.length < before.length + after.length
      ) {
        fail(`seed ${seed}: bytes outside ${key} changed: ${show(replaced)}`);
      }
      const middle = replaced.slice(before.length, replaced.length - after.length);
      if (!middle.startsWith(`${key}: `) || !middle.endsWith(nl) || count(middle, "\n") !== 1) {
        fail(`seed ${seed}: the value left its line: ${show(middle)}`);
      }
      assertEqual(fm.patch(page, [[key, null]]), before + after, `seed ${seed}: delete`);
    }
    return "4000 seeds";
  });

  check("property: review leaves every byte outside verified alone", () => {
    for (let seed = 0; seed < 2000; seed += 1) {
      const rng = new Rng(seed);
      const { segments, nl } = generatedPage(rng);
      const page = join(segments);
      const last = segments[segments.length - 1].bytes;
      const head = join(segments.slice(0, -1));
      const added = `verified:${nl}  - by: human${nl}    at: ${AT}${nl}`;
      const reviewed = fm.review(page, "human", AT);
      assertEqual(reviewed, head + added + last, `seed ${seed}`);
      // A second review appends under the first and touches nothing else.
      const second = `  - by: human${nl}    at: 2026-09-20T08:00:00Z${nl}`;
      assertEqual(
        fm.review(reviewed, "human", "2026-09-20T08:00:00Z"),
        head + added + second + last,
        `seed ${seed}: second review`,
      );
    }
    return "2000 seeds";
  });

  check("property: patch then read returns what was set", () => {
    for (let seed = 0; seed < 4000; seed += 1) {
      const rng = new Rng(seed);
      const title = noise(rng, ALPHABET, 6);
      const custom = noise(rng, ALPHABET, 6);
      const tags = [];
      for (let i = rng.below(4); i > 0; i -= 1) tags.push(noise(rng, ALPHABET, 4));
      const by = noise(rng, ALPHABET, 4);
      const changes = [
        ["title", title],
        ["custom", custom],
        ["tags", tags],
        ["verified", [{ by, at: AT }]],
      ];
      const page = rng.pick([
        "",
        "# Body\n",
        "---\ntitle: old\ntags:\n  - x\n\n  - y\n---\n",
        "---\r\ncustom: |\r\n  a\r\n\r\n  b\r\n---\r\n",
      ]);
      const out = fm.patch(page, changes);
      const got = fm.read(out);
      const context = `seed ${seed}: ${show(out)}`;
      assertEqual(got.title, title, context);
      if (!isDeepStrictEqual(got.custom, [["custom", custom]])) fail(`${context}: custom`);
      if (!isDeepStrictEqual(got.tags, tags)) fail(`${context}: tags`);
      if (!isDeepStrictEqual(got.verified, [{ by, at: AT }])) fail(`${context}: verified`);
      if (page !== "" && !page.startsWith("---")) assertEqual(got.body, `\n${page}`, context);
    }
    return "4000 seeds";
  });

  check("property: a five megabyte page is handled in linear time", () => {
    let page = "---\ntitle: old\ndescription: |\n";
    const scalar = "  a line of a very long literal scalar\n\n";
    page += scalar.repeat(Math.ceil((2_500_000 - page.length) / scalar.length));
    page += "status: draft\n---\n";
    const body = "A body line with --- and key: value text.\n";
    page += body.repeat(Math.ceil((5_000_000 - page.length) / body.length));

    const timings = [];
    const timed = (name, work) => {
      const started = Date.now();
      const out = work();
      const elapsed = Date.now() - started;
      if (elapsed > BUDGET_MS) fail(`${name} took ${elapsed} ms`);
      timings.push(`${name} ${elapsed} ms`);
      return out;
    };
    const out = timed("patch", () =>
      fm.patch(page, [["description", "short"], ["status", "stable"]]),
    );
    // The blank line that ends the scalar is followed by no indented line,
    // so it is not part of the value and stays.
    const head = "---\ntitle: old\ndescription: short\n\nstatus: stable\n---\n";
    assertEqual(out.slice(0, head.length), head, "the head of the patched page");
    timed("review", () => fm.review(page, "human", AT));
    const got = timed("read", () => fm.read(page));
    assertEqual(got.status, "draft", "read");

    const line = `---\nlong: ${"x: y #".repeat(800_000)}\n---\n`;
    const patched = timed("one long line", () => fm.patch(line, [["long", "short"], ["k", line]]));
    assertEqual(fm.read(patched).custom[1][1], line, "a page as a value reads back");
    return timings.join(", ");
  });

  check("property: a hundred thousand keys are patched in linear time", () => {
    const lines = ["---\n"];
    for (let n = 0; n < 100_000; n += 1) lines.push(`key${n}: value ${n}\n`);
    lines.push("---\nBody\n");
    const page = lines.join("");
    // Replace every fifth key, delete every seventh, add as many again.
    const changes = [];
    for (let n = 0; n < 100_000; n += 5) changes.push([`key${n}`, "patched"]);
    for (let n = 1; n < 100_000; n += 7) if (n % 5 !== 0) changes.push([`key${n}`, null]);
    for (let n = 0; n < 20_000; n += 1) changes.push([`added${n}`, n]);
    const started = Date.now();
    const out = fm.patch(page, changes);
    const elapsed = Date.now() - started;
    if (elapsed > BUDGET_MS) fail(`took ${elapsed} ms`);
    if (!out.includes("\nkey99995: patched\n")) fail("key99995 was not patched");
    if (out.includes("\nkey8:")) fail("key8 was not deleted");
    if (!out.endsWith("added19999: 19999\n---\nBody\n")) fail("the added keys are not last");
    assertEqual(fm.read(out).fields.length, 100_000 - 11_429 + 20_000, "keys read back");
    return `${elapsed} ms`;
  });

  check("hostile values stay inside their line and read back", () => {
    const values = [
      "</script><script>alert(1)</script>",
      "---",
      "...",
      "a: b",
      "# not a comment",
      "\tTabbed\t",
      "nul\0nul",
      "x".repeat(1_000_000),
      "line\nkey: injected\n---\nbody",
      `${cp(0x1d11e)} ${cp(0x10ffff)} ${cp(0xe9)}`,
      "\\",
      '"',
    ];
    for (const value of values) {
      for (const page of ["", "# Body\n", "---\ncustom: old\n---\nBody\n"]) {
        const out = fm.patch(page, [["custom", value], ["tags", [value]]]);
        const got = fm.read(out);
        const context = show(value.slice(0, 60));
        if (!isDeepStrictEqual(got.custom, [["custom", value]])) fail(`${context}: custom`);
        if (!isDeepStrictEqual(got.tags, [value])) fail(`${context}: tags`);
        // A replaced key, a new key, and for a new block its two delimiters
        // and the empty line before a body: never a line more.
        const added = page.startsWith("---") ? 1 : page === "" ? 4 : 5;
        assertEqual(count(out, "\n"), count(page, "\n") + added, context);
      }
    }
    return `${values.length} values`;
  });

  check("astral characters and lone surrogates", () => {
    const clef = cp(0x1d11e);
    const lone = String.fromCharCode(0xd834);
    const low = String.fromCharCode(0xdd1e);

    const out = fm.patch("---\n---\n", [["title", clef]]);
    const want = Buffer.concat([
      Buffer.from("---\ntitle: "),
      Buffer.from([0xf0, 0x9d, 0x84, 0x9e]),
      Buffer.from("\n---\n"),
    ]);
    if (!Buffer.from(out, "utf8").equals(want)) fail(`an astral value: ${show(out)}`);
    assertEqual(fm.read(out).title, clef, "an astral value reads back");

    // In the page a lone surrogate is data: left where it is, never thrown on.
    const page = `---\nnote: ${lone}\ntitle: old${low}\n---\nBody ${low}${lone}\n`;
    assertEqual(
      fm.patch(page, [["title", "new"]]),
      `---\nnote: ${lone}\ntitle: new\n---\nBody ${low}${lone}\n`,
      "a lone surrogate in the page",
    );
    assertEqual(fm.read(page).custom[0][1], lone, "a lone surrogate is read as it is");
    assertEqual(fm.patch(lone, []), lone, "identity");
    assertEqual(fm.patch(lone, [["a", "b"]]), `---\na: b\n---\n\n${lone}`, "no block");

    // In a value it cannot be sent to the server, so it is refused.
    for (const bad of [lone, low, `${low}${lone}`, `a${lone}b`]) {
      refusal(() => fm.patch(page, [["title", bad]]), "invalid_value", "string");
      refusal(() => fm.patch(page, [["tags", [bad]]]), "invalid_value", "list");
      refusal(() => fm.patch(page, [["v", [{ by: bad }]]]), "invalid_value", "record");
      refusal(() => fm.patch(page, [["v", [{ [bad]: "x" }]]]), "invalid_value", "field");
      refusal(() => fm.patch(page, [[bad, "x"]]), "invalid_key", "key");
      refusal(() => fm.review(page, bad, AT), "invalid_value", "review");
      refusal(() => fm.promote(page, promoteParams(bad, [])), "invalid_value", "promote");
    }

    // Escapes: an astral one is read, a surrogate one is not a character.
    const escaped = '---\na: "\\U0001D11E"\nb: "\\uD834"\nc: "\\UFFFFFFFF"\nd: "\\' + clef + '"\n---\n';
    assertEqual(
      show(fm.read(escaped).custom),
      show([["a", clef], ["b", '"\\uD834"'], ["c", '"\\UFFFFFFFF"'], ["d", clef]]),
      "escapes",
    );
  });

  check("the change list: shapes, order of checks, numbers", () => {
    const page = "---\ntype: concept\n---\n";
    const bom = `${cp(0xfeff)}${page}`;
    for (const changes of [null, undefined, {}, "x", [["a"]], [["a", "b", "c"]], [[1, "x"]], ["ab"]]) {
      try {
        fm.patch(page, changes);
      } catch (err) {
        if (err instanceof TypeError) continue;
      }
      fail(`${show(changes)} is not a change list and was not a TypeError`);
    }
    for (const text of [null, undefined, 1, {}, []]) {
      for (const work of [() => fm.read(text), () => fm.patch(text, []), () => fm.review(text, "a", AT)]) {
        try {
          work();
        } catch (err) {
          if (err instanceof TypeError) continue;
        }
        fail(`${show(text)} is not a page and was not a TypeError`);
      }
    }

    // `undefined` is never a deletion: JSON would turn it into one.
    refusal(() => fm.patch(page, [["type", undefined]]), "invalid_value", "undefined");
    for (const value of [1.5, NaN, Infinity, 2 ** 53, -(2 ** 53), 1e21, 2 ** 63, 10n, () => 1, Symbol("s")]) {
      refusal(() => fm.patch(page, [["k", value]]), "invalid_value", String(value));
    }
    for (const value of [{ a: "b" }, ["a", 1], [["a"]], [null], ["a", { b: "c" }], [{}], [{ a: null }], [{ a: [] }], [{ a: {} }], [{ 1: "a" }], [{ "bad field": "a" }], [true], new Date(0), new Map()]) {
      refusal(() => fm.patch(page, [["k", value]]), "invalid_value", show(value));
    }

    assertEqual(fm.patch(page, [["k", -0]]), "---\ntype: concept\nk: 0\n---\n", "-0 is 0");
    assertEqual(
      fm.patch(page, [["a", Number.MAX_SAFE_INTEGER], ["b", -Number.MAX_SAFE_INTEGER], ["c", 1e15]]),
      "---\ntype: concept\na: 9007199254740991\nb: -9007199254740991\nc: 1000000000000000\n---\n",
      "integers print as digits",
    );
    assertEqual(fm.patch(page, [["k", []]]), "---\ntype: concept\nk: []\n---\n", "empty list");

    // Shapes first, then each change in order, then the page.
    refusal(() => fm.patch(bom, [["bad key", "x"], ["k", 1.5]]), "invalid_value", "shape first");
    refusal(() => fm.patch(bom, [["bad key", "x"], ["k", 2 ** 53]]), "invalid_key", "key, then range");
    refusal(() => fm.patch(bom, [["k", 2 ** 53], ["bad key", "x"]]), "invalid_value", "in order");
    refusal(() => fm.patch(bom, [["k", "x"], ["k", 2 ** 53]]), "duplicate_change", "duplicate first");
    refusal(() => fm.patch(bom, [["bad key", "x"], ["k", [{ 1: "a" }]]]), "invalid_key", "fields later");
    refusal(() => fm.patch(bom, [["k", "x"]]), "byte_order_mark", "then the page");
    assertEqual(fm.patch(bom, []), bom, "an empty patch is never refused");

    // Keys that are also Object.prototype members are ordinary keys.
    const proto = JSON.parse('[["__proto__", "a"], ["constructor", [{"__proto__": "b", "toString": "c"}]]]');
    assertEqual(
      fm.patch("---\nconstructor: x\n__proto__: y\n---\n", proto),
      "---\nconstructor:\n  - __proto__: b\n    toString: c\n__proto__: a\n---\n",
      "prototype member names",
    );
    const got = fm.read("---\n__proto__: y\nconstructor: x\nhasOwnProperty: z\n---\n");
    assertEqual(show(got.custom), show([["__proto__", "y"], ["constructor", "x"], ["hasOwnProperty", "z"]]), "read");
  });

  check("read: fields, body and raw", () => {
    const none = fm.read("# Title\n");
    assertEqual(none.raw, null, "no block: raw");
    assertEqual(none.body, "# Title\n", "no block: body");
    assertEqual(none.fields.length, 0, "no block: fields");

    const page =
      "---\r\ntype: concept\r\n# note\r\nzeta: [a, b] # why\r\ntags: [\"x, y\", 'it''s', z]\r\n" +
      "verified:\r\n  - by: human\r\n    at: 2026-01-01T00:00:00Z\r\n  - by: only\r\n" +
      "sources:\r\n- title: \"A: b\" # c\r\n  resource: r\r\nstale_after: 2027-01-01\r\n" +
      "description: >-\r\n  folded\r\n\r\n  text\r\ntitle: one\r\ntitle: two\r\n---\r\nBody\r\n---\r\n";
    const got = fm.read(page);
    assertEqual(got.body, "Body\r\n---\r\n", "body");
    assertEqual(got.raw, page.slice(5, page.indexOf("---\r\nBody")), "raw");
    assertEqual(got.type, "concept", "type");
    assertEqual(got.title, "two", "the last of a repeated key wins");
    assertEqual(got.description, "folded text", "description");
    assertEqual(got.stale_after, "2027-01-01", "stale_after");
    assertEqual(show(got.tags), show(["x, y", "it's", "z"]), "tags");
    assertEqual(show(got.verified), show([{ by: "human", at: "2026-01-01T00:00:00Z" }]), "verified");
    assertEqual(show(got.sources), show([{ title: "A: b", resource: "r" }]), "sources");
    assertEqual(show(got.custom), show([["zeta", "[a, b] # why"]]), "custom");
    assertEqual(
      show(got.fields.map((field) => field.key)),
      show(["type", "zeta", "tags", "verified", "sources", "stale_after", "description", "title", "title"]),
      "fields keep block order",
    );
    assertEqual(got.fields.map((field) => field.text).join(""), got.raw.replace("# note\r\n", ""), "field text");
    refusal(() => fm.read("---\ntype: x\n"), "unclosed", "read refuses what the layout refuses");
  });
}

// ---------------------------------------------------------------------------
// Differential testing against the Rust reference

const NBSP = cp(0xa0);
const WIDE_ALPHABET = [
  ...ALPHABET,
  NBSP, cp(0x3000), cp(0x2029), cp(0x7f), cp(0x9f), cp(0x10ffff), cp(0xfffd), cp(0xfffe), cp(0xffff), cp(0x130),
  "\u000b", "\u000c", "</script>", "~", "%", "@", "`", "?", "<", "=", "+", ".", "9", "2026-",
  "T", "Z", "null", "No", "x", " # ", ":\t", "- ", "-\t", "[a, b]", '"q"', "'s'", "\\n", "\\u00e9",
  "\\U0001D11E", "\\uD834", "\\UFFFFFFFF", "\\x41", "|", ">-", "|2+", "&anchor", "*alias", "title", "tags", "type",
];

const KEYS = [
  "title", "type", "description", "status", "tags", "verified", "sources", "stale_after",
  "okf_version", "key", "a", "custom", "true", "x-y", "_u", "K9", "__proto__", "constructor",
];
const BAD_KEYS = ["", "bad key", "1abc", "k:", "-k", cp(0xe9), "a.b", "a b", " a", "a\n", cp(0x1d11e)];
const FIELDS = ["by", "at", "title", "resource", "pinned", "x-y", "__proto__", "1", "007", "bad field", "", "-1"];

function dateLike(rng) {
  const two = (n) => String(n).padStart(2, "0");
  const year = rng.pick(["2024", "2026", "1900", "2000", "0000", "9999", "2023"]);
  const month = two(rng.pick([0, 1, 2, 2, 2, 4, 9, 11, 12, 13, 99]));
  const day = two(rng.pick([0, 1, 28, 29, 30, 31, 32, 99]));
  let out = `${year}-${month}-${day}`;
  if (rng.below(2) === 0) return out + rng.pick(["", "", "x", " ", "T"]);
  out += `${rng.pick(["T", "T", "t", " "])}${two(rng.pick([0, 12, 23, 24, 99]))}:${two(rng.pick([0, 59, 60]))}:${two(rng.pick([0, 59, 60, 61]))}`;
  out += rng.pick(["", "", ".5", ".250", ".", ".x"]);
  out += rng.pick(["Z", "Z", "z", "", "+02:00", "-05:30", "+23:59", "+24:00", "-00:60", "+0200", "Z ", "+02:00x"]);
  return out;
}

function stringValue(rng) {
  const kind = rng.below(10);
  if (kind < 5) return noise(rng, WIDE_ALPHABET, 6);
  if (kind < 7) return dateLike(rng);
  return rng.pick([
    "", " ", " x", "x ", `${NBSP}x`, `x${NBSP}`, `${cp(0x3000)}x`, "true", "True", "FALSE", "nUlL",
    "yes", "No", "ON", "off", "Y", "n", "yes!", "only", `${cp(0x130)}`, `o${cp(0x17f)}`, "123", "1e3", "0x10", "12:30", "-x",
    "- x", "a: b", "a:b", "a #b", "a#b", "x:", ":x", "#x", "~", "<<", "=", "+1", ".5", "...", "---",
    "C:\\dir", 'say "hi"', "it's", "a\tb", "a\rb", "a\r\nb", "nul\0", "</script>", "{a}", "[a]",
    "&a", "*a", "!t", "|", ">", "%x", "@x", "`x", "?x", ",x", "plain value", "agenthub://s/1",
  ]);
}

const NUMBERS = [
  "0", "1", "-1", "42", "1000000000000000", "9007199254740991", "-9007199254740991",
  "9007199254740992", "-9007199254740992", "9007199254740994", "123456789012345680000",
  "9223372036854775808", "-9223372036854776000", "18446744073709552000", "1e+21", "1e+300",
  "1.5", "-0.5", "0.1", "1e-7", "5e-324",
];

// Every value is built as JSON text, which is what both sides are handed:
// the reference reads the text, the module reads what JSON.parse makes of it.
function scalarJson(rng, allowNull) {
  const kind = rng.below(12);
  if (kind < 8) return JSON.stringify(stringValue(rng));
  if (kind < 9) return rng.pick(["true", "false"]);
  if (kind < 11 || !allowNull) {
    return rng.below(3) === 0 ? rng.pick(NUMBERS) : rng.pick(NUMBERS.slice(0, 7));
  }
  return "null";
}

function recordJson(rng) {
  const used = new Set();
  const fields = [];
  for (let i = rng.below(4); i > 0; i -= 1) {
    const field = rng.below(6) === 0 ? rng.pick(FIELDS) : rng.pick(FIELDS.slice(0, 7));
    if (used.has(field)) continue;
    used.add(field);
    fields.push(`${JSON.stringify(field)}:${scalarJson(rng, rng.below(8) === 0)}`);
  }
  return `{${fields.join(",")}}`;
}

function valueJson(rng) {
  const kind = rng.below(20);
  if (kind < 3) return "null";
  if (kind < 11) return scalarJson(rng, false);
  if (kind < 14) {
    const items = [];
    for (let i = rng.below(4); i > 0; i -= 1) items.push(JSON.stringify(stringValue(rng)));
    return `[${items.join(",")}]`;
  }
  if (kind < 19) {
    const records = [];
    for (let i = 1 + rng.below(3); i > 0; i -= 1) records.push(recordJson(rng));
    return `[${records.join(",")}]`;
  }
  return rng.pick([
    '{"a":"b"}', "{}", '["a",1]', '[["a"]]', "[null]", '["a",{"by":"x"}]', '[{"by":"x"},"a"]',
    "[{}]", '[{},"a"]', "[true]", "[1.5]", "[[]]", '[{"by":"x"},{}]', '[{"by":null}]',
  ]);
}

function changesJson(rng) {
  const changes = [];
  const named = [];
  for (let i = rng.below(5); i > 0; i -= 1) {
    let key = rng.pick(KEYS);
    const roll = rng.below(40);
    if (roll === 0) key = rng.pick(BAD_KEYS);
    else if (roll === 1 && named.length > 0) key = rng.pick(named);
    else if (roll < 8) key = `key${rng.below(8)}`;
    named.push(key);
    changes.push(`[${JSON.stringify(key)},${valueJson(rng)}]`);
  }
  return `[${changes.join(",")}]`;
}

function entryText(rng, key, nl) {
  const word = () => noise(rng, WIDE_ALPHABET.filter((f) => !f.includes("\n") && !f.includes("\r")), 3);
  return rng.pick([
    () => `${key}: plain ${word()}${nl}`,
    () => `${key}: "quoted: # ${word()}" # trailing${nl}`,
    () => `${key}: '${word()}'${nl}`,
    () => `${key}: |${nl}  para one${nl}${nl}  para two${nl}`,
    () => `${key}: >-${nl}  folded${nl}${nl}${nl}  more${nl}`,
    () => `${key}:${nl}  - one${nl}# inner comment${nl}  - "two" # c${nl}`,
    () => `${key}:${nl}- by: a${nl}  at: b${nl}${nl}- by: c${nl}  at: 2026-01-01${nl}`,
    () => `${key}:${nl}    - title: t${nl}      resource: r${nl}`,
    () => `${key}:${nl}  - by: a${nl}    at: b${nl}   odd: c${nl}`,
    () => `${key}:${nl}  - by: a${nl} - by: b${nl}`,
    () => `${key}:${nl}\t- by: a${nl}\t  at: b${nl}`,
    () => `${key}:${nl}  nested:${nl}    deep: [1, 2]${nl}`,
    () => `${key}: [a, "b, c", 'd'] # tail${nl}`,
    () => `${key}: [a, ${word()}${nl}`,
    () => `${key}: &anchor value${nl}`,
    () => `${key}: *alias${nl}`,
    () => `${key}:${nl}`,
    () => `${key}: # only a comment${nl}  - by: a${nl}`,
    () => `${key} : spaced${nl}`,
    () => `${key}:\t${word()}${nl}`,
    () => `${key}:${word()}${nl}`,
    () => `${key}: ${word()}${nl}  continued ${word()}${nl}`,
    () => `${key}: -${nl}- item${nl}`,
  ])();
}

function structuredPage(rng) {
  const nl = rng.below(3) === 0 ? "\r\n" : "\n";
  const other = nl === "\n" ? "\r\n" : "\n";
  // Now and then an opener that is not exactly three dashes.
  const opener = rng.below(10) === 0
    ? rng.pick(["--- # comment", "---yaml", "--- text", "---\t#", "----", "-----  ", "---- x", `---${NBSP}`, "---x\ry", "---:", "--- ---"])
    : "---";
  let page = `${opener}${nl}`;
  for (let i = rng.below(7); i > 0; i -= 1) {
    if (rng.below(3) === 0) {
      const filler = rng.pick(["# a comment  ", "", "#", "   ", "\t", cp(0x85), NBSP, cp(0xfeff), cp(0x2028), " # indented comment"]);
      page += filler + nl;
    }
    const key = rng.below(4) === 0 ? `key${rng.below(8)}` : rng.pick(KEYS);
    page += entryText(rng, key, rng.below(12) === 0 ? other : nl);
  }
  page += rng.below(5) === 0 ? rng.pick(["--- ", "...", "---\t", "----", ""]) : "---";
  page += rng.pick([nl, nl, nl, ""]);
  page += rng.pick(["", "Body", "\n---\n\nkey0: not frontmatter\n", "\r\nBody\r\n", `# Title${nl}${nl}---${nl}`]);
  return page;
}

function mutate(rng, text) {
  let out = text;
  for (let i = rng.below(3); i > 0; i -= 1) {
    // Never split a surrogate pair: the page has to survive as UTF-8.
    let at = rng.below(out.length + 1);
    const code = out.charCodeAt(at);
    if (code >= 0xdc00 && code <= 0xdfff) at += 1;
    const cut = rng.below(3) === 0 ? Math.min(out.length, at + rng.below(4)) : at;
    const end = out.charCodeAt(cut) >= 0xdc00 && out.charCodeAt(cut) <= 0xdfff ? cut + 1 : cut;
    out = out.slice(0, at) + rng.pick(WIDE_ALPHABET) + out.slice(Math.max(at, end));
  }
  return out;
}

function pageText(rng, corpusInputs) {
  const kind = rng.below(16);
  if (kind === 0) return noise(rng, WIDE_ALPHABET, 40);
  if (kind === 1) return `---\n${noise(rng, WIDE_ALPHABET, 30)}\n---\n${noise(rng, WIDE_ALPHABET, 8)}`;
  if (kind === 2) return mutate(rng, rng.pick(corpusInputs));
  if (kind === 3) return rng.pick(["", "\n", "---", "---\n", "---\n---", "---\n---\n", "---\r\n---\r\n", "\r\n", "# Title", "---\n\n---\n", "--- #", "---yaml", "----", "----\r", "--- \r\n---\n"]);
  const page = structuredPage(rng);
  return rng.below(3) === 0 ? mutate(rng, page) : page;
}

function operationJson(rng, position) {
  const kind = position % 6;
  if (kind < 3) return `{"operation":"patch","changes":${changesJson(rng)}}`;
  if (kind === 3) {
    const time = rng.below(3) === 0 ? stringValue(rng) : AT;
    return `{"operation":"review","actor":${JSON.stringify(stringValue(rng))},"time":${JSON.stringify(time)}}`;
  }
  if (kind === 4) {
    const fields = ['"operation":"promote"'];
    for (const name of ["type", "title", "description"]) {
      const roll = rng.below(4);
      if (roll === 0) continue;
      fields.push(`${JSON.stringify(name)}:${roll === 1 ? "null" : JSON.stringify(stringValue(rng))}`);
    }
    if (rng.below(2) === 0) {
      const tags = [];
      for (let i = rng.below(3); i > 0; i -= 1) tags.push(JSON.stringify(stringValue(rng)));
      fields.push(`"tags":[${tags.join(",")}]`);
    }
    fields.push(`"session_name":${JSON.stringify(stringValue(rng))}`);
    fields.push(`"session_id":${JSON.stringify(rng.pick(["sess_1", stringValue(rng)]))}`);
    fields.push(`"from_path":${JSON.stringify(rng.pick(["/fs/n.md", stringValue(rng)]))}`);
    return `{${fields.join(",")}}`;
  }
  return '{"operation":"read"}';
}

// What the module answers, in the reference's words.
function answer(line) {
  const { text, op } = JSON.parse(line);
  if (op.operation === "read") {
    try {
      const got = fm.read(text);
      if (got.raw === null) return { read: null };
      const { raw, type, title, description, status, tags, stale_after, okf_version, verified, sources, custom } = got;
      return { read: { type, title, description, status, tags, stale_after, okf_version, verified, sources, custom, raw } };
    } catch (err) {
      if (err instanceof FrontmatterError) return { error: err.code };
      throw err;
    }
  }
  return outcome(() => runOperation(text, op, line), line);
}

function same(js, rust) {
  if (js.ok !== undefined && rust.ok !== undefined) {
    return Buffer.from(js.ok, "utf8").equals(Buffer.from(rust.ok, "utf8"));
  }
  return isDeepStrictEqual(js, rust);
}

// Inputs on which the two sides are known to differ, each with the reason.
// They are outside what the corpus may hold, and are pinned so that a change
// on either side is noticed.
const PINNED = [
  {
    why: "-0 is spelled as a float to the reference's JSON reader; by value it is the integer 0",
    op: '{"operation":"patch","changes":[["k",-0]]}',
    rust: { error: "invalid_value" },
    js: { ok: "---\nk: 0\n---\n" },
  },
  {
    why: "1.0 is an integer by value; the corpus README rules the spelling out",
    op: '{"operation":"patch","changes":[["k",1.0]]}',
    rust: { error: "invalid_value" },
    js: { ok: "---\nk: 1\n---\n" },
  },
  {
    why: "JSON.parse keeps the last of a repeated field and cannot see the repeat",
    op: '{"operation":"patch","changes":[["k",[{"by":"a","by":"b"}]]]}',
    rust: { error: "invalid_value" },
    js: { ok: "---\nk:\n  - by: b\n---\n" },
  },
  {
    why: "2^63 - 1 fits the reference's integer and rounds to 2^63 here, which moves it across the order of checks",
    op: '{"operation":"patch","changes":[["bad key","x"],["k",9223372036854775807]]}',
    rust: { error: "invalid_key" },
    js: { error: "invalid_value" },
  },
];

function runReference(lines) {
  // Under the build tree: the system temp directory is often memory.
  const scratch = path.join(ROOT, "target", "tmp");
  fs.mkdirSync(scratch, { recursive: true });
  const dir = fs.mkdtempSync(path.join(scratch, "frontmatter-differential-"));
  try {
    const cases = path.join(dir, "cases.jsonl");
    const answers = path.join(dir, "answers.jsonl");
    fs.writeFileSync(cases, `${lines.join("\n")}\n`);
    const run = spawnSync(
      "cargo",
      ["test", "--quiet", "--test", "frontmatter_reference", "--", "--ignored", "--exact", "answer_the_cases_in_a_file"],
      {
        cwd: ROOT,
        env: { ...process.env, FRONTMATTER_CASES: cases, FRONTMATTER_ANSWERS: answers },
        encoding: "utf8",
        maxBuffer: 64 * 1024 * 1024,
      },
    );
    if (run.error) fail(`cargo did not run: ${run.error.message}`);
    if (run.status !== 0) fail(`the reference failed:\n${run.stdout}\n${run.stderr}`);
    const out = fs.readFileSync(answers, "utf8").split("\n");
    if (out.pop() !== "") fail("the reference's answers do not end in a newline");
    if (out.length !== lines.length) fail(`${lines.length} cases, ${out.length} answers`);
    return out.map((line) => JSON.parse(line));
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

function differentialChecks(manifest) {
  const corpusInputs = manifest.cases
    .map((meta) => decode(readFixture(meta.input), meta.input))
    .filter((text) => text !== "");

  const lines = [];
  for (let i = 0; i < args.cases; i += 1) {
    const rng = new Rng(args.seed + i);
    const text = pageText(rng, corpusInputs);
    lines.push(`{"text":${JSON.stringify(text)},"op":${operationJson(rng, i)}}`);
  }
  const pinnedLines = PINNED.map((pin) => `{"text":"","op":${pin.op}}`);

  let answers = null;
  check("differential: the reference answers", () => {
    answers = runReference([...lines, ...pinnedLines]);
    return `${answers.length} cases`;
  });
  if (answers === null) return;

  check("differential: same bytes or same refusal", () => {
    const tally = new Map();
    const divergences = [];
    lines.forEach((line, i) => {
      const rust = answers[i];
      if (rust.load_error !== undefined) {
        divergences.push(`seed ${args.seed + i}: the reference could not load ${line}: ${rust.load_error}`);
        return;
      }
      const js = answer(line);
      const label = js.error ?? (js.ok !== undefined ? "ok" : "read");
      tally.set(label, (tally.get(label) ?? 0) + 1);
      if (!same(js, rust)) {
        divergences.push(`seed ${args.seed + i}: ${line}\n          module: ${show(js)}\n       reference: ${show(rust)}`);
      }
    });
    const counts = [...tally.entries()].sort().map(([label, n]) => `${label} ${n}`).join(", ");
    if (divergences.length > 0) {
      fail(`${divergences.length} divergences (${counts}):\n      ${divergences.slice(0, 20).join("\n      ")}`);
    }
    // A generator that stops reaching a refusal, or reaches nothing else,
    // has rotted.
    if (args.cases >= 6000) {
      for (const code of CODES) {
        if (!tally.has(code)) fail(`no generated case was refused with ${code} (${counts})`);
      }
      if ((tally.get("ok") ?? 0) < args.cases / 10) fail(`too few accepted cases (${counts})`);
    }
    return `seeds ${args.seed} to ${args.seed + args.cases - 1}: ${counts}`;
  });

  check("differential: the known differences are exactly the pinned ones", () => {
    PINNED.forEach((pin, i) => {
      const rust = answers[lines.length + i];
      const js = answer(pinnedLines[i]);
      const rustWant = pin.rust === "load_error" ? rust.load_error !== undefined : isDeepStrictEqual(rust, pin.rust);
      if (!rustWant) fail(`${pin.op}: the reference now answers ${show(rust)} (${pin.why})`);
      if (!isDeepStrictEqual(js, pin.js)) fail(`${pin.op}: the module now answers ${show(js)} (${pin.why})`);
    });
    return `${PINNED.length} pinned`;
  });
}

// ---------------------------------------------------------------------------

const wants = (layer) => args.only === null || args.only === layer;

let manifest = null;
if (wants("corpus")) {
  manifest = corpusChecks();
} else {
  manifest = loadManifest();
}
if (wants("properties")) propertyChecks(manifest);
if (args.differential && wants("differential")) differentialChecks(manifest);

const failed = results.filter((result) => !result.ok);
console.log(
  `test-frontmatter: ${results.length} checks, ${failed.length} failed` +
    (args.differential ? "" : " (differential not run: pass --differential, needs cargo)"),
);
process.exit(failed.length === 0 && results.length > 0 ? 0 : 1);
