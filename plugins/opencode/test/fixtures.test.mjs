import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { briefLines } from "../bundle.mjs";
import { contextBlock, notices } from "../format.mjs";
import { resume, startNew } from "../child.mjs";
import { HubError } from "../hub.mjs";
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

describe("brief fixtures", () => {
  const cases = family("brief");
  it("finds the family", () => expect(cases.length).toBeGreaterThan(0));
  for (const fixture of cases) {
    it(fixture.name, () => {
      expect({ lines: briefLines(fixture.input.brief, fixture.input.hub_url) }).toEqual(fixture.expected);
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

describe("subagent fixtures", () => {
  const cases = family("subagent");
  it("finds the family", () => expect(cases.length).toBeGreaterThanOrEqual(10));
  for (const fixture of cases) {
    it(fixture.name, async () => {
      const { input, expected } = fixture;
      const replies = [...input.replies];
      const calls = [];
      const start = async (session_name, exclusive) => {
        calls.push({ session_name, exclusive });
        const reply = replies.shift();
        if (reply.ok) return reply.ok;
        throw new HubError(reply.error.error.message, undefined, reply.error.error.details);
      };
      let child = null;
      try {
        child = input.mapped
          ? await resume({ start, name: input.mapped })
          : await startNew({ start, parent: input.parent, agentType: input.agent_type, agentId: input.agent_id });
      } catch {
        child = null;
      }
      expect(calls).toEqual(expected.calls);
      expect(child?.name ?? null).toEqual(expected.child);
    });
  }
});
