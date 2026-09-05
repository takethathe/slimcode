# TUI display alignment with pi

Status: ready-for-agent

## Problem Statement

The slimcode TUI works but looks nothing like the pi coding agent's terminal UI, which is the visual and interaction reference the user wants to match. Today the TUI renders a flat transcript of prefixed glyph lines (`── turn N ──`, `▶`, `✔`, `✓ done`), a hard-coded small palette, a single-line `cwd | session | model | ready` status line, per-turn magenta usage lines, bordered widgets with titles, a static `RUNNING` word, and no header, no scrollbar, no markdown coloring, no message backgrounds, and no animated working state. The user wants the TUI's display and interaction to align with pi's interaction model — semantic theme, boxed message/tool blocks, markdown rendering, a two-line footer with token stats, an animated spinner while working, a colored editor border, a startup header, and a transcript scrollbar — without changing the CLI frontend or porting pi's layout engine.

## Solution

From the user's perspective, `slimcode`'s TUI keeps its current interaction (multi-line prompt, `/` commands, completion popup, history recall, sessions, skills) but displays like pi: a startup header (bold accent `slimcode` + dim version + compact hints), user prompts as background-boxed messages, assistant text and thinking rendered as pi-style markdown (thinking italic gray), each tool call as one state-colored block (pending/success/error) with collapsed output expandable via Ctrl+O, no turn markers or "done" lines, a two-line dock footer (dim cwd/branch/session and dim token stats with the model right-aligned), an animated spinner row above the editor while a turn runs (Esc cancels it anywhere in the runner), a semantic editor-border color, a right-edge transcript scrollbar, and the terminal title set to `slimcode - <session> - <cwd basename>`. The one-shot CLI renders byte-identical output as before.

## User Stories

