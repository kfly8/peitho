# Preview: edit speaker notes in the filmstrip layout

Date: 2026-09-12
Branch: `preview-notes-edit`
Builds on: `docs/plans/2026-09-12-preview-speaker-notes.md` (Issue #472),
`docs/plans/2026-09-12-preview-filmstrip.md`

## Goal

`peitho preview` single mode shows the current slide's speaker note in a
fixed panel below the slide. Today that panel is read-only. The author wants
to type the note there and have it land in the deck's Markdown, because the
preview is the screen a deck is written against and Markdown is the source of
truth (pillar ①: content lives in Markdown, never in a side database).

Editing is preview-only. `peitho present` and the presenter view keep their
plaintext, read-only notes.

## Author decisions (2026-09-12)

1. **Save timing: autosave, no save button or shortcut.** Unsaved text is
   flushed when the note textarea loses focus and before every slide change,
   whichever navigation path caused it (thumbnail click, keyboard, grid
   selection). It is also flushed on `pagehide` with `keepalive` so a
   rebuild-triggered reload or a closed tab never drops a note.
2. **Keys while the textarea is focused:** arrows, Home, End, and every other
   key are ordinary text editing. Only PageUp / PageDown navigate slides
   (flushing first; the textarea keeps focus so the author can keep typing
   on the next slide). Escape leaves the textarea (blur) and does not enter
   grid mode; a second Escape does. `o`, Enter, and the other shell
   shortcuts are inert while typing.
3. **Write-back shape: one comment per slide.** Every note comment in the
   slide is removed and a single `<!-- ... -->` is written at the position
   of the first one. A slide with no note gets the comment appended at the
   end of its body (before the next `---`). Saving an empty note removes the
   comments. The page settings JSON comment is never touched. A slide that
   comes from an included file is written back to that file.
4. **Locating the comment: re-parse at save time.** The server reads the
   deck as it is on disk when the save arrives, parses it, finds the slide
   by key, and edits the origin file. There is no cached position and no
   hash guard because there is no cached state to go stale.

## Design

### Core: source spans on `ParsedSlide`

`parse_slide` already iterates `into_offset_iter`, so byte ranges are
available for every event. Two Parsed-phase-only fields are added to
`ParsedSlide`:

- `source_span: SourceSpan` — the slide's byte range in the parsed source
  (the `SlideRange` it was parsed from).
- `note_spans: Vec<SourceSpan>` — the byte range of each HTML comment that
  contributed to `notes`, in source order. Both an `HtmlBlock` (the range of
  the whole block) and an `InlineHtml` event (the range of that event) are
  recorded at the same seam that pushes to `note_fragments`, so the two lists
  cannot drift.

`SourceSpan` is a small `pub struct { start: usize, end: usize }` in
`domain.rs`. Neither field rides Mapped / Checked / Rendered; the renderer
and manifest are unchanged and existing decks build byte-identically.

Draft slides are dropped at parse end and page-settings comments are handled
before the note branch in `process_html_chunk`, so neither can appear in
`note_spans`.

### Core: pure rewrite

New module `crates/peitho-core/src/notes_edit.rs`:

```rust
pub fn rewrite_note(
    source: &str,
    slide: SourceSpan,
    notes: &[SourceSpan],
    text: &str,
    highlighter: &Highlighter,
) -> Result<String>;
```

Returns the full source with the slide's note rewritten. Rules (revised
2026-09-12 during Task 2 review, after measuring that in-place replacement
inside containers, inline positions, and next to `---` produced files that no
longer parsed back to the same deck):

- `text` is trimmed the way the parser trims a comment body. Empty text
  removes every note comment and inserts nothing.
- Single-line text is written as `<!-- text -->`; multi-line text as
  `<!--\ntext\n-->`.
- The first note span is replaced in place only when it is a whole-line
  comment starting at column 0 (a top-level HTML block); that is the one
  position where the canonical form is guaranteed to parse back as the same
  note. Every other span (inline, indented, inside a blockquote) is removed,
  and the canonical comment is appended after the slide's last non-blank line
  instead. Later spans are always removed.
- A removed whole-line comment takes its line ending with it, except when the
  lines above and below are both non-blank: then the line becomes a blank
  line (or a bare `>` inside a blockquote) so the neighbours never become
  adjacent — `text` followed by `---` would otherwise turn into a setext
  heading and silently merge two slides. Accepted consequence: a comment
  line removed from inside a tight list (`- item` / `  <!-- note -->` /
  `- next`) leaves a blank line that makes the list loose; deleting the line
  outright is not safe in general (it would merge `- item` / `  <!-- c -->` /
  `  more` into one paragraph), and the re-parse check does not compare
  fragments, so this is a rendering-only change. Likewise a comment that is a
  list item's sole content (`- <!-- x -->`) is not whole-line, so clearing it
  leaves an empty `- ` bullet behind rather than deleting the item line.
  Saving the same text twice is a no-op on the file (idempotent).
