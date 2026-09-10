# lint: report deliberate ellipsis truncation as a note, not an overflow warning

Issue: #462
Date: 2026-09-10
Branch: `issue-462-lint-ellipsis-truncation`

## Problem

`peitho lint` reports "content overflows the `body` slot horizontally" for a
slot the layout truncates on purpose with

```css
white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
```

`measureSlotOverflows` in `lint_measure.js` treats every clipping overflow
value (`hidden`/`clip`/`auto`/`scroll`) identically, so a one-line caption that
ends in "…" by design is indistinguishable from text that is cut off. In the
reporting deck this is 12 of 13 overflow warnings, burying the one real one.

## Decision

The issue offers two shapes: skip the finding, or report it under a separate,
lower-severity message. The second is taken. The #385 design record already
settled the same question for scrollable regions: intentional-but-lossy
clipping stays in scope with its own help text rather than being silently
skipped, because the content is still lost on a projected slide. An ellipsis is
also frequently a *safety net* on a title slot, where the author does want to
hear that the title got cut. Skipping would trade one blind spot for another.

Three lenses:

- **Root cause**: the seam is the measurement, which conflates "clipped" with
  "unintentionally clipped". Both shapes fix it there; neither is a per-consumer
  guard.
- **Type safety**: reporting carries the distinction as a typed payload field
  through to an exhaustive report; skipping discards it in JS.
- **Long-term**: reporting follows the recorded precedent and keeps the signal
  for the safety-net case; skipping is a carve-out.

No lens favors skipping, so this is not a weighted trade-off.

"Lower severity" is made concrete as a new `note:` line that does **not** count
toward the warning total or the exit code. The issue's purpose is that the real
overflow is no longer buried and can be filtered; a note that still flipped the
exit code to 1 would not be lower severity in any way that matters.

## Design

### 1. Measurement (`lint_measure.js`)

`text-overflow` applies to the block container whose `overflow` is not
`visible`, i.e. exactly the element `measureSlotOverflows` already inspects.
Horizontal hidden pixels on that element are classified as **truncation**
instead of overflow when *all* of the following hold:

1. `overflow-x` is `hidden` or `clip`;
2. computed `text-overflow` is anything other than `clip` (`ellipsis` or a
   custom string);
3. the element owns ellipsizable line boxes: its `display` is not
   `flex`/`inline-flex`/`grid`/`inline-grid`, and every descendant *element*
   is either `display: none` or exactly `display: inline` and not a replaced /
   atomic tag (`img`, `svg`, `video`, `audio`, `canvas`, `iframe`, `object`,
   `embed`, `input`, `select`, `textarea`, `button`, `math`). The check
   recurses so `<a><img></a>` is caught.

**Why condition 3 (revised twice during review, both by measurement).** The
browser draws the ellipsis only for text runs it can cut mid-run inside the
container's own line boxes. Two shapes get no "…" at all:

- A wrapper with `overflow: hidden; text-overflow: ellipsis` around a wide
  `<pre>`, `<p>`, `<table>` or nested `<div>` hard-clips (the same container
  with a `<span>` child shows `…`, with a `<pre>` child it does not).
- An **atomic inline** — `inline-block`, or a replaced element such as
  `<img>` — is only ever hidden whole; when nothing before it fits, Chrome
  hard-clips with no ellipsis. This matters for the shipped theme: `.slot-title`
  is `display: inline-block` (`themes/base.css`), so
  `h1 { overflow: hidden; text-overflow: ellipsis }` written against the default
  theme clips the title with no "…". A `.slot-body p { … ellipsis }` caption rule
  likewise clips an `![](x.png)` image (rendered as `<p><img>`) whole.

Classifying by the declared property alone would have demoted those genuine
losses from an exit-1 warning to a non-failing note — a false negative strictly
worse than the false positive this issue removes. The issue's own case survives
the stricter predicate: an author who sees "…" by design has the rule on the
element whose text is cut (`.slot-body p`, or a block title slot), and plain
text, `<a>`, `<code>`, `<strong>`, and `<br>` children all compute to
`display: inline` and still classify as truncation (measured).

### 1b. The slide-box check must not see through clipping ancestors

`contentBounds` expands the slide bounds with every descendant's
`getBoundingClientRect()`. Measured during review: an ancestor's
`overflow: hidden` does **not** clamp an *inline* descendant's rect (a `<a>`
inside a truncated `.slot-title` reported `right = 3895` while the slot itself
ended at `1216`), so a truncated caption that is a link — the issue's likely
real shape, a source line that is a page title — still failed lint with
`content overflows the slide box horizontally by 4065px` even after the note
landed. The #385 record's claim that clipped descendants' rects "never expand
the slide bounds" is true for block descendants that stay inside the slide and
false for inline ones.