1. As a TUI user, I want a startup header showing the bold accent `slimcode` logo, the dim version, and one compact hint line, so that I know what I can do without opening `/help`.
2. As a TUI user, I want my submitted prompts rendered as padded background-boxed messages, so that prompts are visually distinct from assistant output like in pi.
3. As a TUI user, I want assistant messages rendered as markdown with pi's token colors (headings, links, inline code, code blocks, quotes, list bullets), so that answers are as readable as in pi.
4. As a TUI user, I want the model's reasoning rendered as italic gray markdown blocks, so that thinking is distinguishable without being loud.
5. As a TUI user, I want each tool call shown as one full-width block whose background reflects its state — pending, success, or error — with a compact per-tool call title (e.g. `read <path>:<line range>`, `grep /pattern/ in <path>`, `$ command`) instead of a raw JSON dump, followed by the gray plain-text output, so that I can scan tool activity at a glance.
6. As a TUI user, I want a tool block's output collapsed to the first 10 lines with a `… (N more lines, ... to expand)` hint, so that long tool results do not flood the transcript.
7. As a TUI user, I want Ctrl+O to expand and collapse tool output, so that I can inspect full results on demand (pi's `app.tools.expand`).
8. As a TUI user, I want streamed assistant text still merged into one flowing block, so that live streaming reads exactly as it does today.
9. As a TUI user, I want turn markers, the `✓ done` stop line, and the per-turn magenta usage line gone, so that the conversation reads like pi.
10. As a TUI user, I want a two-line Footer in the dock: a dim cwd/branch/session line, then a dim token-stats line (`↑in ↓out Rcache Wcache CH%`) with the model name right-aligned, so that location and usage context match pi.
11. As a TUI user, I want the git branch shown dim in parentheses next to the cwd when inside a repository, so that I see which branch I am on.
12. As a TUI user, I want a spinner row above the editor that animates while a turn runs (pi braille frames, 80ms, accent spinner + muted "Working..."), and is hidden when idle, so that I can see the agent is working without a static `RUNNING` word.
13. As a TUI user, I want the spinner to keep animating during long tool executions, so that a working turn never looks frozen.
14. As a TUI user, I want Ctrl+C / Ctrl+D during a turn to quit after the turn finishes, so that quitting stays predictable (unchanged behavior).
15. As a TUI user, I want the editor border to change color while a turn runs (blue at rest, cyan while running), so that I can see the editor's active state at a glance.
16. As a TUI user, I want the `/` completion popup restyled to pi's SelectList tokens — accent selected row with a `→ ` cursor, muted descriptions, and a muted `(i/n)` overflow indicator — so that the popup matches pi.
17. As a TUI user, I want a right-edge scrollbar on the transcript that appears while scrolling and has a `selectedBg` thumb, so that scroll position is visible like pi.
18. As a TUI user, I want the terminal title set to `slimcode - <session> - <cwd basename>` and updated on `/new` and `/load`, so that I can identify my terminal tab.
19. As a TUI user, I want `/help`, `/history`, `/sessions`, `/skills`, `/usage`, and install/replay confirmations still rendered in the transcript as dim notices, so that no command behavior is lost.
20. As a TUI user, I want failures still rendered in red, so that errors stay visible.
21. As a TUI user, I want terminal resize to keep the dock fixed and the transcript filling the remaining space above it, so that the layout never corrupts.
22. As a developer, I want the alignment implemented against the existing pure `App` core with ratatui `TestBackend` tests, so that rendering changes stay unit-tested without a real terminal.
23. As a developer, I want theme token resolution, markdown span mapping, and footer formatting as pure, unit-tested functions, so that styling regressions are caught at the leaf level.
24. As a one-shot CLI user, I want byte-identical CLI output, so that the alignment does not change scripting behavior.

### Revision 2 — display corrections + turn interruption (tickets 05–07)

The three items below revise stories 5/12/14 above; everything else stands.

R1. As a TUI user, I want each state-colored block's background to span the whole pane width (not only the cells holding text), so blocks read as solid pi-style bands (ticket 05).
R2. As a TUI user, I want the `read` tool to accept optional 1-indexed `offset` and `limit` arguments and every built-in tool block to open with a pi-style compact call title naming the tool and its key arguments — `read` shows its path and range (`read <path>:<start[-end]>`), `grep`/`find` their pattern and scope, `bash` the command line, `edit`/`write`/`ls` the target path — with the content below; unknown tools keep the bold-name + pretty-JSON fallback (ticket 06).
R3. As a TUI user, I want Esc while a turn runs to cancel it at any point — an in-flight LLM request, between tool calls, or a running `bash` child — returning to an idle input with whatever already streamed/applied kept, while Ctrl+C/Ctrl+D keep quit-after-turn semantics (ticket 07).

## Implementation Decisions

- **Semantic Theme layer**: the TUI gains a `Theme` implemented as named semantic tokens whose names and hex values mirror pi's `dark.json` (accent `#8abeb7`, border `#5f87ff`, borderAccent `#00d7ff`, muted `#808080`, dim `#666666`, userMessageBg `#343541`, toolPendingBg `#282832`, toolSuccessBg `#283228`, toolErrorBg `#3c2828`, selectedBg `#3a3a4a`, the markdown tokens, thinking/text tokens). Tokens resolve to ratatui styles at render time; dark theme only, no runtime switching.
- **Markdown rendering**: user and assistant message text renders markdown mapped onto pi's markdown tokens using the `pulldown-cmark` crate (added to the TUI crate only); events map to styled spans. Tool output stays plain gray text. Streaming re-parses the merged block per frame.
- **Block-aware transcript model**: the pure transcript model moves from flat per-kind glyph lines to block-aware entries — user prompt (boxed `userMessageBg` + markdown), assistant text (merged markdown stream), thinking (italic `thinkingText` markdown), tool block (paired start/result: **full-width** state background by ticket 05, per-tool compact call title by ticket 06, gray output, collapsed to 10 lines), dim notice, red error. Adjacent streamed text and reasoning fragments still merge.
- **Tool pairing and expansion**: a tool start opens a block; its result (same name, emitted sequentially by the shared runner) fills it and flips the background pending → success/error. A global Ctrl+O toggles expansion of all tool blocks (pi's `app.tools.expand`).
- **Two-region layout (transcript + dock)**: the screen splits into a scrollable transcript (header + messages + tool blocks + right-edge scrollbar with `selectedBg` thumb, auto mode: thumb appears on scroll) and a fixed dock ordered status indicator row, editor, completion popup, footer. Border titles on the transcript and input vanish; the editor border is colored (blue at rest, borderAccent cyan while a turn runs), and the placeholder stays.
- **Footer**: line 1 = dim `~/path (branch)` (the whole line is dim — pi's `footer.ts`
  wraps pwd, branch, and session in one `dim`; no separate gray branch) ` • session-id`;
  line 2 = dim `↑in ↓out Rcache Wcache CH%` with the model name right-aligned, both
  truncated to fit. Token counts use pi's `formatTokens` port (raw <1000, `x.xk` <10000,
  rounded `xk` <1M, `x.xM` below 10M, rounded `M` above). Context-percentage and cost
  are omitted (no data source); the ` ready|RUNNING` word is gone. Git branch comes
  from `git branch --show-current` best-effort at startup and session changes.
- **Status indicator + spinner animation**: a spinner row above the editor renders pi's braille frames at 80ms with an accent spinner and muted "Working..." (pi's ASCII default) while a turn runs; idle rows are hidden. To animate during a synchronous run, one turn's `run_turn` moves to a worker thread streaming `DisplayItem`s over an mpsc channel; the UI loop polls crossterm events and the channel with an 80ms timeout and redraws on each tick. This widens `Tool::run` to `Box<dyn Fn(...) + Send>`; all existing closures are already Send. While a turn runs, **Esc cancels it** (ticket 07: a `CancelToken` is checked at every agent-runner boundary and inside the provider's chunked body read; long tools like `bash` kill their child on cancel), Ctrl+C/Ctrl+D set a quit-after-turn flag, and every other key is ignored (input box not editable mid-run). The `App` stays a pure state machine — `tick()` advances the spinner frame and is unit-tested; the thread/channel lives in the thin terminal shell.
- **Revision-2 additions (tickets 05–07)**: (a) tool blocks pad every row to the pane width with the state background (pi `Box` paints the whole padded rect); (b) a pure per-tool call-title composer (`read <path>:<range>` with the range in warning when `offset`/`limit` are present, `ls`/`edit`/`write` path in accent, `grep`/`find` `pattern in path`, `bash` `$ command` — pi `format*Call` family) replaces the pretty-JSON args section for built-ins only; unknown tools keep bold-name + pretty JSON; `read` gains optional 1-indexed `offset`/`limit` engine slicing; (c) `Esc` cancels a running turn anywhere in the runner — `CancelToken` (agent crate, `Arc<AtomicBool>`, reset per turn) checked before/after provider calls and between tools, provider `chat` reads the SSE body chunk-wise and aborts when the token flips, `bash` runs via `spawn` + `try_wait` poll and kills its child on cancel, `StopReason::Cancelled` ends the turn silently with consistent partial history; `Provider::chat` and the runner entry points widen to carry the token; the CLI one-shot keeps the plain non-cancellable path.
- **Startup header**: the transcript's first block is the header — bold accent `slimcode` + dim ` v<version>` + one compact hint line (`·`-separated: Enter submit, Shift+Enter newline, ↑ history, /help, Ctrl+O expand). `/new` clears it with the transcript (unchanged `/new` semantics).
- **Completion popup**: keeps its bordered form and title (ADR-0005) but restyles to SelectList tokens — selected row `accent` with a `→ ` cursor, descriptions `muted`, overflow indicator `(i/n)` muted, empty state text.
- **Built-in test-coverage rule**: every feature lands with its tests in the same commit (no testless features); the thin shell keeps only raw crossterm drawing/polling, and every decision (quit-after-turn, spinner tick, title format, branch format) is a pure, tested function or `App` transition.
- **Terminal title**: the OSC 0 title `slimcode - <session> - <cwd basename>` (set on entering the alternate screen and on `/new` `/load`) is produced by a pure, unit-tested `terminal_title(session, cwd)` helper; the shell only executes it.
- **Removals**: turn markers, `✓ done`/`⚠ stopped` stop lines (abnormal stops render as red error text), the per-turn usage line (usage lives in the Footer), the single-line status bar with ` ready|RUNNING`, and the `transcript`/`input` border titles. `/usage` stays but renders a dim detail line.
- **CLI unchanged**: the one-shot frontend keeps its byte-identical streaming output; no glyph or color changes there.

