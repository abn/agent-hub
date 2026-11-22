import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { contextBlock, notices } from "../format.mjs";
import { resolveContext } from "../resolve.mjs";

const dir = join(dirname(fileURLToPath(import.meta.url)), "../../contract/fixtures");
const family = (prefix) =>
  readdirSync(dir)
    .filter((file) => file.startsWith(`${prefix}-`) && file.endsWith(".json"))
    .map((file) => JSON.parse(readFileSync(join(dir, file), "utf8")));

describe("resolve fixtures", () => {
  const cases = family("resolve");
  it("finds the family", () => expect(cases.length).toBeGreaterThan(20));
  for (const fixture of cases) {
    it(fixture.name, () => {
      const got = resolveContext(fixture.input);
      expect(got).toEqual(fixture.expected);
    });
  }
});

describe("notice fixtures", () => {
  const cases = family("notice");
  it("finds the family", () => expect(cases.length).toBeGreaterThan(10));
  for (const fixture of cases) {
    it(fixture.name, () => {
      expect(notices(fixture.input)).toEqual(fixture.expected);
    });
  }
});

describe("context fixtures", () => {
  const cases = family("context");
  it("finds the family", () => expect(cases.length).toBeGreaterThan(10));
  for (const fixture of cases) {
    it(fixture.name, () => {
      expect(contextBlock(fixture.input)).toEqual(fixture.expected);
    });
  }
});
