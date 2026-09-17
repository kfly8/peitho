import { expect, it } from "vitest";
import { executeInlineScripts } from "../src/scripts";

// jsdom never actually executes <script> elements (vitest's jsdom environment doesn't opt
// into `runScripts: "dangerously"`), so these assert the structural swap only; real
// execution was verified by hand against a real browser.

it("replaces an inert (innerHTML-parsed) script with a fresh element, wrapped in an IIFE", () => {
  const container = document.createElement("div");
  container.innerHTML = '<p>before</p><script data-x="1">console.log("hi")</script><p>after</p>';
  const inert = container.querySelector("script")!;

  executeInlineScripts(container, document);

  const replaced = container.querySelector("script")!;
  expect(replaced).not.toBe(inert);
  expect(inert.isConnected).toBe(false);
  expect(replaced.getAttribute("data-x")).toBe("1");
  expect(replaced.textContent).toBe('(function () {\nconsole.log("hi")\n})();');
  // Position is preserved, not appended at the end.
  expect(container.querySelectorAll("p")).toHaveLength(2);
});

it("replaces every script, including multiple and nested ones", () => {
  const container = document.createElement("div");
  container.innerHTML = "<script>1</script><div><script>2</script></div><script>3</script>";
  const before = Array.from(container.querySelectorAll("script"));

  executeInlineScripts(container, document);

  const after = Array.from(container.querySelectorAll("script"));
  expect(after).toHaveLength(3);
  expect(after.map((s) => s.textContent)).toEqual(["(function () {\n1\n})();", "(function () {\n2\n})();", "(function () {\n3\n})();"]);
  for (const [i, script] of after.entries()) expect(script).not.toBe(before[i]);
});

it("wraps a classic script's own top-level declarations so two copies of the same layout don't collide", () => {
  const container = document.createElement("div");
  container.innerHTML = "<script>let n = 0</script>";

  executeInlineScripts(container, document);

  expect(container.querySelector("script")!.textContent).toContain("(function () {");
});

it("leaves a type=\"module\" script's source untouched (it already has its own scope)", () => {
  const container = document.createElement("div");
  container.innerHTML = '<script type="module">export const x = 1</script>';

  executeInlineScripts(container, document);

  const replaced = container.querySelector("script")!;
  expect(replaced.type).toBe("module");
  expect(replaced.textContent).toBe("export const x = 1");
});

it("leaves an external script's src reference untouched (nothing here to wrap)", () => {
  const container = document.createElement("div");
  container.innerHTML = '<script src="./widget.js"></script>';

  executeInlineScripts(container, document);

  const replaced = container.querySelector("script")!;
  expect(replaced.getAttribute("src")).toBe("./widget.js");
  expect(replaced.textContent).toBe("");
});

it("leaves a non-JS script type (e.g. an inline JSON data island) untouched", () => {
  const container = document.createElement("div");
  container.innerHTML = '<script type="application/json">{"a":1}</script>';

  executeInlineScripts(container, document);

  const replaced = container.querySelector("script")!;
  expect(replaced.textContent).toBe('{"a":1}');
});

it("adversarial: content with no script tags is left untouched", () => {
  const container = document.createElement("div");
  container.innerHTML = "<p>no scripts here</p>";

  executeInlineScripts(container, document);

  expect(container.innerHTML).toBe("<p>no scripts here</p>");
});

it("adversarial: works on a DocumentFragment, not just an Element", () => {
  const template = document.createElement("template");
  template.innerHTML = '<script>console.log("x")</script>';
  const fragment = template.content;

  executeInlineScripts(fragment, document);

  const script = fragment.querySelector("script")!;
  expect(script.textContent).toBe('(function () {\nconsole.log("x")\n})();');
});
