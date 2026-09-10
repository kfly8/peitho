# lint: let layout CSS lower the font-size floor for deliberately small text

Date: 2026-09-11
Branch: `lint-font-size-waiver`

## Problem

`peitho lint` warns for every slide whose smallest visible text renders below
24pt (`docs/specs/2026-08-01-lint-font-size-design.md`). Captions, source
lines, and footers are often small on purpose, and the deck author has no way
to say so. Every run of lint on such a deck exits 1 and buries the warnings
that matter, which is the same failure mode Issue #462 fixed for ellipsis
truncation.

## Decision

The waiver lives in the layout CSS as a custom property:

```css
.slot-caption { font-size: 14pt; --peitho-lint-min-font-size: 12pt; }
[data-slide-key="stats"] .slot-body { --peitho-lint-min-font-size: 16pt; }
```

Text whose computed `--peitho-lint-min-font-size` is set is checked against
that value instead of the 24pt default. Text at or above its own floor is
reported as a `note:` (visible, exit code unaffected); text below it is still a
warning that names the floor.

### Why CSS and not a page-settings key or frontmatter

- The font size is decided in the layout CSS, so the statement "this size is
  intentional" belongs next to it. The warning's own help text points at the
  layout CSS.
- Custom properties inherit, so the existing cascade gives every granularity
  for free: a slot, one slide via a keyed selector, or the whole deck via
  `.peitho-slide`. No new Markdown syntax, no pipeline field, no manifest
  change. This also follows the author's direction of keeping presentation
  concerns out of Markdown (Issue #364).
- A per-slide `{"lint":…}` page setting would waive a whole slide, so a title
  that accidentally shrank on the same slide would pass. A deck-wide
  frontmatter threshold cannot express "only the caption".
- Precedent: #462 downgraded ellipsis truncation to a note because the CSS
  itself expresses the intent. Same principle here; the waiver is never
  silent.

### Value grammar

A single CSS length in `pt` or `px` (`12pt`, `16px`), or `0` to accept any
size. Anything else (`none`, `small`, `1em`, `50%`) is a hard lint error that
names the slide and the value — relative units would need a second resolution
pass and "no silent path" applies, so the grammar is closed rather than
lenient.

## Design

### Measurement (`lint_measure.js`)

`measureTextFont` already walks visible text nodes and tracks the smallest
computed size. For each node it now reads
`getComputedStyle(parent).getPropertyValue("--peitho-lint-min-font-size")`:

- unset → the node feeds `minFontSizePx`/`minFontSample` exactly as today;
- set and parseable → the node goes to the waiver bucket with its threshold in
  px;
- set and unparseable → `fontSizeWaiverError` carries the raw value; the walk
  continues so one bad value does not hide the other measurements.

The waiver bucket keeps one representative per slide, `fontSizeWaiver:
{fontSizePx, sample, thresholdPx}`, chosen so a violation is never hidden by
an allowed node: the node with the most negative `fontSizePx - thresholdPx`
wins, and only when no node is below its floor does the smallest allowed node
win. Rust makes the final warning/note call on rounded pt, so the JS choice is
selection only.

Footnotes stay excluded from the walk as before.

### Reporting (`lint.rs`)

`SlideMeasurement` gains `font_size_waiver: Option<FontSizeWaiverMeasurement>`
and `font_size_waiver_error: Option<String>` (both serde-default, so old
payload shapes still parse). `run` rejects any measurement carrying an error
before reporting:

```
slide 3 has an invalid --peitho-lint-min-font-size value `none`
  help: use a length in pt or px, such as 12pt, or 0 to accept any size
```

`collect_font_size_warnings` returns the existing 24pt warnings plus waiver
warnings; a new `collect_font_size_notes` returns waived text. Both compare
`round_font_size_pt_for_display(size)` against
`round_font_size_pt_for_display(threshold)` so the message and the decision
agree.

```
warning: slide 3 has text at 10pt, below the layout's --peitho-lint-min-font-size of 12pt: "Source: …"
   help: raise the font size or lower --peitho-lint-min-font-size in the layout CSS
note: slide 3 has text at 14pt, allowed by --peitho-lint-min-font-size: 12pt: "Source: …"
   help: the layout CSS lowers the minimum for this text; remove the property if the small size is unintended
```

Notes print after the truncation notes and before the default font-size
warnings; only warnings count toward the summary and exit code.

## Files touched

- `crates/peitho-core/src/lint_measure.js` — property read, value parse,
  waiver bucket, payload fields
- `crates/peitho-core/src/render.rs` — script marker test
- `crates/peitho/src/lint.rs` — payload fields, error gate, collectors,
  report lines, tests
- `crates/peitho/tests/lint.rs` — Chrome E2E for note, waiver warning, and
  invalid value
- `site/content/guide/cli.md` — document the property
- `CLAUDE.md` — invariant bullet
