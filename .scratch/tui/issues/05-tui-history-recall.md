# 05: TUI input-history recall + history commands

**What to build:** Input-history recall inside the TUI pure core, per the spec's "Input-history recall keys" decision: ↑/↓ at the empty input box enter a distinct recall state, ↑/↓ move through stored prompts, and any other key exits recall back to editing. Plus the three history commands (`/history`, `/!!`, `/!N`) reusing `slimcode-common::history`.

**Blocked by:** 04 — slimcode-tui crate + pure App core

**Status:** resolved

- [x] History recall is a distinct `App` state (owned by the pure core): ↑/↓ at the empty input enters recall, ↑/↓ navigate entries (newest first), and any other key exits recall to editing. It is implemented so tui-textarea's own ↑/↓ cursor movement is not fighting the recall (spec decision).
- [x] Recalled entries load through `slimcode-common::history` (`HistoryStore`), surfaced as an effect the terminal loop fulfills so the core stays pure.
- [x] `/history` lists recent prompts (reusing the shared display/limit semantics — newest first, numbered, capped), `/!!` re-runs the most recent prompt, `/!N` re-runs the N-th listed prompt, all as frontend-owned transcript entries.
- [x] Re-run prompts submit as a fresh turn without being re-recorded in input history (same semantics as the REPL's replay path).

- [x] Pure-core tests with `TestBackend` + scripted keys cover: ↑/↓ enters and navigates recall, typing/other keys exit recall, `/history`/`/!!`/`/!N` behave as transcript entries, and replay does not double-record history.

- [x] The full workspace test suite passes; clippy is clean.

## Notes

- `Input history` is distinct from `message history` (per `CONTEXT.md`) — replay affects the session's message history only through the turn it runs, never the stored prompt list.
- The spec lists this as its own testable decision: "history recall enters/exits on ↑/↓ and exits on typing."
