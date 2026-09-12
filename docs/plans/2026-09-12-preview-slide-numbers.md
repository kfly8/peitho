# preview: slide numbers on thumbnails and in the notes panel

Date: 2026-09-12
Branch: `preview-slide-numbers`

## Problem

`peitho preview` never says which slide is on screen. The presenter shows
`Slide 3 of 12`; the preview filmstrip and grid only carry the number in an
`aria-label`. While editing a deck, "which slide is this" and "where is slide
7" are constant questions.

The deck-level `page_numbers` frontmatter is a different thing: it renders a
number into the slide itself and therefore into PDF and `dist/`. Preview-only
numbering must stay in the shell so build output is untouched.

## Decisions (author, 2026-09-12)

- Both placements: a number badge on every filmstrip thumbnail and grid
  tile, plus `N / total` at the top of the notes panel in single mode.
- Always on. No frontmatter key, no toggle, same stance as the preview
  notes panel.
- Numbers are absolute 1-based `ManifestSlide.index + 1`, so skipped slides
  keep their number (that is how a `{"skip":true}` slide is found).

## Design

- `createSlideView` adds a `.peitho-preview-number` badge to the tile and to
  the thumbnail (bottom-left, `pointer-events: none`, above the shadow host).
  The tile badge is hidden in single mode, where the tile is the stage.
- The notes panel gets two children: a position line
  (`[data-peitho-preview="position"]`, `N / total`) and the note body
  (`[data-peitho-preview="note"]`). `renderNotes` writes both; the dimmed
  placeholder applies to the body only.
- No new event, sync message, manifest field, or storage key.

## Tasks

1. TS: badges + position line in `preview.ts`; tests.
2. Rebuild `dist/preview.js`; README, guide, CLAUDE.md.
3. E2E in Chrome with `peitho preview examples/peitho-tour/deck.md`.
