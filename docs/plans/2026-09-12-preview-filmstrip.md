# preview: filmstrip layout in single mode

Date: 2026-09-12
Branch: `preview-filmstrip`

## Problem

`peitho preview` single mode shows one slide and its notes. Knowing where
that slide sits in the deck, or jumping to another one, means toggling into
the grid overview and back. Every slide editor (Google Slides, Keynote,
PowerPoint) keeps a filmstrip of thumbnails beside the stage for exactly this.

## Decisions (author, 2026-09-12)

- Grid mode stays. `o` / Esc / Enter and the grid keyboard contract are
  unchanged; only the single-mode layout changes.
- Thumbnail click, strip width, and notes placement were left to the
  implementer: a thumbnail click is a direct `index` jump (same as a grid tile,
  so it can land on skipped slides); the strip is a fixed 200px column on the
  left; notes keep their fixed-height panel below the stage.

## Design

- Each slide gets a second shadow host built from the same HTML/CSS strings
  (`createSlideHost`), wrapped in a `.peitho-preview-thumb` element. A DOM node
  can only sit in one place, so the stage host and the thumbnail host are
  separate; the fragment is fetched once.
- A `<nav class="peitho-preview-strip">` pinned to the left edge holds the
  thumbnails as a vertical flex column and scrolls independently. Thumbnail
  hosts have `pointer-events: none`, so a click anywhere on a thumbnail
  navigates and links inside thumbnails are inert.
- Single mode fits the stage into the viewport minus the strip width and the
  notes height; the tile and the notes panel start at `PREVIEW_STRIP_WIDTH`.
  The current thumbnail is outlined and scrolled into view (`block: nearest`).
- ArrowUp/ArrowDown in single mode walk the filmstrip (skip-aware, like
  left/right); in grid mode they still move by a row.
- Grid mode hides the strip, exactly as it hides the notes panel.
- No new event, sync message, manifest field, or storage key: the filmstrip
  is a projection of the existing `currentIndex`.

## Tasks

1. TS: strip + thumbnails in `preview.ts`; tests for thumbnail count,
   selection, click navigation, stage offset, and grid hiding.
2. Rebuild `dist/preview.js`; README, guide, CLAUDE.md.
3. E2E in Chrome with `peitho preview examples/peitho-tour/deck.md`.