- With no note span, the comment is appended after the last non-blank line
  of the slide body, separated by one blank line, followed by a newline.
- The postcondition below is enforced inside `rewrite_note`: the source and
  the candidate are both parsed with the caller's `Highlighter`, and the
  candidate is returned only if slide count, keys, sections, every page
  setting projection, and every other slide's notes are unchanged and the
  target slide's notes equal the trimmed text. Otherwise the result is an
  `ErrorKind::Parse` error (`speaker note cannot be written at this
  position`) — for example when the slide ends inside an unclosed fenced code
  block. The unchecked splice is private, so no caller can skip the check.
- A leading BOM is stripped before splicing (spans are relative to the
  stripped text, as in `parse_markdown`) and re-added to the result.
- Line endings follow the note's own line (revised in Task 6): an inserted
  comment takes the terminator of the note line it replaces, or of the line
  it is appended after; a line with no terminator (the slide ends the file)
  falls back to `\r\n` when the slide contains one, else `\n`.
- Rejected with a line-numbered `BuildError` and help: text containing
  `-->` (cannot be represented in an HTML comment) and text whose trimmed
  form starts with `{` (the parser would read it as a page settings comment
  on the next build). These, the postcondition failure above, and spans that
  do not match the source are all `ErrorKind::Parse` errors so the CLI maps
  them to 422.

The postcondition is stated as a property: re-parsing the returned source
yields the same slides, keys, and page settings, with the target slide's
`notes` equal to the trimmed text (or `None` for empty text), and every other
slide's bytes unchanged.

### Core: mapping spans back to the origin file

The parsed source is the include-expanded combined source. A helper on
`include::LineMap` translates a combined-source byte span into an
`OriginSpan` (origin file plus line/byte-column coordinates), and
`origin_span_to_range` converts those coordinates against a separately read
origin file into an exact byte range (revised 2026-09-12 during Task 3
review):

