# Preview: edit speaker notes in single mode

Date: 2026-09-12
Branch: `preview-notes-edit`

<!-- derived-from ../specs/2026-09-12-preview-notes-edit-design.md -->

## Goal

Turn the single-mode preview notes panel into an autosaving plaintext editor.
Each save re-reads and parses the deck, locates the slide by `SlideKey`, rewrites
exactly one speaker-note HTML comment in the owning Markdown file, and leaves the
normal preview watcher to rebuild and reload. Present mode and distributed output
remain read-only and unchanged.

## Verified existing seams

- `split_slide_ranges` converts each pulldown-cmark `local_range` into
  `global_start = content_start + range.start` and
  `global_end = content_start + range.end`; its `SlideRange { start, end }` is
  the exact value handed to `parse_slide`. `parse_slide` repeats the conversion
  relative to `range.start`. For `Start(Tag::HtmlBlock)`, the offset range covers
  the whole HTML block, so that `global_start..global_end` must be retained in
  `html_buf` while its per-line `Event::Html` strings are joined. For
  `Event::InlineHtml` (and the existing stray `Event::Html` path outside a
  block), that event's own `global_start..global_end` is the comment span.
  `process_html_chunk` handles page settings before notes; the note span is
  pushed in the same `extract_html_comment_body(raw)` branch as the matching
  `note_fragments.push(text)`, never in a separate pass.
- `LineOriginKind` has exactly three variants: `Source { line: usize }`,
  `SyntheticTerminator`, and `SyntheticLine`. `SyntheticTerminator` owns the
  newline inserted by `append_synthetic_boundary_newline` when the preceding
  included text had no final newline; it does not add an output line in
  `LineMap::translate`. `SyntheticLine` owns an inserted newline that creates a
  boundary blank line and does add an output line. They are created only by
  `append_region_leading_newline_if_needed` and
  `append_separator_boundary_blank_line_if_needed`. A half-open byte span
  touches a non-source origin exactly when it contains one of those generated
  newline bytes. Such a span, a span crossing files, or a span whose source line
  numbers are not contiguous cannot be represented by one origin-file byte
  range and must be refused.
- `build_artifacts` calls `resolve_assets(input, &loaded.frontmatter)`, then
  `load_highlighter(assets.syntaxes.path())`. `load_highlighter` uses
  `collect_asset_files(path, "sublime-syntax")` plus
  `Highlighter::with_user_files` for a resolved syntax path, or
  `Highlighter::defaults` when there is none. Note saving must reuse this seam
  and call the new parse-only `parse_deck`; it must never call
  `parse_deck_and_transform`, `CliSvgRunner`, `CliEmbedRenderer`, or
  `CliOEmbedFetcher`.
- `prepare_watch_loop` uses a `PollWatcher` with a 200 ms poll interval and a
  200 ms `DebounceConfig` timeout. One debounced event batch reaches
  `handle_watch_paths_with_rebuild`, which performs at most one rebuild.
  `resolve_watch_targets` already passes `LoadedDeckSource::included_files()`
  to `WatchTargets::new`; each included Markdown file is a `.md` watch root and
  its parent directory is registered. Therefore an atomic write to either the
  top deck or an include is already observed and coalesced. The notes writer
  must not call `rebuild_preview_once_for_watch`, `swap_root`, or
  `broadcast_reload`.

## Tasks

### Task 1: Record Parsed-phase slide and note byte spans

**Goal.** Give each surviving `ParsedSlide` its complete source range and the
ordered ranges of the non-empty comments that contributed to `notes`, without
carrying either field into `MappedSlide`.

**Files.**

- `crates/peitho-core/src/domain.rs`
- `crates/peitho-core/src/phase.rs`
- `crates/peitho-core/src/parser.rs`
- `crates/peitho-core/src/code_images.rs`
- `crates/peitho-core/src/mapping.rs`

**Test.** Add
`parser::tests::parsed_slide_records_html_block_and_inline_note_spans` and assert
the source slices, not only numeric offsets:

```rust
assert_eq!(slide.source_span, SourceSpan { start: 0, end: separator_start });
assert_eq!(slide.note_spans.len(), 2);
assert_eq!(source[slide.note_spans[0].start..slide.note_spans[0].end].trim_end(),
           "<!--\nblock note\n-->");
assert_eq!(&source[slide.note_spans[1].start..slide.note_spans[1].end],
           "<!-- inline note -->");
assert_eq!(slide.notes.as_deref(), Some("block note\n\ninline note"));
```

Also add `parser::tests::page_settings_and_empty_comments_do_not_get_note_spans`:

```rust
assert_eq!(slide.note_spans.len(), 1);
assert_eq!(&source[slide.note_spans[0].start..slide.note_spans[0].end],
           "<!-- real note -->");
```

**Implementation.** Add
`pub struct SourceSpan { pub start: usize, pub end: usize }` with
`Debug + Clone + Copy + PartialEq + Eq` in `domain.rs`, then add public
`source_span: SourceSpan` and `note_spans: Vec<SourceSpan>` fields to
`ParsedSlide`. Initialize `source_span` from the `SlideRange` passed to
`parse_slide`. Extend `html_buf` to retain the whole-block span captured from
the `Start(Tag::HtmlBlock)` event's `global_start` and `global_end`, making its
type `Option<(String, usize, SourceSpan)>`; pass that span to
`process_html_chunk` on `End(TagEnd::HtmlBlock)`. Pass the current event span
for `InlineHtml` and stray `Html`. Initialize `note_spans` in `parse_slide`, add
the `span: SourceSpan` and `note_spans: &mut Vec<SourceSpan>` parameters to
`process_html_chunk`, and push the span immediately beside
`note_fragments.push(text)` only after page-settings handling and only when
`extract_html_comment_body` returns `Some`. Update existing `ParsedSlide`
literals in the three test modules with inert spans. Do not copy the new fields
in `map_slide`; they end with `Deck<Parsed>`.

**Verification.**

```sh
cargo test -p peitho-core parsed_slide_records_html_block_and_inline_note_spans
cargo test -p peitho-core page_settings_and_empty_comments_do_not_get_note_spans
```

### Task 2: Implement the pure note rewrite

**Goal.** Canonicalize all note comments in one slide through a deterministic,
idempotent source-to-source function.

**Files.**

- `crates/peitho-core/src/notes_edit.rs`
- `crates/peitho-core/src/lib.rs`

**Test.** Add the following tests under `notes_edit::tests`:

