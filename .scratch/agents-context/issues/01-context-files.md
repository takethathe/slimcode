# 01: AGENTS.md context files — discovery (global + cwd + git root), scope-labelled rendering, frontend wiring

**What to build:** slimcode injects `AGENTS.md` context files into the system message (pi-style `<project_context>`), with the global home file and the project files (cwd itself + git repository root) each labelled `scope="global|project"` and a closing note that project requirements override global ones. CLI one-shot and TUI both consume the shared `slimcode-common::context_files` module via `ContextBuilder::with_context_files`.

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] `ContextScope` (`Global` / `Project`) + `ContextFile { path, content, scope }` in `slimcode-common::context_files`.
- [x] `load_context_files(home, cwd)`: global `<home>/AGENTS.md` first; project = cwd itself + git root (nearest ancestor holding a `.git` dir or `gitdir:` file marker, worktree/submodule compatible), git root before cwd; canonical-path dedup (cwd under home does not re-inject the global file); non-git ancestors are never loaded.
- [x] `format_context_files(&[ContextFile])`: `## Project context` markdown section (aligned with `## Skills` / `## Tools`) with one `<project_instructions path scope>` XML block per file (path XML-escaped; the XML wrapper isolates each file's content so inner headings/lists cannot clash with the outer markdown), opening sentence `project requirements override global requirements when they conflict.`; empty input renders `""` so the whole section is omitted when nothing can be injected.
- [x] `ContextBuilder::with_context_files(&[ContextFile])`: injected between the base system prompt and the `## Skills` index; resumed sessions (non-empty history) are not re-seeded, matching the skills-advertising semantics.
- [x] CLI wiring: `run()` loads the files, `run_once` passes them to the builder; TUI `Tui` holds them and both `submit_prompt` / `trigger_skill` add `.with_context_files`.
- [x] Tests: global+cwd order/scopes; git root + cwd both injected when cwd is a subdirectory; `gitdir:` file marker recognized; cwd == git root deduped; non-git ancestor AGENTS.md ignored; cwd-under-home dedup; empty; exact render format; path escaping; builder injection position.
- [x] Docs: `CONTEXT.md` (Context file entry), `docs/development.md`, `docs/explanation.md`, `docs/user-manual.md`, `.scratch/agents-context/spec.md`.
- [x] `cargo test` green; `cargo fmt --all`; clippy 0 error / 0 warning.
