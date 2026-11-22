// The markdown sanitiser keeps a GFM task box and nothing else an input could
// carry, and still removes the form controls it always removed.

import fs from "node:fs";
import path from "node:path";
import { beforeAll, describe, expect, it } from "vitest";

import { renderMarkdown, sanitizeMarkdownHtml } from "../../web/sessions.mjs";

// jsdom does not load the vendored parser the way a page does, so it is run in
// the window the module reads it from.
beforeAll(() => {
  window.eval(fs.readFileSync(path.resolve(process.cwd(), "web/vendor/marked.js"), "utf8"));
});

function parse(html) {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host;
}

describe("renderMarkdown task lists", () => {
  it("keeps checked and unchecked state as disabled checkboxes", async () => {
    const host = parse(await renderMarkdown("- [x] done\n- [ ] open\n"));
    const boxes = [...host.querySelectorAll("input")];
    expect(boxes.map((box) => box.checked)).toEqual([true, false]);
    for (const box of boxes) {
      expect(box.type).toBe("checkbox");
      expect(box.disabled).toBe(true);
    }
  });

  it("keeps only type, checked and disabled on the box", () => {
    const host = parse(
      sanitizeMarkdownHtml(
        '<ul><li><input type="checkbox" checked name="x" onclick="alert(1)" style="color:red" class="c" value="v"> a</li></ul>',
      ),
    );
    const box = host.querySelector("input");
    expect(box).not.toBeNull();
    expect(box.getAttributeNames().sort()).toEqual(["checked", "disabled", "type"]);
  });

  it("disables a box the source left enabled", () => {
    const host = parse(sanitizeMarkdownHtml('<input type="checkbox">'));
    expect(host.querySelector("input").disabled).toBe(true);
  });
});

describe("sanitizeMarkdownHtml form controls", () => {
  it("strips a text input", () => {
    expect(parse(sanitizeMarkdownHtml('<p>a<input type="text" value="x">b</p>')).querySelector("input")).toBeNull();
  });

  it("strips an input with no type", () => {
    expect(parse(sanitizeMarkdownHtml("<p><input></p>")).querySelector("input")).toBeNull();
  });

  it("strips a form and its inputs", () => {
    const host = parse(sanitizeMarkdownHtml('<form action="/x"><input type="checkbox"><button>go</button></form><p>kept</p>'));
    expect(host.querySelector("form")).toBeNull();
    expect(host.querySelector("input")).toBeNull();
    expect(host.querySelector("button")).toBeNull();
    expect(host.textContent).toContain("kept");
  });
});

describe("sanitizeMarkdownHtml unwrapped elements", () => {
  it("sanitises an event-handler image inside an unknown wrapper", () => {
    const host = parse(sanitizeMarkdownHtml('<x-y><img src="https://example.com/a.png" onerror="alert(1)"></x-y>'));
    const img = host.querySelector("img");
    expect(img).not.toBeNull();
    expect(img.hasAttribute("onerror")).toBe(false);
  });

  it("removes a script inside an unknown wrapper", () => {
    const host = parse(sanitizeMarkdownHtml("<x-y><x-z><script>alert(1)</script>kept</x-z></x-y>"));
    expect(host.querySelector("script")).toBeNull();
    expect(host.textContent).toBe("kept");
  });

  it("removes a text input inside an unknown wrapper", () => {
    const host = parse(sanitizeMarkdownHtml('<x-y><input type="text" value="x"></x-y>'));
    expect(host.querySelector("input")).toBeNull();
  });

  it("removes a form control whose type is written in capitals", () => {
    const host = parse(sanitizeMarkdownHtml('<input type="TEXT"><input type="CHECKBOX">'));
    expect([...host.querySelectorAll("input")].map((box) => box.type)).toEqual(["checkbox"]);
  });
});