- Synthetic bytes (a generated terminating newline after an included file
  without one, a generated blank line before a following `---`, or the
  terminator that expansion rewrites at the frontmatter boundary) are bytes the
  origin file lacks or whose terminator expansion rewrote. A
  `ParsedSlide.source_span` routinely ends on one (the last slide of an included
  file followed by `---`) or starts on one (the first slide after frontmatter
  when it is an include), so `translate_span` clips synthetic bytes off both
  ends of the span and reports the clipped combined sub-span as
  `OriginSpan::combined`, whose bytes are byte-for-byte the origin bytes. The
  result is `None` only when a synthetic byte lies strictly inside the span,
  when the span touches two files, when lines are not consecutive, or when
  nothing but synthetic bytes remains; then the CLI refuses the save ("this
  slide cannot be edited from preview; edit the note in `<file>`").
- Column offsets are preserved because include expansion splices whole
  lines.
- Coordinates are `LineCol { line, byte_col }` (1-based line, 0-based byte
  column). The span end is exclusive and is expressed on the line of the last
  byte covered, so a span that ends with a line's terminator has `byte_col`
  equal to that line's length including the terminator, and a span ending at
  EOF is representable. `origin_span_to_range` accepts a byte column up to
  and including that length, refuses anything beyond it, and refuses an
  offset that is not a char boundary (the origin may have changed on disk).
- Byte columns are relative to the BOM-stripped origin bytes.
  `expand_includes` strips one leading BOM from every source it reads (top
  deck and included files, at the read seam, before the included-file
  validators) so `body_start` and the combined source agree;
  `origin_span_to_range` skips a leading BOM in the origin text it is given
  and offsets the range past it, so the CLI indexes the raw file bytes.
- The rewritten slide is taken from
  `rewritten[combined.start..slide.source_span.end + rewritten.len() -
  combined_source.len()]` (add before subtracting, the rewrite may shrink the
  source): a leading synthetic byte is never touched by the rewrite and is
  skipped, while trailing synthetic bytes (a generated terminator and a
  generated blank line) that the rewrite leaves in place become real bytes in
  the origin (a one-time whitespace normalization; the next expansion then needs
  no synthetic bytes there). `rewrite_note` picks the terminator for inserted
  lines from the replaced note line, or the line the comment is appended after,
  falling back to the slide's own bytes (revised 2026-09-13 in Task 6 review),
  so an LF include inside a CRLF deck keeps LF; the CLI converts only bare LF
  bytes (the materialized synthetic ones) to CRLF when the origin is
  consistently CRLF and leaves mixed-ending files exactly as produced. A save
  whose rewritten slide equals the origin's current bytes writes nothing at all
  (this is what keeps a save of unchanged text a no-op even when trailing
  synthetic bytes exist).

For a deck without includes the map is the identity and the origin file is
the deck itself.

### CLI: parse-for-notes and the writer

`peitho_core` exposes `pub fn parse_deck(source, frontmatter, highlighter) ->
Result<UntransformedDeck>` (today `parse_markdown` is `pub(crate)` and only
reachable through `parse_deck_and_transform`, which runs `code_images`
renderers — external commands, Chrome, network — and must never run on a
note save). `UntransformedDeck` (revised 2026-09-13 during Task 4 review)
wraps the parsed deck and exposes only `parsed_slides()` and `settings()`;
it cannot enter mapping or rendering, so a deck whose declared renderers
never ran is unrepresentable on the build path by type rather than by
convention. `parse_deck` also skips transform-phase validation (embed URL
and `mode=` rules, external-command failures), so a deck it accepts may
still fail `peitho build`; the next watch rebuild reports that as usual.

In `main.rs`, the preview command builds a `NotesWriter` closure and hands it
to the server:

1. `load_and_expand_deck_source(input)` (existing) → combined source,
   frontmatter, `LineMap`.
2. Resolve the highlighter exactly as `build_artifacts` does (shared helper
   extracted from it; no duplicated syntax resolution).
3. `parse_deck` → find the `ParsedSlide` whose key matches. Missing key →
   conflict.
4. `rewrite_note` on the combined source with the `ParsedSlide`'s own
   `source_span` and `note_spans`, then translate only the slide span to the
   origin file via `LineMap` and splice the rewritten slide bytes into the
   origin file text. The two routes are not equivalent (revised 2026-09-12):
   `rewrite_note` re-parses the text it is given, and an origin file parsed
   on its own lacks the top deck's frontmatter and still carries `include`
   comments, so running it on origin text would reject every save on a deck
   with includes.
5. Write atomically (temp file + rename in the same directory; `write_atomic`
   already exists in `server.rs`). The server serializes concurrent
   saves (revised in Task 5).

The watch loop already observes the deck and included files, so the save
causes a normal rebuild and generation bump. Nothing else triggers reloads.

### Server: `POST /notes`

Mirrors `POST /rehearsal`: the endpoint exists on `PresentServer`, and
whether it does anything is decided in one place. `PresentServer::
with_notes_writer(NotesWriter)` is called only by `peitho preview`.

Request body: `{"key": "<slide key>", "text": "<note>"}`, sent with
`Content-Type: application/json`. The server requires that media type
(revised 2026-09-13 during Task 5 review): a `text/plain` body would be a
CORS "simple request" that any page the author has open could fire at
`127.0.0.1:<port>` without a preflight, whereas a JSON content type forces a
preflight that the server answers with 405. `key` deserializes as a
`SlideKey`, so malformed keys are rejected before any deck work. The server
holds a lock around the writer call, so saves are serialized by construction
rather than by a convention inside the writer, and runs the call on a spawned
thread like the long-poll handlers so a save never stalls `/sync` or static
fetches on the single preview listener. `write_atomic` follows an
existing symlink to its canonical target, keeps the target's permissions, and
removes its staging file when the rename fails, so a symlinked or 0600 deck
survives a save unchanged in shape.

| Outcome | Status | Body |
| --- | --- | --- |
| saved | 200 | `{"saved":true}` |
| invalid JSON / missing fields / unknown fields / malformed key / wrong content type | 400 | text |
| no writer (present, or any non-preview server) | 404 | text |
| deck does not parse, key not found, span not editable | 409 | `{"error":"<message>"}` |
| text not representable (`-->`, leading `{`) | 422 | `{"error":"<message>"}` |
| I/O failure | 500 | `{"error":"<message>"}` |

Error messages are the `BuildError` message plus help, rendered as plain text
so the shell can show them verbatim. The server never runs a rebuild itself.

### Shell (`preview.ts`)

- The note body `<div>` becomes a `<textarea data-peitho-preview="note">`
  styled like the current text (same font, transparent background, no
  border, no resize handle, fills the panel below the position line). The
  dimmed placeholder becomes the textarea's `placeholder` attribute.
- The position line gains a status span on the right
  (`data-peitho-preview="status"`) that shows the last save error (red) and
  is empty otherwise. No spinner.
- Dirty is derived, not stored: `textarea.value !== (notes[key] ?? "")`.
- `flushNotes(keepalive = false)`: if the current slide's textarea is
  dirty, `POST /notes` with `{key, text}`. On 200 the in-memory `notes`
  map is updated (entry removed when text is empty). On any error the
  status shows the message and the textarea keeps its text, so the next
  flush retries.
- Flush points: textarea `blur`, the start of `setIndex` (before
  `currentIndex` moves), `exitGrid`/`enterGrid` when the index changes,
  and `pagehide` with `keepalive: true`. `saveState` (called by
  `installPreviewReload` before `location.reload()`) also stores the draft.
- A slide change with a dirty note waits for the flush and is committed only
  on success. A failed save (409 while the deck is broken, 422 for
  unrepresentable text) shows its message in the status line and leaves the
  index, text, and focus where they were, so unsaved text is never lost and
  the next flush retries. Approved by the author 2026-09-12.
- Preview state gains `draft?: { key, text, selectionStart, selectionEnd,
  focused }`. After a reload, if `draft.key` is the current slide's key the
  textarea is filled from the draft and, when `focused`, refocused with the
  selection restored. Dirty then follows the same derived rule, so a draft
  whose rebuild already landed is clean and one still in flight is retried
  on the next flush. The draft is cleared from storage once applied.
- Escape inside the textarea blurs it (handled on the textarea's own
  `keydown` in the shell; the blur flushes). Focus survives PageUp/PageDown
  because the textarea element is reused across slides.

### Keyboard (`installPreviewKeyboard`)

When `event.target` is a `<textarea>` (or any editable element), the
installer dispatches `peitho:navigate` only for PageUp / PageDown
(`preventDefault`) and returns for every other key without touching the
event, so text editing keys and Escape reach the textarea. All existing
behavior outside the textarea is unchanged. `hasChordModifier` still applies.

§16 holds: the keyboard module only emits request events; the blur, the
flush, and the state transitions live in the preview shell.

### Non-goals

- Editing in `peitho present` / the presenter.
- Markdown rendering of notes (still an undecided item).
- A notes-only rebuild path. A note save is an ordinary file change and goes
  through the watch rebuild like any other edit.
- Creating or reordering slides from the preview.

## Edge cases

- **Slide with several note comments:** collapsed into one at the first
  comment's position. Documented in the guide.
- **Note that contains `-->`:** 422, shown in the status line, text kept in
  the textarea.
- **Note that starts with `{`:** 422 for the same reason (it would become a
  page settings comment).
- **Deck currently broken:** the preview shows the error page and has no
  textarea. A save racing a breaking edit gets 409 and is retried after the
  next successful build's reload (the draft survives in preview state).
- **Key vanished between build and save** (the author retitled the slide
  in the editor and the derived key changed): 409 with the key in the
  message; the draft stays in the textarea.
- **Included slides:** written to the include file. Synthetic-origin spans
  are refused with a message naming the file to edit.
- **Skipped slides:** editable like any other.
- **Concurrent saves:** serialized by the writer mutex; the watch debounce
  coalesces the rebuilds.
- **CRLF files:** inserted line endings match the replaced or preceding
  line (falling back to the slide's bytes).
- **BOM:** stripped before offsets are used, restored on write.
- **`.peitho/preview-cache/`, `dist/`, `notes.json`:** unchanged in shape.
  Notes still never enter `dist/`.

## Files

- `crates/peitho-core/src/domain.rs` — `SourceSpan`
- `crates/peitho-core/src/phase.rs` — `ParsedSlide.source_span`,
  `ParsedSlide.note_spans`
- `crates/peitho-core/src/parser.rs` — record spans; `pub fn parse_deck`
- `crates/peitho-core/src/notes_edit.rs` — `rewrite_note` and its property
  tests
- `crates/peitho-core/src/include.rs` — span translation on `LineMap`
- `crates/peitho/src/server.rs` — `POST /notes`, `NotesWriter`,
  `with_notes_writer`
- `crates/peitho/src/main.rs` — preview wires the writer; shared highlighter
  resolution
- `packages/peitho-present/src/preview.ts` — textarea, flush, draft state
- `packages/peitho-present/src/preview.ts` (`installPreviewKeyboard`) —
  editable-target gate
- `packages/peitho-present/test/preview.test.ts`
- `packages/peitho-present/dist/preview.js` (rebuilt, committed)
- `README.md`, `site/content/guide/cli.md`, `CLAUDE.md`

## Verification

- Rust unit tests for spans, `rewrite_note` (each rule above plus the
  re-parse property), `LineMap` translation, and the `/notes` status table.
- Vitest for flush points, dirty derivation, draft restore, keyboard gate.
- E2E in Chrome: `peitho preview examples/peitho-tour/deck.md`, type a note,
  click away → `deck.md` changes and the preview reloads with the note;
  PageDown while typing keeps focus; a note with `-->` shows the error and
  keeps the text; an included-slide deck writes to the include file.