Same root conflation, so the fix lives at the same kind of seam: `contentBounds`
still visits every descendant, but when a clipping element (`clipsOverflow` on
either axis) sits in the descendant's **containing-block chain** below the
slide, the descendant contributes only its **top and left** edges to the
bounds, not its right and bottom. The clipping element's own rect still
expands all four edges (a clipping element that itself escapes the slide is
still caught). Loss past the clipper's end/bottom edge is
`measureSlotOverflows`' job via `scrollWidth`/`scrollHeight`; loss past its
start/top edge is invisible to those metrics (scrollable overflow never extends
negative), so the slide-box check must keep seeing it.

**Why top/left edges are kept (revised in review round 5, measured).** The
first version skipped the whole rect. Against the unmodified theme, a
`.slot-body p { margin-left: -400px }` (or `margin-top: -200px`, or
`position: relative; left: -400px`) paragraph is genuinely cut on its start
side by `.body { overflow: hidden }` — the screenshot shows the first words
gone — and main reported `slide box horizontally by 328px` while the whole-rect
skip reported nothing. Keeping the start edges restores main's number exactly
(contentWidth 1280 → 1608) and cannot bring back the linked-title false
positive: overflowing inline content is start-aligned (CSS Text §7.1), so the
`<a>`'s left edge stays inside the slot and only its right edge, which is now
ignored, ran past the slide. Known ceiling: in a `direction: rtl` clipper the
covered side flips, so a truncated RTL title would again produce a slide-box
warning; no deck emits `dir`, so this is recorded rather than built for.

**Why the containing-block chain and not the subtree (revised in review round
4, measured).** The first version pruned the whole subtree of a clipping
element on the premise that nothing inside can paint outside it. That premise
is false for `position: absolute`/`fixed` descendants: CSS overflow clips only
descendants whose containing-block chain passes through the clipper. The
theme's `.body` wrapper is non-positioned, so an absolutely positioned element
inside it is positioned against the slide (`.peitho-slide` carries a
`transform`, making it the containing block), escapes `.body`, and is clipped
by the slide — exactly the loss the slide-box warning exists for. Measured with
a `left: -400px` badge inside `.body`: main reported `slide box horizontally by
400px`, the pruning version reported nothing (and `slide.scrollWidth` cannot
rescue it — scrollable overflow never extends to the top/left). The chain walk
follows `offsetParent` for absolutely positioned elements and `parentElement`
otherwise; `position: fixed` yields a null `offsetParent`, which counts the
rect (the conservative direction). Measured verdicts: the badge counts in
full, a static `<p>` in `.body` contributes only its start edges, an absolute
child of a `position: relative; overflow: hidden` wrapper likewise, and the
inline `<a>` in a truncated `.slot-title` likewise — so the linked-title false
positive this section was written for stays fixed.

Measured consequence: a deck whose `.body` clips 80 paragraphs used to report
the same loss twice — `overflows the slide box vertically by 7811px` **and**
``overflows the `body` slot vertically by 7931px`` — and now reports it once,
at the clipping slot. `lint_reports_slide_vertical_overflow` was the only
Chrome pin of the slide-box path and relied on that duplicate, so it now
overrides `.body { overflow: visible }` (content genuinely escapes the slide)
and asserts the slide-box warning directly; measured: `slide box vertically by
7931px`, exit 1, two warnings.

**Why condition 1.** `overflow-x: auto`/`scroll` regions stay warnings with
`SCROLLABLE_OVERFLOW_HELP` exactly as #385 decided, even when they also declare
`text-overflow`. The truncation classification is a narrowing of the
`hidden`/`clip` case only, so the scrollable decision is untouched.

The classification is computed **inside `consider`** from the axis, never
passed in by the caller, so the vertical branch cannot mark an entry truncated.
That is the single seam for the invariant "truncated ⇒ horizontal ∧
hidden/clip ∧ inline-owning container". A Rust-side enum was considered and
declined: the Rust type cannot constrain what the JS emitter writes, so it
would only reshape the receiver without making the broken state unreachable;
the invariant lives where the decision is made, and the block-child E2E test
pins it.

Truncation gets its **own worst-offender bucket** next to `horizontal` and
`vertical`. If it shared the horizontal bucket, a 300px deliberate ellipsis
would shadow a 5px genuine clip on the same slide and the real finding would
vanish — the same failure shape this issue exists to remove.

