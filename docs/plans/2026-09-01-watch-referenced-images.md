# Watch deck-referenced images in `preview` / `build --watch` (Issue #454)

## Problem

`resolve_watch_targets` builds the watch set from the deck file, its
includes, and `ResolvedAssets` (layouts / css / syntaxes / fonts). It never
parses the deck body, so `![x](img/a.png)` contributes no watch root.
Overwriting `img/a.png` in place produces no rebuild and preview keeps
serving the stale hashed asset until restart.

## Root cause

The watch set is derived from frontmatter + include expansion only. Image
references are body-level and are only discovered at
`resolve_image_paths` time inside the build. The fix makes the parser feed
the watch set — one upstream seam, not a per-consumer filter.

## Design (as landed)

1. **peitho-core**: `referenced_image_paths(source, frontmatter, highlighter)
   -> Result<Vec<RawImagePath>>` parses with `parse_markdown` and walks
   every slide's fragments (recursing into `SlotGroup` children, exhaustive
   `FragmentKind` match, deduplicated by path).
   - It deliberately parses *before* `transform_code_images`: transformed
     decks contain `FragmentKind::Image` whose `src` points into
     `.peitho/code-images-cache/` (and card thumbnails into
     `.peitho/embeds-cache/`). Watching those would turn every rebuild's
     cache write into a source change → rebuild loop (a past incident class
     per CLAUDE.md).
   - It returns only paths, never the untransformed `Deck<Parsed>`.
     Transformed and untransformed decks share one type, so exposing a
     parse-only entry would let a future caller dispatch a pre-transform
     deck (the hazard recorded in `docs/plans/2026-07-12-issue-241-code-images.md`
     Amendments). Keeping `parse_markdown` `pub(crate)` makes that state
     unrepresentable outside the crate.
2. **CLI `WatchTargets`**: referenced images (`deck_dir.join(raw)`) become
   file roots exactly like `included_files`. `watch_dirs()` already watches
   a file root's parent (nearest existing ancestor when missing) and
   `is_relevant_change` matches file roots by identity, so atomic-save
   final paths are covered without new mechanism. `WatchRoot.ext` is only
   consulted for directory roots, so image roots carry `ext: None`.
3. **`resolve_watch_targets`** loads the highlighter and calls
   `referenced_image_paths`. A highlighter/parse failure yields an empty
   image list but keeps includes and assets watched (the build reports the
   error loudly on its own).
4. **Targets refresh on every relevant change**, not only deck/include
   changes. Reason: a parse failure caused by a *non-source* input (an empty
   or broken deck-adjacent `syntaxes/`) left the image set empty; fixing the
   syntax file rebuilt fine but never re-derived the image roots until the
   deck itself was touched. Refreshing unconditionally removes the special
   case (`is_source_change` is gone).
5. Both `build --watch` and `preview` share `prepare_watch_loop`, so one
   change covers both commands.

## Investigated and rejected

- **Mixed-case references (`img/A.png` vs on-disk `a.png`)**: a review
  candidate claimed `same_watch_path`'s canonicalize comparison misses these
  on APFS. Measured 2026-09-01: Rust `fs::canonicalize` and C `realpath(3)`
  both return the on-disk casing (only Python's pure-Python
  `os.path.realpath` does not), so the existing comparison already matches
  the notify event. `same_watch_path` is unchanged.

## Accepted tradeoff

`resolve_watch_targets` parses the deck once more per relevant event (the
rebuild parses again with the full code_images transform). Deriving the watch
set from the build itself would avoid the double parse, but the build has no
image list when it fails (missing image, broken fence), which is exactly when
the watch set must still be correct. The extra parse is cheap relative to a
rebuild.

## Tests

- Core: recursive listing including `::: {slot=…}`, mermaid fence lists
  nothing, repeated references deduplicated.
- CLI: referenced images are relevant and their dirs watched; overwrite
  rebuilds with a new hash and drops the stale asset; changing the reference
  rewatches the new directory; parse-failing deck still watches includes and
  layouts; mermaid decks never watch `.peitho/`; highlighter recovery
  re-derives image roots.
- E2E (manual, 2026-09-01): `build --watch` and `preview` on
  `examples/image-showcase` rebuild on image overwrite (new hash under
  `dist/assets/`, `/sync` generation 0 → 1), including with `img/ARCH.png`
  referenced against on-disk `arch.png`.

## Out of scope

- Images referenced from layouts/CSS (`url(...)`) — those are asset roots
  already and CSS is watched as a whole.
