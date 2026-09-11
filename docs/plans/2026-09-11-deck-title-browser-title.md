# preview / present: use the deck title as the browser title

Date: 2026-09-11
Branch: `deck-title-browser`
Issue: #469

## Problem

`peitho preview` and `peitho present` always show the fixed `<title>` strings
`Peitho Preview` / `Peitho Present`. The distribution `index.html` written by
`peitho build` already sets `document.title` from `manifest.title` (the first
slide's title, `Untitled` as fallback), so the shells are inconsistent with
it, and with several decks open the tabs and windows cannot be told apart.

## Decision

- The browser title is the **deck title** (`manifest.title`), fixed for the
  session. It does not follow the current slide: preview is used side by side
  with other tabs, so a stable name is what lets the user find the deck, and
  per-slide updates would need a fallback rule for slides without a title.
- **preview**: the preview shell owns its page, so `PreviewShellController`
  sets `this.doc.title` right after the manifest loads.
- **present**: `PresentShellController` is a component, not a page — the
  presenter mounts two of them in its own document. Setting the title inside
  the component would rename the presenter window too. The title is therefore
  set by the `present.html` entry page after `mountPresentShell` resolves,
  exactly where the distribution index does it. `shell.manifest` is `null`
  when the load failed (the shell shows the error in its root), so the entry
  page guards on it and leaves the fallback `<title>` in place.
- **presenter** keeps `Peitho Presenter` so its role stays visible in window
  switchers; a presenter test pins `document.title` unchanged.

No pipeline or contract change: `manifest.title` already exists and rides
`bindings/Manifest.ts`.

## Verification

- vitest: preview title set from manifest; presenter title untouched.
- Rust: `render_present_index` output contains the guarded title assignment.
- Real browser (Chrome, Issue #469): preview and `present.html` show the deck
  title, `/presenter` shows `Peitho Presenter`.
