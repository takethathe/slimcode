# 03: List state prefactor (frame stack, no visible change)

**What to build:** A behaviour-preserving restructure of the markdown renderer's list
handling, so that ticket 04 (nesting depth, continuation alignment, in-item paragraphs)
becomes a small, local change. Today the renderer tracks a list with a single
"ordered? next number" slot, a single `in_item` flag and a single marker string, which
cannot represent nesting depth or more than one block per item. After this ticket the same
renderer tracks a **stack of open lists** (ordered-ness, next number, depth) plus the
**current item** (marker, indentation, continuation prefix), and every emitted row is built
through one prefix pipeline that composes the quote prefix with the item prefixes. Visible
output is **byte-identical**: bullets stay `• `, nesting stays flat, loose lists and
multi-paragraph items keep their current (defective) shapes — those are ticket 04's job.
This is the "make the change easy, then make the easy change" step.

**Blocked by:** 01 (same module; 01's gap-row helper defines the "no gap inside a list
item" carve-out this prefactor must preserve, and the prefactor's no-visible-change
baseline is 01's output)

**Status:** resolved

- [x] The list state becomes a stack of frames (per open list: ordered-ness, next number,
      nesting depth) plus a current-item frame (marker text, first-row prefix,
      continuation prefix), replacing the single list slot, the `in_item` flag and the
      single marker string. Nesting is representable but renders with zero indentation, so
      output does not change.
- [x] Every rendered row (block rows, quote rows, code-block rows, gap rows) is produced
      through one prefix pipeline that composes the quote prefix with the current item's
      first-row/continuation prefix, instead of the current ad-hoc per-call prefixing.
- [x] **No visible change:** the existing markdown tests pass unmodified (they pin the
      current `• ` bullets, flat nesting, quoted-prefix rows, and the code-block layout),
      and the transcript frame-buffer tests are unaffected. Nothing in the module's public
      surface changes: the markdown entry point keeps its signature and returns the same
      row type.
- [x] Characterization tests are added for the shapes this prefactor must carry over
      unchanged, each marked as a known-defect baseline for ticket 04: a nested list
      renders all levels in one column; a loose list renders its items adjacent with no
      blank row; an item holding two paragraphs renders them concatenated into one row.
- [x] Module comments describe the new state (frames + prefix pipeline) and name what
      ticket 04 changes on top of it; the no-gap-inside-a-list-item rule from ticket 01
      stays true and is enforced by the same pipeline.
- [x] `cargo fmt --all` applied, `cargo clippy --all-targets --all-features -D warnings`
      clean, `cargo test` green.
- [x] No documentation change is required (`docs/` describes visible behaviour, which this
      ticket does not alter); the spec's Testing Decisions still hold, and the ticket
      records the no-visible-change property for review.

## Notes

- Spec: `.scratch/tui-block-spacing/spec.md` — Implementation Decisions ("List state
  becomes a stack", "Paragraph flushing inside items" for what 04 then switches on),
  Testing Decisions (both seams pre-existing; no new seam).
- Ground truth for what 04 will need (pi v0.84.4, `pi-tui/dist/components/markdown.js`
  `renderList`): `indent = "    ".repeat(depth)`, `firstPrefix = indent + listBullet(marker)`,
  `continuationPrefix = indent + " ".repeat(visibleWidth(marker))`,
  `itemWidth = width - visibleWidth(firstPrefix)`, nested lists recursed at `depth + 1`.
  The pipeline introduced here is what makes those four quantities local to one place.
- Why a prefactor rather than doing it inside 04: the geometry change (04) touches marker
  glyphs, indentation, wrapping width, in-item paragraphs and loose-list gaps at once; with
  the frames in place first, 04's diff is geometry only and the "before" behaviour is
  pinned by tests instead of by memory.
- Review hook: `git diff` on this ticket should show no change to any rendered row; any
  test expectation touched here (other than the new characterization tests) means the
  prefactor leaked behaviour and must be reverted, not re-baselined.
