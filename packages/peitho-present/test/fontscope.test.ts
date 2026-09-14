import { expect, it } from "vitest";
import { extractFontScopeCss } from "../src/fontscope";

it("extracts leading imports after charset comments and whitespace", () => {
  const css = `
/* deck fonts */
@charset "UTF-8";

@import url("fonts/noto-sans-jp/index.css");
@import url("fonts/inter/index.css") screen;
.peitho-slide { color: red; }
`;

  expect(extractFontScopeCss(css)).toBe(
    [
      '@import url("fonts/noto-sans-jp/index.css");',
      '@import url("fonts/inter/index.css") screen;'
    ].join("\n")
  );
});

it("does not promote imports after ordinary rules", () => {
  const css = `
@import url("fonts/prefix.css");
.peitho-slide { color: red; }
@import url("fonts/late.css");
@font-face { font-family: "Late"; src: url("fonts/late.woff2"); }
`;

  expect(extractFontScopeCss(css)).toBe(
    [
      '@import url("fonts/prefix.css");',
      '@font-face { font-family: "Late"; src: url("fonts/late.woff2"); font-display:block;}'
    ].join("\n")
  );
});

it("extracts top level font face blocks from anywhere", () => {
  const css = `
.slot-title { color: red; }
@font-face { font-family: "Heading"; src: url("fonts/heading.woff2"); }
.slot-body { color: blue; }
@font-face {
  font-family: "Body";
  src: url("fonts/body.woff2");
}
`;

  expect(extractFontScopeCss(css)).toBe(
    [
      '@font-face { font-family: "Heading"; src: url("fonts/heading.woff2"); font-display:block;}',
      '@font-face {\n  font-family: "Body";\n  src: url("fonts/body.woff2");\nfont-display:block;}'
    ].join("\n")
  );
});

it("skips comments and strings while scanning font face blocks", () => {
  const css = `
.fake::before { content: "@font-face { nope }"; }
/* @font-face { font-family: "Comment"; } */
@font-face {
  font-family: "Brace } Face";
  src: url("fonts/{brace}.woff2");
  unicode-range: U+0-5FF; /* } */
}
`;

  expect(extractFontScopeCss(css)).toBe(
    [
      "@font-face {",
      '  font-family: "Brace } Face";',
      '  src: url("fonts/{brace}.woff2");',
      "  unicode-range: U+0-5FF; /* } */",
      "font-display:block;}"
    ].join("\n")
  );
});

it("omits non font rules", () => {
  const css = `
@media screen {
  @font-face { font-family: "Nested"; src: url("fonts/nested.woff2"); }
}
.peitho-slide { font-family: "Nested"; }
`;

  expect(extractFontScopeCss(css)).toBe("");
});

it("replaces an authored font-display with block", () => {
  const css = `@font-face { font-family: "Inter"; src: url("theme-fonts/Inter.woff2"); font-display: swap; }`;

  const scoped = extractFontScopeCss(css);
  expect(scoped).not.toContain("swap");
  expect(scoped.match(/font-display/g)).toHaveLength(1);
  expect(scoped).toContain("font-display:block;");
});

it("adds block to a face that declares no font-display", () => {
  const css = `@font-face { font-family: "Inter"; src: url("theme-fonts/Inter.woff2"); }`;

  expect(extractFontScopeCss(css)).toContain("font-display:block;");
});

it("forces block on every face, not just the first", () => {
  const css = `
@font-face { font-family: "A"; src: url("a.woff2"); font-display: swap; }
@font-face { font-family: "B"; src: url("b.woff2"); font-display: optional; }
`;

  const scoped = extractFontScopeCss(css);
  expect(scoped.match(/font-display:block;/g)).toHaveLength(2);
  expect(scoped).not.toContain("swap");
  expect(scoped).not.toContain("optional");
});