## Testing Decisions

**Every shipped feature must be covered by at least one test.** The primary rule: any behavior that can be expressed as a pure function or a pure state-machine transition is extracted and tested there; the crossterm/ratatui shell shrinks to the truly I/O-bound five-liner, and even it gets its decision logic extracted for tests.

- A good test asserts external behavior: frame buffers out of the TUI pure core, styled-line output out of the theme/markdown/footer leaves, display-item streams and channel deliveries out of the runner/worker seam — never internal state.
- **Primary seam**: the pure `App` core drawn against ratatui `TestBackend`, driven by scripted key events (existing prior art in the TUI crate: `render_buffer`, `line_at`, `buffer_contains`, scripted key injection).
- **Leaf seams (new, deterministic)**: pure functions tested in their modules — theme token → style resolution; markdown → styled-spans mapping (per construct); footer/`formatTokens` formatting; the OSC 0 title format string; `App::tick()` spinner-frame advance; the quit-after-turn decision (pure App state).
- **Worker-thread seam**: the channel handshake is tested end-to-end with a scripted provider and a channel-recording renderer (the `FakeProvider` prior art from `slimcode-common::runner` tests), asserting the same ordered `DisplayItem` stream arrives over the channel; the `Tool::run + Send` bound is compiler- and test-verified at that seam.
- **Git-branch seam**: a best-effort wrapper whose formatting is unit-tested, plus an integration test in a temp `git init` repository (skipped when `git` is unavailable) asserting the branch string appears in the Footer.
- **tmux rendering verification**: a real-terminal integration seam runs the actual `slimcode` binary inside a detached tmux pane against a local mock SSE endpoint (`slimcode-ai`'s wire format, scripted chunks), then asserts on `tmux capture-pane` output — layout regions, blocks, footer lines, completion popup, scrollbar thumb, spinner frames advancing across captures, window resize keeping the dock fixed, and the OSC 0 pane title. These tests auto-skip when `tmux` is unavailable (probe `tmux -V`) and live in `crates/cli/tests/` (they need the built binary). TestBackend stays the exact-color seam; tmux is the real-terminal seam.
- Cases to cover (feature → test mapping is checked in verification ticket 04, plus `cargo llvm-cov` when installed): theme tokens map to the right hex; markdown headings/links/code/code blocks/quotes/lists/rules render with the right tokens and wrap; user prompts render as boxed messages; assistant text and thinking merge and render markdown/italic; tool blocks show pending/success/error backgrounds and collapse/expand (Ctrl+O); turn markers, done lines, and per-turn usage lines no longer appear; footer shows the two pi-style lines (with and without branch, truncation); the spinner row appears while running, advances on `tick()`, and disappears when idle — with the spinner's on-screen animation asserted by tmux captures; the editor border color reflects running state; the completion popup uses SelectList tokens; the transcript scrollbar renders its thumb; resize keeps the dock fixed; the channel worker delivers the full stream and its final result; quit-after-turn suppresses non-Ctrl+C keys and fires after the turn.

**Revision-2 additions (tickets 05–07):** tool-block rows reach the rightmost column with the state bg (frame-buffer assertion per row kind × state); the per-tool call-title composer is a pure leaf with per-tool unit tests and the `read` offset/limit engine slicing is pure-tested; cancel is tested at four seams — `CancelToken` unit, agent `run_loop` boundary checks (before chat, after chat error, between tools, parallel), the provider's `read_body_interruptibly(reader, cancel)` over an in-memory `Read`, and the cancellable `bash` child poll (kill-on-cancel, no 30s wait); the App's bare-Esc → `Effect::CancelRunning` transition and unchanged Ctrl+C/Ctrl+D are pure `handle_key_running` tests; tmux adds an Esc-during-slow-mock assertion (spinner disappears, input usable, partial text kept, next prompt runs).

## Out of Scope

- The one-shot CLI frontend and the `DisplayItem`/`run_turn` shared contract (tickets 05–07 only widen signatures behind the CLI's plain non-cancellable path; its bytes do not change).
- Cancelling a *restoring/loading* session turn or anything outside a freshly `reset()` run; mid-run *pause/resume*; interrupting between turns after completion.
- Light theme and runtime theme switching; images/Kitty; mermaid; diff rendering; selectors/overlays (model/session/theme pickers); OSC-133 prompt zones; external editor; Alt+Enter follow-up queue; `!` bash mode; file-path autocomplete; syntax highlighting grammars; mouse support; clipboard; search overlay; per-tool custom renderers supplied by extensions (the Revision-2 built-in call-title subset is in scope; arbitrary tool renderers are not).
- Porting pi's View/VStack/HStack/ScrollView layout engine; the TUI stays ratatui.
- Footer context-percentage and cost stats (no data source) and the `(auto)` compaction suffix (no auto-compaction).
- A per-read network timeout in the blocking provider (reqwest blocking exposes only a whole-request timeout): cancellation reacts within one socket chunk on a flowing stream; a silent server stays bounded by the 300s client timeout.

## Further Notes

- Decisions are recorded in ADR-0006 (`docs/adr/0006-tui-display-alignment-with-pi.md`).
- Glossary terms in `CONTEXT.md` used throughout: `Theme`, `Transcript`, `Dock`, `Footer`, `Status indicator` (plus existing `TUI`, `Renderer`, `DisplayItem`, `Completion popup`, `Frontend`).
- Docs to sync before commit: `docs/development.md`, `docs/user-manual.md`, `docs/explanation.md` where the TUI display, status line, or usage flow is described; `docs/index.md` already lists ADR-0006.
- The worker thread widens `Tool::run` to require `Send` — a one-word API change in `slimcode-agent`; existing call sites are unaffected.
- v1 TUI spec and tickets stay in `.scratch/tui/` as history; this effort lives in `.scratch/tui-pi-alignment/`.