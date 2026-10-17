# 02: `gui_applicable` registry + GUI host `complete` IPC

**What to build:** The GUI can ask for `/` completion candidates and get the right pool. The command
registry declares, per Command, whether it applies to the GUI, and the shared fuzzy, best-first
candidate assembly filters on that when serving the GUI while the TUI still sees everything. The Tauri
宿主 embeds a Skills snapshot and exposes a `complete` IPC so a partial `/` returns ranked Commands +
installed Skills, with the Commands that have no GUI backing (`/history`, `/!!`, `/!N`,
`/install-skill`) excluded. No UI yet — this is the verifiable data/API rung under the popup.

**Blocked by:** None (can start immediately; independent of ticket 01).

**Status:** ready-for-agent

- [ ] `slimcode-commands` `Command` gains a `gui_applicable` flag; the four history/install Commands are
      false, the rest true; registry tests updated.
- [ ] The GUI-caller candidate assembly (`complete()` producing `CompletionItem`s, incl. the
      `/skill:name` skill half) filters out non-`gui_applicable` Commands; the TUI caller is unchanged.
      Bare-name skill scoring, canonical values, and ADR-0005 ranking are preserved.
- [ ] The GUI controller embeds a Skills snapshot (mirroring the CLI completion provider) and exposes a
      `complete(input) -> Vec<CompletionItem>` IPC over the filtered shared source.
- [ ] Controller test: a leading `/` returns the GUI pool (four excluded) with skills as
      `/skill:name`; a `slimcode-commands` unit test asserts the TUI caller still sees all and ranking
      is unchanged.
- [ ] `cargo test` green; `architecture.rs` matrix passes (the `Command` shape changed); GUI crate's
      own gates pass; fmt + clippy clean.