Payload: each `slotOverflows` entry gains `slotOverflowTruncated: boolean`. A
truncated entry keeps `slotOverflowAxis: "horizontal"` and its `slotOverflowValue`
so older readers of the payload still see a well-formed entry. The key is
pinned by the `lint_measure_script_emits_slot_overflow_payload_fields` marker
test in `render.rs` like its siblings, so a rename cannot silently revert every
ellipsis to a warning behind `#[serde(default)]`.

`white-space: nowrap` is deliberately **not** required. `text-overflow` also
ellipsizes an unbreakable token on a wrapping line, and the property alone is
what decides whether the browser draws an ellipsis. The issue's "or its nearest
block ancestor" clause is already covered: the element carrying the overflow is
the block container, and `slotNameFor` resolves the slot name from there.

### 2. Reporting (`lint.rs`)

`SlotOverflowMeasurement` gains `truncated: bool` (serde default `false`, rename
`slotOverflowTruncated`). The collector partitions entries above
`OVERFLOW_TOLERANCE_PX`: truncated entries become a `TruncationNote { slide,
overflow_px, slot }`, everything else stays a `SlotOverflowWarning`.
`OverflowValue::help` is untouched — truncation never reaches it.

Output, printed after the slot overflow warnings and before font-size warnings:

```
note: slide 3 text in the `body` slot is truncated with an ellipsis (42px hidden)
   help: the layout CSS truncates this text with text-overflow; shorten the text or widen the slot if the cut is unintended
```

Without a slot name: `note: slide 3 text in a container is truncated …`.

Notes are excluded from `warning_count`; a deck whose only findings are notes
prints `checked N slide(s): no warnings` and exits 0. The summary line format is
unchanged.

### 3. Docs

`site/content/guide/cli.md` lint section: one sentence that text truncated by
`text-overflow` is reported as a `note:` that does not affect the exit code.

## Tasks (TDD, one failing test before each production change)

1. `lint.rs` unit: payload with `"slotOverflowTruncated":true` deserializes;
   missing field defaults to `false` (extend the existing payload tests).
2. `lint.rs` unit: collector yields a `TruncationNote` for a truncated entry,
   a `SlotOverflowWarning` for a non-truncated one on the same slide, and
   applies the 1px tolerance to notes.
3. `lint.rs` unit: `write_lint_report` prints the note line and help, named and
   unnamed forms, exit 0 and `no warnings` when only notes exist, and a mixed
   deck counts only the warnings.
4. `lint_measure.js`: separate truncation bucket, `slotOverflowTruncated` on
   every emitted entry.
5. `tests/lint.rs` (Chrome-gated, `#[ignore]` like its siblings): a deck with a
   long `# title` and `.slot-title { display:block; line-height:1.2;
   white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }` produces
   the note, no `content overflows` line, and exits 0 (the `line-height`
   exists because the theme's `1.05` h1 line box clips 4px of descenders once
   `overflow: hidden` is on, which is a real vertical finding, not a fixture
   for this issue); a second deck with the same rule **plus** a genuinely
   clipped `.body` still reports the body overflow; a third deck with the
   ellipsis rule on a **wrapper** (`.body`) around a paragraph holding an
   unbreakable 320-character word still reports a horizontal `warning:` and
   no note — the block-child case (the child that matters is the block
   `div.slot-body`; a fenced code block would route to the `code` slot, not
   `.body`, in the default layout). Three more Chrome-gated cases from review:
   `h1 { nowrap; hidden; ellipsis }` against the unmodified theme (the
   `inline-block` `.slot-title`) is a horizontal `warning:` and no note; a
   `.slot-body p { … ellipsis }` paragraph whose only content is a long
   `inline-block` `<code>` token is a `warning:` and no note; and a linked
   title `# [long](https://…)` under the block `.slot-title` ellipsis rule is a
   note only, exit 0, with no "overflows the slide box" line.
6. `render.rs` marker test: pin `slotOverflowTruncated` in `LINT_MEASURE_JS`.
7. `site/content/guide/cli.md`.
8. `lint_measure.js` `contentBounds`: skip rects clipped inside a
   containing-block ancestor (§1b), plus a Chrome-gated test with a custom
   layout holding an absolutely positioned `left: -400px` badge inside
   `.body`, asserting the slide-box horizontal warning survives.

## Verification

Workspace gates plus a real Chrome run: `peitho lint` over every `examples/`
deck must produce no new note or warning (no shipped theme uses
`text-overflow`, so the delta must be zero), and the synthetic deck from task 5
must show the note.
