# TUI display follows pi's interaction model (semantic theme, message blocks, dock layout)

slimcode's TUI display is realigned to match the [pi](https://github.com/earendil-works/pi)
coding-agent's terminal UI (`pi` v0.84.4, `packages/coding-agent` interactive mode).
"Alignment" means **visual + interaction parity for the displayed states**, not a port of
pi's View/VStack/HStack/ScrollView engine: slimcode keeps ratatui + crossterm and its pure
`App` core, and only the rendering, layout, and display-state decisions adopt pi's model.

## Decisions

### D1 — Semantic theme tokens (pi `dark.json`)

The TUI colors move from hard-coded ratatui colors to a semantic token layer whose names
and hex values mirror pi's `dark.json` (accent `#8abeb7`, border `#5f87ff`, borderAccent
`#00d7ff`, muted `#808080`, dim `#666666`, userMessageBg `#343541`, toolPendingBg
`#282832`, toolSuccessBg `#283228`, toolErrorBg `#3c2828`, selectedBg `#3a3a4a`, the
markdown tokens, etc.). Tokens keep pi's names so the palette stays comparable and future
theme files can reuse them. Dark theme only; pi's light theme and runtime switching are out
of scope.

### D2 — Message block model

The transcript stops being flat glyph-prefixed lines and renders pi-style blocks:

- **User prompt** — a padded background box (`userMessageBg`) with markdown text.
- **Assistant text** — markdown on the default background.
- **Thinking** — italic `thinkingText` markdown block (always shown; no hide toggle).
- **Tool call** — one block per tool: background by state (pending `toolPendingBg` →
  success `toolSuccessBg` / error `toolErrorBg`) painted across the **full pane
  width** — every row (title, output, expand hint) is padded to the width so
  the colored region is one continuous band, exactly like pi's `Box` whose `bgFn`
  fills the padded rect. The header is a **compact per-tool call title**
  (ticket 06, pi `format*Call` family): `read <accent path><warning :start[-end]>`
  (range only when `offset`/`limit` are present), `ls <accent path>` with an
  optional muted ` (limit N)`, `grep /pattern/ in <scope>`, `find <pattern> in
  <scope>`, `edit`/`write <path>`, and `$ command` for bash (bold); paths and
  patterns are `accent`, line ranges `warning`, secondary info `toolOutput`, and
  empty paths default to `.` — rendered by the pure `toolcall` module. Built-ins
  show **no JSON args section**; unknown tools keep pi's fallback: bold name +
  pretty-printed JSON args. Below the title: gray plain-text output. Output is
  collapsed to the first 10 lines with
  `... (N more lines, Ctrl+O to expand)` (muted prefix, dim key hint, muted suffix,
  matching pi's `tool-execution.ts` hint which renders `keyHint("app.tools.expand",
  "to expand")`) and toggled with Ctrl+O, matching pi's `app.tools.expand` (ctrl+o).
- **Frontend notices** — dim plain lines (was Gray+Bold); **errors** stay red.

ToolStart/ToolResult pairs merge into a single block (the shared runner emits them
sequentially per tool). Turn markers, per-turn usage lines, and the `✓ done` stop marker
are removed — pi shows none of these; usage moves to the footer (D5), and abnormal stops
render as red error text.

### D3 — Markdown rendering

User and assistant messages render markdown with pi's markdown tokens (heading
`#f0c674`, link `#81a2be`, code `accent`, codeBlock `green`, quote/hrule `gray`, list
bullet `accent`). We use the `pulldown-cmark` crate and map its events to styled spans;
hand-rolling a CommonMark subset would trade a small dependency for a large, bug-prone
parser, and ADR-0003 already established that the interactive frontend may take
dependencies. Fenced and indented code blocks render as a ` ```lang` border line
(`mdCodeBlockBorder` gray), two-space-indented green lines, and a closing ` ``` `; the
fence is the code block's border, matching pi's `markdown.ts`. Tool output stays plain
gray text (as in pi).

### D4 — Layout regions (transcript + dock)

The screen becomes pi's two-region model:

- **Transcript** — the whole top area scrolls (header, messages, tool blocks), with a
  right-edge scrollbar (thumb = `selectedBg`; auto mode: appears on scroll, fades after
  ~1s).
- **Dock** — fixed bottom region, order: status indicator row, editor, completion popup,
  footer. The editor loses its ` input ` title and gets a semantic border color (blue at
  rest, `borderAccent` cyan while a turn runs). The completion popup keeps its bordered
  form (ADR-0005) but restyles to pi's SelectList tokens: selected row `accent` with a
  `→ ` cursor, descriptions `muted`, overflow indicator `(i/n)` muted.
- **Startup header** — at the top of the transcript: bold accent `slimcode` + dim
  ` v<version>` + one compact hint line (`·`-separated); full help stays under `/help`.

