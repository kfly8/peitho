// Mirrored in crates/peitho-core/src/render.rs's embedded distribution-viewer script (kept
// in sync by hand — that one runs as a plain <script>, not a bundled module, so it can't
// import this).
const CLASSIC_JAVASCRIPT_TYPES = new Set(["", "text/javascript", "application/javascript"]);

// Classic scripts share ONE global scope regardless of Shadow DOM boundaries, so re-running
// the same layout's script (a second slide, a repeat visit) throws "already declared"
// without this wrap.
function needsScopeWrap(script: HTMLScriptElement): boolean {
  if (script.hasAttribute("src")) return false;
  return CLASSIC_JAVASCRIPT_TYPES.has((script.getAttribute("type") ?? "").trim().toLowerCase());
}

/**
 * A `<script>` inserted via `innerHTML` never executes — the HTML spec marks it "already
 * started". Re-creates each one as a fresh element so it actually runs.
 */
export function executeInlineScripts(root: ParentNode, doc: Document): void {
  for (const oldScript of Array.from(root.querySelectorAll("script"))) {
    const newScript = doc.createElement("script");
    for (const attribute of Array.from(oldScript.attributes)) {
      newScript.setAttribute(attribute.name, attribute.value);
    }
    newScript.textContent = needsScopeWrap(oldScript)
      ? `(function () {\n${oldScript.textContent ?? ""}\n})();`
      : oldScript.textContent;
    oldScript.replaceWith(newScript);
  }
}