- `rewrite_note_formats_single_and_multiline_text` asserts
  `<!-- one line -->` and `<!--\nline one\nline two\n-->` exactly.
- `rewrite_note_replaces_first_span_and_removes_later_whole_lines` asserts the
  first comment's position is retained, later comments and their line endings
  disappear including indentation on comment-only lines, and the JSON
  page-settings comment remains byte-identical.
- `rewrite_note_empty_removes_every_note_span` asserts no replacement is
  inserted.
- `rewrite_note_appends_after_last_nonblank_line` asserts this exact result:

  ```rust
  assert_eq!(rewritten, "# Title\n\n<!-- new note -->\n");
  ```

- `rewrite_note_preserves_crlf_and_is_idempotent` reparses the first result
  through the existing `crate::parser::parse_markdown` to obtain fresh spans
  and asserts `rewrite_note(&once, slide, notes, text) == once` with no inserted
  bare `\n`.
- `rewrite_note_rejects_comment_close_and_page_settings_prefix` asserts
  `ErrorKind::Parse`, the target comment/slide line, and non-empty `help` for
  both `-->` and trimmed text beginning with `{`.
- `rewrite_note_reparse_preserves_slides_keys_settings_and_other_bytes` asserts
  the postcondition:

  ```rust
  assert_eq!(after.parsed_slides().iter().map(|s| &s.key).collect::<Vec<_>>(),
             before.parsed_slides().iter().map(|s| &s.key).collect::<Vec<_>>());
  assert_eq!(after.parsed_slides()[1].notes.as_deref(), Some("edited"));
  assert_eq!(after.parsed_slides()[0].layout_request,
             before.parsed_slides()[0].layout_request);
  assert_eq!(after.parsed_slides()[2].skip, before.parsed_slides()[2].skip);
  assert!(rewritten.starts_with(&source[..target.source_span.start]));
  assert!(rewritten.ends_with(&source[target.source_span.end..]));
  ```

  Compare `key_source`, `layout_request`, `skip`, `page_number_hidden`, and
  `DeckSettings::sections()` for every reparsed slide/deck so every page-setting
  projection is covered; draft filtering and slide count must also match.

**Implementation.** Publish `notes_edit` from `lib.rs` and implement the fixed
signature:

```rust
pub fn rewrite_note(
    source: &str,
    slide: SourceSpan,
    notes: &[SourceSpan],
    text: &str,
) -> Result<String>;
```

Trim `text` with the same `str::trim` semantics as
`extract_html_comment_body`. Reject any text containing `-->` with message
`speaker note cannot contain '-->'` and help
`remove or rewrite '-->' because it closes the HTML comment`. Reject a trimmed
value beginning with `{` with message `speaker note cannot start with '{'` and
help `start the note with another character so it is not parsed as page settings`.
Use the first note's line, or the slide's first line when there is no note, on
both `BuildError`s.

