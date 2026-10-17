# Compaction persists a keep boundary so the recent tail survives a resume

ADR-0020 D2 recorded a known cost: because compaction appends the `CompactSummary` checkpoint
*after* the recent-keep tail it preserved, and `load` dropped every record before the newest
checkpoint, the tail lived in the live session but vanished on resume — a reloaded session was
`[checkpoint, …post-checkpoint]` while the live one was `[checkpoint, …kept, …post-checkpoint]`,
so the model lost its immediate working context across a `/session` reload. This ADR removes that
asymmetry. We adopt the design pi reaches with `firstKeptEntryId`, but without message ids, which
ADR-0009 forbids: the checkpoint itself records how many recent messages compaction kept, and
`load` reconstructs the same order the live session holds.

## Decisions

### D1 — The keep boundary is a `kept_count` on the `CompactSummary`, not a message id

`AgentMessage::CompactSummary` gains `kept_count: usize`: the number of non-checkpoint messages
immediately before this checkpoint that compaction kept (the Kept tail). `compact_messages` writes
it as the length of the tail it carries, so the checkpoint and its `kept_count` are produced
together and cannot drift. A count of *how many to keep walking back* is pure offset semantics — it
adds no stable message id and no second ordering concept to the log, so ADR-0009 D1 ("records carry
no id/parentId, the history is linear") stands unchanged.

`kept_count` is `#[serde(default)]`, which makes the change purely additive in both directions: a
checkpoint written before this field parses with `kept_count = 0`, and an older reader ignores the
unknown field. `0` is exactly "drop everything before the boundary" — the pre-existing load
behaviour — so legacy logs reload byte-for-byte as before, with no migration and no version bump.

### D2 — `load` reconstructs `[checkpoint] ++ Kept tail ++ [after]`; `build()` stays a positional substitution

The trim in `SessionStore::load` replaces ADR-0020 D2's blind `drain(..index)`. After locating the
newest checkpoint it walks **back** collecting `kept_count` **non-checkpoint** messages — skipping
any older, superseded checkpoints that sit physically between the tail and the newest one — then
rebuilds the history as `[newest checkpoint] ++ [kept tail in order] ++ [everything appended
after]`. Pulling the checkpoint to the front (ahead of the tail it physically follows on disk) makes
the resumed history equal the live history, so `ContextBuilder::build()` keeps its
position-preserving injection contract (ADR-0020 D2) and the model reads
`[summary, …kept, …new]`. `build()` is not changed.

Because a `kept_count` of `0` collects no tail, the reconstruction degenerates to the old "drop
everything before the boundary" for any legacy checkpoint, so the two code paths are one.

### D3 — Trim before repair: a dropped record is never repaired nor counted

`load` now applies the boundary trim **before** the dangling-tool-batch repair pass, and the repair
runs over the surviving history only. Previously the repair was interleaved with the line-by-line
replay, so a dangling batch in the span the checkpoint replaced got a spurious in-memory
`Error: interrupted` result and inflated `LoadOutcome.repaired_tool_calls` for a record that the
boundary then dropped. With trim-first the repair count reflects only real repairs. This is sound
because the Kept tail never starts on a `tool` result — `compact_messages` advances the keep
boundary past tool results so a kept result keeps the assistant call it answers (ADR-0020 D4) — so
no tool batch straddles the boundary, and every batch in the surviving history has its assistant and
arrived results adjacent for the repair pass to pair deterministically.

## Considered Options

- **pi's `firstKeptEntryId` (reference the kept record by id)** — rejected: it needs stable message
  ids, which ADR-0009 D1 deliberately withholds. A count is the id-free equivalent.
- **Re-deriving the tail on load from the keep heuristic** — rejected: it couples the reader to a
  token-estimation policy that may change, and a drifted estimator would silently reshape old
  sessions. Persisting the decided boundary keeps the log's meaning fixed at write time.
- **Rewriting the log at compaction so the tail sits after the checkpoint** — rejected: it breaks
  ADR-0009's append-only invariant.
- **Leaving the asymmetry (accept the lost tail)** — rejected: this is the user-visible defect the
  ADR set out to fix; a resume silently dropping the immediate working context contradicts the
  "live, resumed and assembled histories all agree" goal ADR-0020 D2 already states.

## Consequences

- A resumed session is now larger than before (the kept tail returns). The footer's `ContextUsage`
  estimate and the auto-compaction trigger both read the restored history, so they now agree with
  the live session across a resume instead of under-counting it — a resumed session can sit closer
  to the threshold, which is the correct reflection of its real context.
- The `kept_count` must stay in sync with what `compact_messages` actually keeps; both are produced
  in one place, so a divergence is a local bug, not a data-format risk.
- Append-only logging and "records carry no id" both remain intact invariants.
