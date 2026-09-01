# External links open in a new tab (Issue #455)

Date: 2026-09-01
Issue: #455 — `[text](https://…)` in slide content navigates the deck away

## Problem

Markdown links render as plain `<a href="…">`. Clicking one while
presenting replaces the deck with the target page and loses the slide
position. This hits `peitho preview` and the built deck alike, and decks
that cite a source per slide hit it on every slide. A site-side
`<base target="_blank">` only fixes the published copy, so the fix belongs
in the renderer.

Two things have to hold for "clicking a citation never loses the slide":

1. the link opens somewhere other than the deck's own window/tab, and
2. the click that opens it does not *also* advance the deck — the shell's
   canvas click navigation fires prev/next for any click on the slide.

## Design

### Renderer: one `a[href]` post-pass at the slot seam

`render_slot` in `render.rs` is the single function that produces every
slot's HTML — headings, body Markdown (paragraphs, lists, blockquotes,
tables, footnote entries), and embed cards. Its output goes through
`open_external_links_in_new_tab`, a `lol_html::rewrite_str` pass with one
`a[href]` handler:

- an `http:` / `https:` href (ASCII case-insensitive scheme) gets
  `target="_blank"`, plus `rel="noopener"` only when the anchor has no
  `rel` yet — embed-card anchors already carry `rel="noopener noreferrer"`
  and keep it verbatim;
- everything else is untouched: relative paths and `#fragment` keep the
  browser default (the issue's stated rule), and `mailto:` / `tel:` never
  replace the deck in the first place, so `_blank` would only leave an
  empty window behind after the OS handoff.

Doing it once at the slot seam (rather than rewriting pulldown-cmark link
events and hand-editing the card writers) means Markdown links and the
hand-built card anchors share one rule, pulldown-cmark's own writer keeps
producing the `<a>` bytes, no escaping is reimplemented, and there is no
per-consumer literal to forget. Layout HTML is not rewritten: a layout is
author-controlled HTML and can carry `target` itself. Decks without links
are byte-identical — lol_html passes untouched elements through verbatim.
Everything reaching the pass is generated HTML (the parser rejects raw HTML
in content), so a lol_html parse failure there is an internal render error,
reported at the slot's first fragment line with a "report this issue" help.

`render_heading_inline` used to duplicate the footnote-reference arm of
`normalize_markdown_event` and bypass it. It now routes heading events
through `normalize_markdown_event` (`breaks = false`; a setext heading can
carry a soft break, but heading `breaks` handling is pre-existing and
out of scope). One observable consequence is pinned by a test: an HTML
comment inside a heading (`# Title <!-- secret -->`) is dropped like body
comments, instead of being emitted verbatim into the title slot — which
also stops a speaker-note comment written in a heading from leaking into
`dist/`.

### Shell: a click on a link never navigates the deck

`createClickNavigationGuard.shouldIgnoreClick` in
`packages/peitho-present/src/clickNavigationGuard.ts` is the shared guard
for present's canvas click navigation, preview's grid tiles, and the
"kept in sync" inline copy in `render_distribution_index` (dist
`index.html`). It now also ignores a click whose `composedPath()[0]` is
inside an `<a>` (the slide canvas is a shadow root, so `event.target` is
retargeted to the host and `closest("a")` on it would miss). The inline
copy carries the identical check.

In `peitho present` the slides window is a Chrome `--app` window without a
tab strip, so `_blank` opens a new browser window there; in preview and
the built deck it is a new tab.

## Tests

Renderer, `render.rs`:

1. Table-driven unit test over `open_external_links_in_new_tab`: `https://`,
   `http://`, `HTTPS://`, an href with `&amp;` (attribute bytes preserved),
   `mailto:`, `tel:`, `other.html`, `/abs/path`, `#top` (untouched), an
   anchor with an existing `rel` (kept verbatim, target added), an anchor
   with a `title`, and HTML with no anchors (byte-identical).
2. Deck tests: paragraph link, heading link, footnote-body link, and the
   card anchors (X card author anchor, generic card permalink — the X
   card's link/date anchors take the same path) carrying
   `target="_blank" rel="noopener noreferrer"`.
3. `# Title <!-- secret -->` renders the title slot without the comment.
4. The distribution index inline script contains the anchor check.
5. A lol_html strict-mode failure (`<select><xmp><script>`) surfaces as the
   internal render error with the slot's line.

Shell, vitest: a click whose composed path starts inside an `<a>` is
ignored by the guard; a click on plain text still navigates.

## Non-goals

- No per-link or per-deck opt-out.
- No change to `plain.rs` (text extraction), lint, PDF, or the presenter.
- Anchors written in layout HTML are left as authored.
