# preview: show speaker notes below the slide in single mode

Date: 2026-09-12
Issue: #472
Branch: `preview-notes`

## Problem

Speaker notes are written in the deck as HTML comments, but the only place
they render is the presenter window of `peitho present`. `peitho preview` is
the screen an author writes against, so checking a note today means leaving
the editing loop and starting a presentation.

## Decisions (author, 2026-09-12)

- Single mode only. The grid overview stays as is.
- Always visible: a fixed panel below the slide, no toggle key.
- Plaintext (`textContent`), same as the presenter. Markdown rendering of
  notes stays an undecided item (CLAUDE.md).

## Design

- `emit_preview_cache_generation` writes `notes.json` next to `manifest.json`
  in every generation directory. The preview cache is local and never enters
  `dist/`, so `PRESENTATION_ONLY_DIST_FILES` and the publish contamination
  check are untouched. The present cache and the preview cache share one
  `write_notes_json` helper so the two cannot drift.
- The preview shell fetches `notes.json` right after `manifest.json` (still
  after the `/sync` generation handshake) and keeps the `Notes` binding in
  memory. A `<aside class="peitho-preview-notes">` panel is appended to the
  root after the slide tiles.
- In single mode the slide is fitted into the viewport minus a panel of
  `PREVIEW_NOTES_HEIGHT` px pinned to the bottom; the panel shows the current
  slide's note or a dimmed "No notes for this slide." placeholder. In grid
  mode the panel is hidden and the layout is unchanged.
- The panel is a sibling of the tiles, so clicking it neither navigates nor
  leaves grid mode, and text in it can be selected.

## Tasks

1. Rust: `write_notes_json` helper; preview generation emits `notes.json`;
   flip the `emit_preview_cache_writes_preview_only_files_in_generation_dir`
   assertion.
2. TS: fetch notes, notes panel, single-layout fit; tests for note text,
   placeholder, grid hiding, and fetch order.
3. Rebuild `dist/preview.js`; README, `site/content/guide/cli.md`, CLAUDE.md.
4. E2E in Chrome: `peitho preview examples/peitho-tour/deck.md`, check the
   panel in single mode and its absence in grid mode.