### D5 — Footer and status indicator

- **Footer** replaces the single status line: line 1 = dim `~/path (branch) • session-id`
  (the whole line is dim — pi's `footer.ts` wraps the pwd, branch, and session in a
  single `dim`; there is no separate gray branch); line 2 = dim stats
  `↑{input} ↓{output} R{cache} W{cacheWrite} CH{hit%}` with the model name
  right-aligned. Context-percentage and cost are omitted — slimcode has no
  context-window or pricing data source; the trailing ` ready|RUNNING` word is gone.
  Token counts use pi's `formatTokens` port (`raw <1000`, `x.xk <10000`, rounded `xk`
  `<1M`, `x.xM` below 10M, rounded `M` above — verified against `footer.ts`). Git
  branch comes from `git branch --show-current` best-effort at startup and session
  changes, omitted when git reports nothing.
- **Status indicator** — a spinner row above the editor while a turn runs: pi's braille
  frames (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`, 80ms), `accent` spinner + `muted` "Working..." (pi's
  ASCII default working message); hidden when idle.

### D6 — Spinner animation via a worker thread

The provider and runner are synchronous, so to animate the spinner (and later read keys)
during a run, one turn's `run_turn` moves to a worker thread that streams `DisplayItem`s
over an mpsc channel; the UI loop polls crossterm events and the channel with an 80ms
timeout, advancing the spinner (`App::tick`) and redrawing on each tick. This requires
widenings: `Tool::run` becomes `Box<dyn Fn(...) > + Send>` (one word; every existing
closure is already Send) and the concrete provider must be `Send` (it is:
`reqwest::blocking::Client` is Send). While a turn runs, bare **Esc cancels it** (ticket
07) and Ctrl+C/Ctrl+D set a "quit after turn" flag (previous behavior: buffered quit
after turn); other keys are ignored. The pure `App` core stays testable: `tick()`, the
running state, and the running-key decisions are plain state-machine behavior; the
thread/channel lives in the thin, untested terminal shell (ADR-0003 convention).

### D6a — Esc cancellation (ticket 07)

Cancellation is a shared `CancelToken` (`slimcode-agent`, an `Arc<AtomicBool>` cloneable
handle with `new`/`cancel`/`reset`/`is_cancelled`), reset at the start of every TUI turn
and cancelled on Esc. Every agent-runner boundary checks it — before a provider call,
right after a provider error (an error that coincides with a cancel is a silent
`StopReason::Cancelled`, not a propagated failure), after a request's deltas were streamed
(a cancel discards the half text: what streamed stays on the transcript, no assistant
message enters history), and before/after each tool dispatch, serial and parallel.
`Provider::chat` carries the token so an in-flight request interrupts itself: the
Bailian provider replaces the whole-body `resp.text()` with a chunked
`read_body_interruptibly` (checks the token between chunks, reports `Cancelled` with the
partial body, and the provider salvages whatever parseable deltas already arrived) — a
flowing stream reacts within one socket chunk, while a silent server stays bounded by the
300s client timeout (blocking reqwest exposes no per-read timeout). The only unbounded
tool, `bash`, runs cancellable variants (`bash_tool_with_cancel` /
`build_tools_with_cancel`, used by the TUI): the `sh -c` child is spawned in its own
process group, polled every ~50ms against the token, and the whole group is SIGKILLed on
cancel so no 30s command swallows Esc and no orphan survives. The CLI one-shot keeps the
plain tools and a token it never sets.

### D7 — Terminal title

The terminal title is set to `slimcode - <session> - <cwd-basename>` on entering the
alternate screen and on session changes (`/new`, `/load`), matching pi's OSC 0 title.

## Consequences

- The one-shot CLI frontend is **unchanged** — it keeps its byte-identical streaming
  output contract; alignment is TUI-only.
- Removed from the TUI display: turn markers, per-turn usage lines, `✓ done`, the
  ` ready|RUNNING` footer word, the ` input `/` transcript ` border titles.
- Known differences kept (documented, not fixed): no interrupt of a *restoring/loading*
  session or outside a freshly `reset()` run, no mid-run pause/resume, no light theme, no
  `!` bash mode, no follow-up
  queue (Alt+Enter), no external editor, no overlays/selectors, no images/mermaid, no
  OSC-133 prompt zones, no file-path autocomplete, no context-window/cost footer stats.
- Cross-crate API widenings for cancellation enter `slimcode-agent` (`Tool::run` gains
  `+ Send`; `Provider::chat` and every run entry point gain `&CancelToken`;
  `StopReason::Cancelled`); existing tool closures are unaffected at call sites and the
  CLI one-shot passes a never-set token.
- The markdown dependency (`pulldown-cmark`) is added to the TUI crate only.