Choose `\r\n` for inserted lines when the file contains `\r\n`, otherwise
choose `\n` (revised in Task 6: the replaced or preceding line's terminator
wins, then the slide's bytes); normalize internal `\r\n` and lone `\r` to `\n`
before joining submitted lines with that choice. Empty trimmed text removes all
note spans. One logical line becomes `<!-- text -->`; two or more become
`<!--`, the text, and `-->` on separate lines. Detect whether each span is the
only non-whitespace content across its source line or lines. For a whole-line
replacement, preserve whitespace before `span.start` but consume trailing
whitespace and the existing terminator; for a whole-line removal, consume from
the first line's start through the last line's terminator so an indented
removed comment cannot concatenate two surrounding lines. Treat a terminator
already contained in an `HtmlBlock` span as consumed. A replacement re-emits
one matching terminator only when one was consumed, while a removal emits none;
a comment ending at EOF does not gain a terminator. Inline spans replace or
remove only their own bytes. Apply edits from the last span backward so offsets
remain valid, replacing only the first span and deleting all later spans. With
no span, trim only trailing blank lines inside `slide`, preserve the last
non-blank line byte-for-byte, and append exactly one blank line, the canonical
comment, and one final line ending. Use the existing `parser::line_for_offset`
for error lines.

**Revision (2026-09-12, PR for this task).** Review measured that in-place
replacement of inline, indented, or blockquote comments, and whole-line
removal next to a non-blank line, produced files that no longer parsed back to
the same deck (setext merge across `---`, unterminated `<!--` inside
containers, notes leaking into slide content, comments appended inside an
unclosed fence). The implemented rules therefore differ from the text above:
the signature takes `highlighter: &Highlighter`; only a column-0 whole-line
first span is replaced in place and every other span is removed with the
comment appended at the slide end; whole-line removal between two non-blank
lines leaves a blank line (or a bare `>`); the design postcondition is
enforced by re-parsing inside `rewrite_note` and failing with a third
`ErrorKind::Parse` error (`speaker note cannot be written at this position`);
a leading BOM is stripped and restored inside the function; and spans that
do not match the source (out of range, unsorted, overlapping, not on a char
boundary, or not an HTML comment) are a fourth `ErrorKind::Parse` error
instead of a panic. Because `rewrite_note` re-parses the text it is given,
Task 6 must call it on the combined source with the `ParsedSlide`'s own
spans and translate only `source_span` to the origin file (an origin file
parsed alone lacks the top deck's frontmatter and still carries `include`
comments, so the origin-text route in its step 4 would reject every save on
a deck with includes); Task 3's note-span translation is therefore unused by
Task 6. Task 6 maps all four `ErrorKind::Parse` errors to `Unprocessable`.
The design record carries the revised rules.

**Verification.**

```sh
cargo test -p peitho-core notes_edit::tests
```

### Task 3: Translate combined byte spans to one origin file

**Goal.** Map parser spans from include-expanded Markdown to origin-file line
and byte-column coordinates, convert those coordinates against a separately
read origin, and refuse every non-representable span.

**Files.**

- `crates/peitho-core/src/include.rs`

**Test.** Add
`include::tests::translate_span_maps_plain_and_included_coordinates`,
`include::tests::origin_span_to_range_maps_utf8_and_crlf`, and
`include::tests::translate_span_refuses_generated_boundary_bytes`:

```rust
let combined_start = expanded.source.find("<!-- shared note -->").unwrap();
let origin_start = included_source.find("<!-- shared note -->").unwrap();
let translated = expanded.line_map.translate_span(
    &expanded.source,
    SourceSpan { start: combined_start, end: combined_start + "<!-- shared note -->".len() },
).unwrap();
assert_eq!(translated.file, included);
assert_eq!(translated.start, LineCol { line: 3, byte_col: 0 });
assert_eq!(translated.end, LineCol { line: 3, byte_col: 20 });
assert_eq!(
    origin_span_to_range(&included_source, &translated),
    Some(origin_start..origin_start + "<!-- shared note -->".len()),
);
let generated_newline = expanded.source.find("# Included").unwrap() + "# Included".len();
assert_eq!(expanded.line_map.translate_span(
    &expanded.source,
    SourceSpan { start: generated_newline, end: generated_newline + 1 },
), None);
```

Use an included fixture whose note starts on line 3 for the shown coordinate
assertions. The first two tests also cover an identity-mapped plain deck, a
multi-line span, UTF-8 byte columns, CRLF, and a missing line or byte column.
The refusal test separately intersects a `SyntheticTerminator`, a
`SyntheticLine`, and two different `Source` files.

**Implementation.** Add these public coordinate types, method, and conversion
function:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineCol {
    pub line: usize,
    pub byte_col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginSpan {
    pub file: PathBuf,
    pub combined: SourceSpan, // added in the revision below
    pub start: LineCol,
    pub end: LineCol,
}

impl LineMap {
    pub fn translate_span(
        &self,
        combined_source: &str,
        span: SourceSpan,
    ) -> Option<OriginSpan>;
}

pub fn origin_span_to_range(
    origin_source: &str,
    span: &OriginSpan,
) -> Option<Range<usize>>;
```

`LineCol::line` is 1-based and `LineCol::byte_col` is a zero-based byte column.
Treat spans and returned ranges as half-open.
`translate_span` walks `combined_source` and `origins` together so original
line bytes belong to `Source`, a generated terminating newline belongs to
`SyntheticTerminator`, and a generated blank-line newline belongs to
`SyntheticLine`. Require every intersected byte to map to `Source` entries for
one file with consecutive original line numbers. Return `None` for an invalid
boundary, empty/out-of-bounds span, either synthetic variant, mixed files, or
non-consecutive lines. For `LineMap::for_source`, return coordinates in the
source file.

`origin_span_to_range` indexes `origin_source` by the two line/byte-column
coordinates and returns `None` when either line or byte column does not exist.
It otherwise returns the exact byte range, including for UTF-8 and CRLF text.

**Revision (2026-09-12, PR for this task).** Review measured that
`ParsedSlide.source_span` ends on a `SyntheticLine` byte for the last slide of
an included file followed by `---`, and starts on a `SyntheticTerminator` for
a first slide after frontmatter that is an include, so the literal
"every intersected byte is `Source`" rule refused the primary include use
case. `translate_span` therefore clips synthetic bytes off both ends and
reports the clipped sub-span as `OriginSpan::combined`; `None` remains for a
synthetic byte strictly inside, mixed files, non-consecutive lines, invalid
bounds, or nothing but synthetic bytes. `origin_span_to_range` also refuses
offsets off a char boundary. The same review found and fixed a pre-existing
expansion bug: with CRLF frontmatter the region start is `\r\n`, which the
leading-newline helper did not recognize, so an included first line was glued
onto the closing `---`. The design record carries the revised rules and the
splice formula Task 6 must use.

**Verification.**

```sh
cargo test -p peitho-core include::tests
```

### Task 4: Expose a parse-only public entry point

**Goal.** Let the CLI re-parse notes without making code-image commands,
Chrome, or network reachable.

**Files.**

- `crates/peitho-core/src/parser.rs`
- `crates/peitho-core/src/lib.rs`
- `crates/peitho-core/tests/parse_deck.rs`

**Test.** Add integration test
`parse_deck_is_public_and_leaves_code_images_untransformed`:

```rust
let deck = peitho_core::parse_deck(source, frontmatter, &Highlighter::defaults()).unwrap();
assert!(matches!(deck.parsed_slides()[0].fragments[1].kind(), FragmentKind::Code));
assert_eq!(deck.parsed_slides()[0].note_spans.len(), 1);
```

Use a `code_images.dot` command in the fixture so the test proves the public
API returns the parsed code fragment without accepting or invoking any
renderer.

**Implementation.** Add
`pub fn parse_deck(source: &str, frontmatter: ParsedFrontmatter,
highlighter: &Highlighter) -> Result<Deck<Parsed>>` as a thin call to the
existing `parse_markdown`, keep `parse_markdown` `pub(crate)`, and re-export
`parse_deck` from `lib.rs`. Leave `parse_deck_and_transform` unchanged for the
normal build pipeline.

**Revision (2026-09-13, PR for this task).** Review found that a public
`Deck<Parsed>` produced without the transform is indistinguishable from the
transformed deck the public mapping chain accepts, so an outside caller could
render a deck whose declared renderers never ran. `parse_deck` therefore
returns `UntransformedDeck`, a wrapper exposing only `parsed_slides()` and
`settings()` with no way to reach `Deck<Parsed>` (pinned by a `compile_fail`
doctest). Task 6 uses `parsed_slides()` exactly as written and is otherwise
unaffected; `LoadedDeckSource::translate` is generic over the result type.

**Verification.**

```sh
cargo test -p peitho-core --test parse_deck
```

### Task 5: Add the capability-gated `POST /notes` route

**Goal.** Accept note saves only when a caller installs a writer and map every
specified outcome to its exact HTTP contract.

**Files.**

- `crates/peitho/src/server.rs`

**Test.** Add HTTP-level tests
`notes_route_saves_with_writer`, `notes_route_rejects_invalid_shapes`,
`notes_route_without_writer_returns_404`,
`notes_route_maps_conflict_to_409`,
`notes_route_maps_unprocessable_to_422`, and
`notes_route_maps_io_to_500`, plus
`write_atomic_appends_tmp_to_the_full_file_name`. The no-writer test sends
malformed JSON and still asserts 404. Representative assertions are:

```rust
assert_eq!(saved.status, 200);
assert_eq!(saved.body, r#"{"saved":true}"#);
assert_eq!(
    captured.lock().unwrap().as_ref()
        .map(|(key, text)| (key.as_str(), text.as_str())),
    Some(("intro", "new note")),
);
assert_eq!(unknown_field.status, 400);
let without_writer = http_request(&server_without_writer, "POST", "/notes", "{");
assert_eq!(without_writer.status, 404);
assert_eq!(conflict.status, 409);
assert_eq!(unprocessable.status, 422);
assert_eq!(io_failure.status, 500);
assert_eq!(serde_json::from_str::<Value>(&unprocessable.body).unwrap()["error"],
           "line 3: speaker note cannot contain '-->'\n  = help: remove or rewrite '-->' because it closes the HTML comment");
assert_eq!(atomic_tmp_path(Path::new("deck.md")), PathBuf::from("deck.md.tmp"));
assert_eq!(atomic_tmp_path(Path::new("rehearsal-X.json")),
           PathBuf::from("rehearsal-X.json.tmp"));
```

The tests cover this complete table:

| Outcome | Status | Body |
| --- | --- | --- |
| saved | 200 | `{"saved":true}` |
| invalid JSON / missing fields / unknown fields | 400 | text |
| no writer (present, or any non-preview server) | 404 | text |
| deck does not parse, key not found, span not editable | 409 | `{"error":"<message>"}` |
| text not representable (`-->`, leading `{`) | 422 | `{"error":"<message>"}` |
| I/O failure | 500 | `{"error":"<message>"}` |

**Implementation.** Directly below `PresentServer`, beside the existing
`RehearsalSink` placement, add:

```rust
pub type NotesWriter = Box<
    dyn Fn(&str, &str) -> Result<(), NotesWriteError> + Send + Sync + 'static
>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotesWriteError {
    Conflict(String),
    Unprocessable(String),
    Io(String),
}
```

Store it as `notes_writer: Option<Arc<NotesWriter>>` on the cloneable
`PresentServer`, initialize it to `None` in `bind_addr`, and add
`PresentServer::with_notes_writer(NotesWriter)`. Add a deny-unknown-fields
`NotesRequest { key: String, text: String }` for exactly
`{"key":"intro","text":"new note"}` in the success test, route only
`POST /notes` from `respond`, and implement `respond_notes_post` beside
`respond_rehearsal_post`. Check `notes_writer` first and return 404 before
reading or parsing the body when it is absent; only a server with a writer
parses the body and returns 400 for malformed input. Call the writer once, and
add `send_json_response_with_status(request, status, body)` so error strings
are serialized and escaped without changing their newlines. The string in
each 409/422/500 `error` field is the complete plain `BuildError` headline plus
help. Make the existing `write_atomic` a `pub fn`; `main.rs` is the package's
binary crate and cannot call a `pub(crate)` item in the separate `peitho`
library crate. Add private `atomic_tmp_path(&Path) -> PathBuf`: copy
`path.file_name()` into an `OsString`, push `.tmp`, and pass it to
`path.with_file_name`; have `write_atomic` use that path. Thus `deck.md` becomes
`deck.md.tmp`, while `rehearsal-X.json` still yields the byte-identical existing
staging path `rehearsal-X.json.tmp`. No present call site installs a writer, so
`peitho present` and every default server answer 404 for every body, including
malformed input.

**Revision (2026-09-13, PR for this task).** Review changed the server
contract in four ways, all recorded in the design record: the route requires
a `Content-Type` whose media type is `application/json` (400
`invalid notes content type` otherwise) so a cross-origin simple request from
another page cannot rewrite the deck without a CORS preflight, which the
server rejects; the writer is `Box<dyn FnMut(&SlideKey, &str) -> Result<(),
NotesWriteError> + Send>` stored as `Option<Arc<Mutex<NotesWriter>>>` and the
server holds the lock around the single call, so Task 6 needs no mutex of its
own; `NotesRequest.key` is a `SlideKey` (malformed keys are 400 before any
deck work); and `write_atomic` follows an existing symlink to its canonical
target, copies the target's permissions onto the staging file, and removes
the staging file when the rename fails. An `Io` failure is also logged to
stderr before the 500, as the rehearsal path does. A poisoned writer lock is
recovered rather than turned into a permanent 500. The review added
`notes_route_requires_json_content_type`,
`notes_route_serializes_concurrent_saves`,
`write_atomic_follows_symlinks_and_keeps_permissions`, and
`write_atomic_removes_tmp_when_rename_fails` to the test list above.

**Verification.**

```sh
cargo test -p peitho --lib notes_route_
cargo test -p peitho --lib write_atomic_appends_tmp_to_the_full_file_name
```

### Task 6: Wire an include-aware, serialized preview notes writer

**Goal.** Re-read the live files for every save, find by stable key, rewrite
the owning file atomically, and let the existing watcher observe the change.

**Files.**

- `crates/peitho/src/main.rs`

**Test.** Add
`tests::preview_notes_writer_reparses_and_rewrites_the_current_deck`,
`tests::preview_notes_writer_writes_the_include_origin`,
`tests::preview_notes_writer_preserves_a_leading_bom`, and
`tests::preview_notes_writer_does_not_run_code_image_commands`. Add
`tests::preview_notes_writer_uses_build_highlighter_resolution` with a
deck-adjacent `syntaxes/` definition and a fenced block using its token; the
save must parse and succeed. Add
`tests::preview_notes_writer_atomic_event_batch_rebuilds_once`; pass
`deck.md.tmp` and `deck.md` together to `handle_watch_paths_with_rebuild` and
assert its rebuild counter is `1`:

```rust
let writer = preview_notes_writer(deck.clone());
writer("shared", "edited in preview").unwrap();
assert_eq!(fs::read_to_string(&deck).unwrap(), top_source);
assert!(fs::read_to_string(&included).unwrap().contains("<!-- edited in preview -->"));
assert!(fs::read(&bom_deck).unwrap().starts_with(b"\xef\xbb\xbf"));
assert!(!sentinel.exists());
```

Add `tests::preview_notes_writer_classifies_conflict_unprocessable_and_io`
and assert a broken deck or vanished key is
`server::NotesWriteError::Conflict`, `-->` and leading `{` are
`server::NotesWriteError::Unprocessable`, and a top-deck read or origin write
failure is `server::NotesWriteError::Io`.

**Implementation.** Extract the existing `resolve_assets` plus
`load_highlighter` sequence from `build_artifacts` into this helper:

```rust
fn resolve_assets_and_highlighter(
    input: &Path,
    frontmatter: &peitho_core::ParsedFrontmatter,
) -> miette::Result<(ResolvedAssets, peitho_core::highlight::Highlighter)>;
```

Both `build_artifacts` and note saving use it, preserving the existing
explicit/deck-adjacent/built-in syntax resolution and sorted
`.sublime-syntax` loading.

Add these functions in `main.rs`:

```rust
fn write_preview_note(
    input: &Path,
    key: &SlideKey, // revised in Task 5: the route validates the key
    text: &str,
) -> Result<(), server::NotesWriteError>;
fn preview_notes_writer(input: PathBuf) -> server::NotesWriter;
```

The closure captures no parsed deck, source position, or content hash and
needs no lock of its own: the server holds `Mutex<NotesWriter>` around every
call (revised 2026-09-13 in Task 5), so the read/parse/rewrite/write
transaction is serialized by construction. The writer receives the key as a
validated `&SlideKey`; compare it with `slide.key == *key`. For each call:

1. Call `load_and_expand_deck_source(input)`, bind the combined string as
   `combined_source` (expansion has already stripped every leading BOM),
   resolve the shared highlighter, and
   pass the `peitho_core::parse_deck` result through
   `LoadedDeckSource::translate`. Do not call the transform entry point.
2. Find the `ParsedSlide` whose `slide.key == *key`; a miss is a 409
   conflict whose message contains the requested key.
3. Call `rewrite_note(&combined_source, slide.source_span, &slide.note_spans,
   text, &highlighter)` on the combined source with the `ParsedSlide`'s own
   spans (revised 2026-09-12 in Task 2: `rewrite_note` re-parses the text it
   is given, so it must see the whole deck — an origin file parsed alone
   lacks the top deck's frontmatter and still carries `include` comments).
   Bytes outside the slide span are unchanged by contract. If `rewritten ==
   combined_source`, return success without touching any file (a same-text
   save must not rewrite an origin whose slide carries trailing synthetic
   bytes).
4. Call `loaded.line_map.translate_span(&combined_source, slide.source_span)`
   for the slide span only (note spans need no translation) and bind the
   `OriginSpan` as `span`. On `None`, take `(origin_path, line)` from
   `loaded.line_map.translate(<combined line of slide.source_span.start>)`
   and return a conflict `BuildError` at that line with
   `origin_for_display(&origin_path, input)`, message `this slide cannot be
   edited from preview`, and help built as
   `format!("edit the note in {}", origin_path.display())`. On `Some`, bind
   `span.file` as `origin_path`, read it once as `origin_source` (raw bytes
   as read; `origin_span_to_range` skips a leading BOM itself), convert with
   `origin_span_to_range(&origin_source, &span)`, and require
   `origin_source[range] == combined_source[span.combined]`; a failed
   conversion or a mismatch (the file changed since it was expanded) is the
   same conflict.
5. Take the rewritten slide as
   `rewritten[span.combined.start..slide.source_span.end +
   rewritten.len() - combined_source.len()]` (add before subtracting; the
   rewrite may shrink the source; `combined.start` skips a leading synthetic
   byte, see the design record's mapping section revised in Task 3).
   `rewrite_note` picks its line ending from the replaced or preceding line
   (falling back to the slide's bytes), so only
   materialized synthetic bytes can be bare LF in a CRLF origin: when
   `origin_source` contains `\r\n` and no bare `\n`, convert bare `\n`
   to `\r\n`; otherwise leave the bytes as produced. If the result equals
   `origin_source[range]`, return success without writing. Splice them into
   `origin_source` at `range` (the BOM, if any, stays in place by
   construction) and call `server::write_atomic` in the origin directory.

Classify a report with the typed check
`report.downcast_ref::<DeckDiagnostic>().is_some()` as `Conflict`; do not match
report text. `DeckDiagnostic` is already `pub(crate)` in `diagnostics.rs` and
is reachable because `main.rs` declares `mod diagnostics`, so no visibility
change is needed. This covers frontmatter/include/parser/asset-definition
failures. Classify generic reports from reading the top deck or loading syntax
files as `Io`. Missing keys and refused spans are `Conflict`; map
`rewrite_note`'s four `ErrorKind::Parse` failures (`-->`, leading `{`,
postcondition, span mismatch) to `Unprocessable`; origin reads and
`server::write_atomic` failures are `Io`.
The classification test deletes the top deck after constructing the writer and
creates a directory at the `deck.md.tmp` path used by `write_atomic`, giving
deterministic read and write I/O failures without permission-dependent tests.

Format parse reports with `plain_diagnostic_text` after
`LoadedDeckSource::translate`. A rewrite error carries combined-source
line numbers: translate it through the combined `LineMap` like any other
parse report before formatting. Construct the missing-key conflict with message
`slide key '{key}' not found in current deck` and help
`reload preview and retry on a slide whose key still exists`; construct the
span conflict with the message/help fixed in step 4. Wrap origin read and write
errors in reports whose help says respectively `make the file readable and
retry` and `make the file and its directory writable and retry`, then flatten
them through `plain_diagnostic_text`.

Install `preview_notes_writer(options.input.clone())` with
`with_notes_writer` only in `preview`; leave `present` untouched.

Do not trigger a rebuild from the closure. `WatchTargets` already contains the
top deck and `LoadedDeckSource::included_files`. The `deck.md.tmp` staging path
does not match either `.md` root, the rename destination does, and the 200 ms
debouncer reduces that atomic-save batch to the one
`handle_watch_paths_with_rebuild` call that performs the normal rebuild and
generation bump.

**Verification.**

```sh
cargo test -p peitho --bin peitho preview_notes_writer_
cargo test -p peitho --bin peitho resolve_watch_targets_keeps_includes_and_assets_when_deck_parse_fails
```

### Task 7: Replace the note body with an autosaving textarea

**Goal.** Make the existing single-mode panel editable, derive dirty state from
the loaded notes map, and surface save failures without discarding the draft.

**Files.**

- `packages/peitho-present/src/preview.ts`
- `packages/peitho-present/test/preview.test.ts`

**Test.** Add
`renders_an_editable_notes_textarea_with_placeholder_and_status`,
`flushes_only_dirty_notes_on_blur`, and
`keeps_text_and_shows_the_server_error_when_a_save_fails`. Add
`resize_does_not_overwrite_a_dirty_note` to change the value, dispatch
`resize`, and assert the value is unchanged:

```ts
expect(note).toBeInstanceOf(HTMLTextAreaElement);
expect(note.placeholder).toBe("No notes for this slide.");
note.value = "edited";
note.dispatchEvent(new Event("blur"));
await vi.waitFor(() => expect(notesPosts()).toHaveLength(1));
expect(JSON.parse(notesPosts()[0][1].body as string)).toEqual({ key: "intro", text: "edited" });
resolveNotesPost(okJson({ saved: true }));
await vi.waitFor(() => expect(status.textContent).toBe(""));
note.dispatchEvent(new Event("blur"));
expect(notesPosts()).toHaveLength(1);

note.value = "bad -->";
note.dispatchEvent(new Event("blur"));
resolveNotesPost({
  ok: false,
  status: 422,
  text: async () => JSON.stringify({ error: serverError })
} as Response);
await vi.waitFor(() => expect(status.textContent).toBe(serverError));
expect(note.value).toBe("bad -->");
```

The success test also clears an existing note and asserts the local map entry
is removed, so a second blur is clean. Extend the fetch fixture with
`notesPosts()` to return recorded `/notes` calls and `resolveNotesPost()` to
settle a deferred save; Tasks 8 and 9 reuse those helpers.

**Implementation.** In `createNotesPanel`, replace the note `<div>` with one
reused `HTMLTextAreaElement`, keep `data-peitho-preview="note"`, and use
`placeholder` for `NO_NOTES_PLACEHOLDER`. Preserve the current font and
plaintext behavior; make the textarea fill the panel below the position row
with a transparent background, inherited color/font, no border, and
`resize: none`. Add a right-aligned red
`<span data-peitho-preview="status">` to the position row and no spinner.

Rename the controller field to `notesTextarea: HTMLTextAreaElement`, add
`notesStatus` and `notesPositionText`, and make the position row hold the
latter two spans so `renderNotes` does not remove the status node. Set the row
to `display: flex` with the status pushed right; set the panel to a column
flexbox and the textarea to `flex: 1`, `minHeight: 0`, and `width: 100%` so it
fills the remaining panel. Record the represented key in
`notesTextareaKey: string | null` and assign
`notesTextarea.value = notes.notes[key] ?? ""` only when that key changes;
`applyLayout` on resize must not overwrite an in-progress value. Dirty is
always computed as the exact expression
`notesTextarea.value !== (notes.notes[key] ?? "")`; do not add a dirty flag.
Implement `flushNotes(keepalive = false): Promise<boolean>` to snapshot the
current `{ key, text }`, return `true` for a clean value, and POST JSON to
`/notes` with `method: "POST"`, `Content-Type: application/json`, and
`JSON.stringify({ key, text })`. Pass `keepalive` through the fetch options. On
200, update `notes.notes[key]` to the submitted text or delete the entry when
the text is empty, clear the status, and return `true`. On a non-200 response,
read the body once, use its JSON `error` string when present and otherwise the
raw body; on a rejected fetch use the thrown error string. Put that value in
`notesStatus.textContent`, change neither the map nor textarea, and return
`false`. Add `onNotesBlur` as a stored listener that starts `flushNotes()`,
attach it to textarea `blur`, and remove it in `destroy`.

**Revision (2026-09-13, PR for this task).** Review found that overlapping blur
flushes could reach the server out of order (each `POST /notes` runs on its own
thread), so `flushNotes` snapshots `{key, text}` at enqueue time, queues onto a
private promise chain, and checks that snapshot against the notes map when its
turn comes; `flushNotes` is not part of the exported `PreviewShell` type; a
clean flush clears a stale status; the `is-empty` class is gone (the
placeholder supplies the look); the textarea carries `aria-label="Speaker
notes"` and no user-agent padding; the position text is `flex-shrink: 0` and
the status wraps (`pre-wrap`, `overflow-wrap: anywhere`) so a multi-line server
error shows as lines. The status is cleared by a clean or successful flush and
otherwise shows the last failure until the retry settles. Task 8 inherits two
notes: its transition gate needs no re-check for text typed before its flush
starts (the chain queues it), but text typed during the request still needs the
post-await re-check its `reflushes_text_typed_during_a_transition_save` test
requires; and it must add a test that navigating away and back during an
in-flight save does not revert the saved text from the stale map, plus a
`keepalive` fallback for bodies over Chrome's 64 KiB in-flight keepalive limit
on `pagehide`.

**Verification.**

```sh
cd packages/peitho-present && npm test -- preview.test.ts
```

### Task 8: Flush transitions and preserve a reload draft

**Goal.** Cover every autosave boundary and restore the exact editing caret and
focus after a watch-triggered reload.

**Files.**

- `packages/peitho-present/src/preview.ts`
- `packages/peitho-present/test/preview.test.ts`

**Test.** Add
`flushes_before_slide_and_grid_index_changes`,
`reflushes_text_typed_during_a_transition_save`,
`pagehide_saves_a_draft_and_posts_with_keepalive`, and
`restores_and_clears_a_focused_draft_with_selection`:

```ts
note.value = "in flight";
note.focus();
bus.dispatchEvent(new CustomEvent("peitho:navigate", { detail: { to: "next" } }));
expect(notesPosts()).toHaveLength(1);
expect(shell.currentIndex).toBe(0);
resolveNotesPost(okJson({ saved: true }));
await vi.waitFor(() => expect(shell.currentIndex).toBe(1));
expect(document.activeElement).toBe(note);

note.value = "reload draft";
note.setSelectionRange(3, 8);
window.dispatchEvent(new Event("pagehide"));
expect(notesPosts().at(-1)![1].keepalive).toBe(true);
expect(JSON.parse(sessionStorage.getItem("peitho:preview-state")!)).toMatchObject({
  draft: { key: "middle", text: "reload draft", selectionStart: 3, selectionEnd: 8, focused: true }
});
```

After remount, assert the value, `selectionStart`, `selectionEnd`, and focus are
restored; assert storage no longer has `draft`. Run the restore once when
`notes[key]` already equals the draft (clean, no POST on blur) and once when it
does not (dirty, POST on the next flush). A rejected transition save must leave
the index and textarea unchanged so retry remains possible. In the in-flight
test, change `note.value` after the first POST, resolve it, assert a second POST
carries the newer value, and resolve that POST before expecting the index to
change. In `flushes_before_slide_and_grid_index_changes`, drive a thumbnail
click, a `peitho:navigate` request, and grid activation separately and assert
each POST exists while the old index is still current.

**Implementation.** Extend `PreviewState` with
`draft?: { key: string; text: string; selectionStart: number;
selectionEnd: number; focused: boolean }`. Make `saveState` include the current
slide key, textarea value and selection, and whether the textarea is
`document.activeElement`; `installPreviewReload` already calls this method
before `location.reload()`.

Route index-changing mutations in `setIndex`, `enterGrid`, and `exitGrid`
through a new
`transitionAfterNotesFlush(changesIndex: boolean, commit: () => void): void`
method. Its clean branch invokes `commit` synchronously. Its dirty branch awaits
`flushNotes()` and rechecks the same derived dirty expression before committing;
if the author typed while the request was in flight, it saves the newer value
too. A failed flush never invokes `commit`, leaving the old index, text, focus,
and status available for retry. Replace the grid tile's current two-call
`setIndex(...); exitGrid()` path with `activateGridIndex(index)`, which changes
the selected/current index and mode in one guarded commit so the asynchronous
branch cannot exit using the old selection. Keep the textarea node mounted and
reuse it across slides, preserving focus after a successful PageUp or PageDown
transition. Keep `navigateToTarget` returning a boolean synchronously: return
`true` as soon as `resolveTarget` yields an index and start the guarded
transition, independent of the asynchronous flush outcome, so `onNavigate`
can call `preventDefault`. A failed flush leaves the index unchanged, but the
event remains handled. Existing clean navigation stays synchronous.

Add a stored `onPageHide` listener that calls `saveState()` and starts
`flushNotes(true)`; attach it to `pagehide` and remove it in `destroy`. During
`load`, after the restored index is selected and `applyLayout` has populated
the textarea, apply a draft
only when `draft.key` equals the current slide key. Restore its clamped
selection and, when `focused` is true, focus the textarea. Rewrite the stored
state without `draft` immediately after applying it. The value-versus-map dirty
expression then decides whether the landed rebuild made the draft clean or it
still needs a retry. Extend `readState` to retain `draft` only when `key` and
`text` are strings, both selection offsets are finite non-negative numbers,
and `focused` is boolean; malformed draft data is ignored without discarding a
valid `mode` and `index`. `saveState` uses `notesTextareaKey`, not a grid
selection whose note has never been rendered.

**Revision (2026-09-13, PR for this task).** Review replaced the
`transitionAfterNotesFlush(changesIndex, commit)` seam and `activateGridIndex`
with one `commitTransition(index, mode)` method that owns every state mutation;
`setIndex`, `enterGrid`, `exitGrid`, and the grid tile click all route through
it. The gate predicate is "the textarea's key would change, or the panel would
be hidden by entering grid mode", so grid selection moves are ungated, exiting
grid to another slide flushes, and a dirty note never enters grid with a
failed save hidden behind the panel (a failed flush keeps single mode with the
error visible). A note counts as settled only when no flush is in flight and the
value equals the map (`notesSettled()`, backed by a `flushesInFlight`
counter); the synchronous gate, the post-await loop, and `saveState` all use
that one predicate, because a value equal to a stale map during an in-flight
save is not clean and would otherwise commit or persist a text-less draft and
lose the queued write's effect. `destroy` bumps the transition sequence so
a pending commit never runs after teardown. `PreviewDraft.text` is optional and
present only when dirty at save time; the draft is stored only when dirty or
focused; focus is restored in single mode only. `pagehide` posts the current
snapshot directly with `keepalive` rather than queuing behind the chain, and
the keepalive size fallback counts encoded bytes. `isDirty(key, text)` and
`stateIndex()` are the single sources shared by the flush path, the gate, and
state save/restore. `commitTransition` returns early on a no-op (same index, selection, and
mode) before bumping the sequence, so a redundant `exit` request or a click
on the current thumbnail cannot cancel a pending gated navigation. Tests added:
`clean_draft_does_not_overwrite_freshly_loaded_notes`,
`destroy_cancels_a_pending_transition_commit`,
`transition_waits_for_every_queued_flush`,
`reload_state_keeps_text_while_a_save_is_in_flight`,
`a_no_op_transition_does_not_cancel_a_pending_one`,
`pagehide_before_load_keeps_the_stored_state` (`saveState` is a no-op until
`load` finished), `restores_an_unfocused_draft_without_focusing`,
`drops_a_draft_whose_key_is_gone`, a multibyte keepalive case, and `navigating_away_and_back_during_an_in_flight_save_does_not_revert_
it` now exercises the real hazard (value returned to the stale map value while
the first save is in flight).

**Verification.**

```sh
cd packages/peitho-present && npm test -- preview.test.ts
```

### Task 9: Restrict preview shortcuts while editing

**Goal.** Preserve normal text editing while retaining PageUp/PageDown slide
navigation and the two-step Escape behavior.

**Files.**

- `packages/peitho-present/src/preview.ts`
- `packages/peitho-present/test/preview.test.ts`

**Test.** Add
`preview_keyboard_only_dispatches_page_keys_from_editable_targets` and
`escape_in_the_notes_textarea_blurs_before_entering_grid`:

```ts
textarea.dispatchEvent(new KeyboardEvent("keydown", { key: "PageDown", bubbles: true, cancelable: true }));
expect(navigations).toEqual([{ to: "next" }]);
for (const key of ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "o", "Enter", "Escape"]) {
  textarea.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
}
expect(navigations).toEqual([{ to: "next" }]);

textarea.focus();
textarea.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
expect(document.activeElement).not.toBe(textarea);
expect(shell.mode).toBe("single");
window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
expect(shell.mode).toBe("grid");
```

Repeat the editable-target assertion with an `<input>` and a
`contenteditable="true"` element, and assert chord-modified PageUp/PageDown are
untouched.

**Implementation.** In `installPreviewKeyboard`, keep `hasChordModifier` as
the first gate. Detect textarea, input, select, and contenteditable targets.
For an editable target, dispatch `peitho:navigate` with `prev` only for PageUp
and `next` only for PageDown, call `preventDefault`, and return without handling
every other key. Do not mutate preview state in the keyboard installer. Add the
textarea's own `keydown` listener in `PreviewShellController`: unmodified
Escape calls `preventDefault()` and `blur()`, whose existing blur listener
flushes the note. Because the global editable-target branch ignores Escape,
the first press cannot dispatch `peitho:overviewrequest`; the next press after
blur follows the existing shell shortcut and enters grid mode. Store this
listener as `onNotesKeyDown` and remove it in `destroy`.

**Revision (2026-09-13, PR for this task).** Review added an IME guard:
both the window installer and the textarea's Escape handler return early when
`event.isComposing` is true, because Chrome fires `keydown` during a Japanese
conversion and a blur there would commit the candidate and flush it. Editable
detection reads `composedPath()[0]` rather than `event.target` so an editable
inside a slide's shadow root is not retargeted to the host. Shift+PageUp /
Shift+PageDown inside an editable target are left to the browser so they
extend the selection by a page instead of navigating. The installer ignores
auto-repeated Escape so a held key cannot blur and enter grid in one press
(extended to Enter by the Issue #500 follow-up below).
Page keys from an editable target are `preventDefault`ed only when the shell
accepted the request (boundary presses keep the textarea's own scroll), and
`keyCode 229` counts as composing for Safari's post-`compositionend` Escape.
The editable branch shares the navigate-dispatch path with the rest of the
installer instead of duplicating it. Tests added:
`composing_keys_are_ignored_in_the_notes_textarea` and
`editable_page_navigation_is_prevented_only_when_accepted`.

**Verification.**

```sh
cd packages/peitho-present && npm test -- preview.test.ts
```

### Task 10: Rebuild the embedded preview bundle

**Goal.** Commit the generated shell bytes consumed by
`BUILTIN_PREVIEW_JS`.

**Files.**

- `packages/peitho-present/dist/preview.js`

**Test.** Use the existing
`tests::builtin_preview_shell_matches_committed_bundle` assertion:

```rust
assert_eq!(BUILTIN_PREVIEW_JS, committed);
```

**Implementation.** Run the package build once after Tasks 7-9 and retain the
generated `dist/preview.js`. No hand edit is permitted in the bundle.

**Verification.**

```sh
(cd packages/peitho-present && npm run build)
git diff --exit-code packages/peitho-present/dist/preview.js
cargo test -p peitho --bin peitho builtin_preview_shell_matches_committed_bundle
```

### Task 11: Document the editing contract

**Goal.** Explain autosave, keyboard behavior, canonical comment write-back,
and include ownership at both user-facing documentation levels and in the
project invariant summary.

**Files.**

- `README.md`
- `site/content/guide/cli.md`
- `CLAUDE.md`

**Test.** Define documentation acceptance check
`preview_notes_edit_documentation_contract` with these assertions:

```sh
rg -q 'autosaves on blur, slide changes, and page exit' README.md
rg -q 'PageUp.*PageDown' README.md
rg -q 'note comments collapse into one' site/content/guide/cli.md
rg -q 'included file' site/content/guide/cli.md
rg -q 'editable.*textarea' CLAUDE.md
```

**Implementation.** In README's “Preview while you write,” change the notes
panel description to an editable textarea and state that it autosaves on blur,
slide changes, and page exit; state that PageUp/PageDown still navigate while
typing and Escape leaves the editor before a second Escape enters the grid. In
the CLI guide's `peitho preview` section, add the same workflow plus the source
shape: save replaces the first speaker-note comment, removes later note
comments and their line endings, appends one comment after the last non-blank
slide line when absent, removes comments for an empty note, preserves CRLF,
writes included slides to their include file, and rejects `-->` or a leading
`{` without losing the textarea draft. Explicitly document that several note
comments collapse into one. Extend the existing CLAUDE preview bullet with the
preview-only editable textarea, autosave/reparse/origin-write flow, draft
reload state, PageUp/PageDown exception, and Escape blur; retain the statement
that notes never enter `dist/`.

**Verification.**

```sh
make demo-site
git diff --check -- README.md site/content/guide/cli.md CLAUDE.md
```

### Task 12: Reviewer-run Chrome acceptance

**Goal.** Verify the server, filesystem watcher, reload state, textarea focus,
error display, and include write-back together in a real browser.

**Files.**

- `examples/peitho-tour/deck.md`
- `target/preview-notes-e2e/deck.md`
- `target/preview-notes-e2e/included.md`

**Test.** The acceptance case is `chrome_preview_notes_edit_acceptance`. Its
manual steps are the design's Verification steps: `peitho preview
examples/peitho-tour/deck.md`, type a note, click away → `deck.md` changes and
the preview reloads with the note; PageDown while typing keeps focus; a note
with `-->` shows the error and keeps the text; an included-slide deck writes to
the include file.

```text
1. Run `peitho preview examples/peitho-tour/deck.md` and open its Chrome window.
2. Type a note and click away; assert deck.md changes and preview reloads with the note.
3. While typing, press PageDown; assert the next slide opens and the textarea keeps focus.
4. Enter a note containing `-->`; assert the status shows the error and the textarea keeps the text.
5. Run the included-slide deck, edit its note, and assert included.md—not deck.md—changes.
```

**Implementation.** This is a reviewer-owned verification pass, not a Codex
run. The reviewer restores `examples/peitho-tour/deck.md` after the first check,
then creates these exact ignored fixtures; no E2E fixture or edited example is
committed. `target/preview-notes-e2e/deck.md` contains:

```markdown
<!-- {"include":"included.md"} -->
```

`target/preview-notes-e2e/included.md` contains:

```markdown
# Included

<!-- original note -->
```

**Verification.**

```sh
peitho preview examples/peitho-tour/deck.md
peitho preview target/preview-notes-e2e/deck.md
```

### Follow-up: Enter focuses the note (Issue #500, 2026-09-13)

**Goal.** Close the keyboard-only loop: grid → Enter → Enter → type →
PageDown → Escape → Escape.

**Files.** `packages/peitho-present/src/preview.ts`, `test/preview.test.ts`,
`dist/preview.js`, `README.md`, `site/content/guide/cli.md`, `CLAUDE.md`,
`docs/specs/2026-09-12-preview-notes-edit-design.md`.

**Implementation.** `activateSelection` branches on the mode: grid keeps
opening the selected slide; single focuses the textarea with the caret at the
end (author decision: caret at the end). The installer is unchanged except
that Enter, like Escape, ignores auto-repeat, and Enter on a focused link is
left to the browser. The textarea handler swallows the repeats of the
focusing Enter so a held key does not type newlines; entering grid blurs a
focused textarea inside `commitTransition`'s commit.

**Tests.** `enter_in_single_mode_focuses_the_notes_textarea_at_the_end`,
`enter_in_single_mode_focuses_an_empty_notes_textarea_at_zero`,
`entering_grid_blurs_a_focused_textarea`, repeat-Enter and link-Enter cases.

## Summary

<!-- derived-from #verified-existing-seams -->
<!-- derived-from #tasks -->

The implementation proceeds from Parsed-only provenance to a pure rewrite,
origin translation, parse-only CLI wiring, capability-gated HTTP, shell
autosave, generated bundle, documentation, and reviewer-run browser proof.

## Gates (all must pass before committing)

```
cargo test --workspace          # run 3 times in a row (past test-race incidents)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
git diff --exit-code bindings/  # contract drift
cd packages/peitho-present && npm run build && npm test && npm run typecheck
git diff --exit-code packages/peitho-present/dist/shell.js  # embedded shell drift (after npm run build)
git diff --exit-code packages/peitho-present/dist/preview.js  # embedded preview drift (after npm run build)
git diff --exit-code packages/peitho-present/dist/remote.js  # embedded remote drift (after npm run build)
```
