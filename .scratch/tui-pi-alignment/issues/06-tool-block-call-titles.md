# 06: Per-tool compact tool-block titles (pi renderCall family)

**What to build:** A tool block's header today is a bold bare tool name, a blank line,
then the args as a pretty-printed JSON blob (`{ "path": "…" }` …), then the gray
output. The user's report: for `read`, the block should carry a title that names the
tool **and its key arguments** — `read <path>:<offset range>` — with the content below
it. Port pi's per-tool call-title family (verified against `tool-execution.ts` and the
`format*Call` helpers in `read.ts` / `ls.ts` / `grep.ts` / `find.ts` / `edit.ts` /
`write.ts` / `bash.ts`) for slimcode's seven built-in tools, and drop the separate
JSON-args section for them (unknown/non-built-in tools keep pi's fallback: bold name +
pretty JSON).

Because pi's `read` title carries `:<start>[-<end>]` only when `offset`/`limit` are
present, slimcode's `read` tool must gain those arguments (it currently accepts only
`path`): extend the tool **and** its engine with optional 1-indexed `offset` and
`limit` line slicing so model-generated range args are honored, described, and
rendered.

**Blocked by:** 05 — full-width backgrounds (new titles render on padded rows)

**Status:** resolved

- [x] **`read` offset/limit (agent tool, `crates/agent/src/tools/files.rs`)**: schema
      gains optional `offset` (1-indexed line to start) and `limit` (max lines); engine
      slices the file content accordingly (extract a pure, unit-tested
      `slice_lines(content, offset, limit)` — decision: 1-indexed, clamp/empty at EOF,
      `offset` without `limit` reads to the end, `limit` without `offset` reads from
      line 1); description updated; agent + common tool tests for slicing, defaults,
      and bounds. CLI one-shot behavior unaffected (its prompts may now yield range
      args — output bytes stay whatever the engine returns; golden CLI tests don't
      cover read internals).
- [x] **Pure per-tool title composer** (new `crates/tui/src/toolcall.rs` or similar):
      `tool_call_title(name, args_raw) -> Option<Vec<CallPart>>` where
      `CallPart { text, fg: Token, bold }`; parse the raw JSON arguments and produce
      pi's shapes:
      - `read`: bold `read` + ` ` + accent `<path>` + warning `:<start>`/`:<start-end>`
        when offset/limit set (mirrors `formatReadCall` + `formatReadLineRange`; omit
        range when neither arg present);
      - `ls`: bold `ls` + ` ` + accent `<path>` (empty path renders `.`) +
        toolOutput ` (limit N)` (slimcode ls has no limit — support the suffix only if
        args carry one, for forward-compat);
      - `grep`: bold `grep` + ` ` + accent `/pattern/` + toolOutput ` in <path|.>`;
      - `find`: bold `find` + ` ` + accent `<pattern|.>` + toolOutput ` in <path|.>`;
      - `edit`: bold `edit` + ` ` + accent `<path>`;
      - `write`: bold `write` + ` ` + accent `<path>`;
      - `bash`: bold `$ <command>` (whole call line bold text color, pi
        `formatShellCall`); empty command falls back to `…` gray;
      - anything else / unparseable args / missing required fields → `None` (caller
        uses the pi fallback: bold name + pretty JSON args, unchanged behavior).
      Unit tests per tool: composition, accent/gray/warning token choice, defaults
      (`.`/`/pattern/`/` in .`), absent args, invalid JSON, unknown tool name.
- [x] **Render**: `tool_rows` builds the header from the composer (title line, no
      blank+JSON section for built-ins), wraps it at pane width, then the gray output
      preview + collapse hint unchanged (ticket 05 padded rows). Keep the fallback
      path exactly as today's pretty-args layout.
- [x] **App frame-buffer tests updated + added**: existing `tool_block_*` tests that
      assert pretty-arg text (`"path": "a.txt"`, `tool_block_error_background_and_pretty_args`)
      are rewritten to assert compact titles (`read a.txt`, `edit a.txt`, `grep /x/ in
      .`…); new tests: read title with `offset`/`limit` shows `:<range>`, read without
      shows none; unknown tool still shows pretty JSON args; row-count/marker math
      tests re-run (title area shrinks from name+blank+JSON(N) to 1 wrapped line —
      re-check `total_lines` expectations).
- [x] **tmux smoke** (`crates/cli/tests/tui_smoke.rs`): the mock calls `ls` with
      `{"path":"."}` — the tool output (`marker.txt`) assertion stays valid; add an
      assertion that the block header shows `ls .` (or update any assertion that
      expected the pretty JSON shape; header text is visible to `capture-pane`).
- [x] Docs sync (AGENTS.md): spec US 5/6 wording (pretty JSON args → per-tool compact
      titles; the `… (N more lines, Ctrl+O to expand)` hint wording unchanged), ADR-0006
      D2 block description, the spec "Out of Scope" line that listed "per-tool custom
      renderers" (this lands a minimal built-in subset; extension renderers stay out),
      `docs/development.md` module list + tool block bullet, `docs/user-manual.md` tool
      bullet, `.scratch/tui-pi-alignment/test-map.md` new rows. `read` offset/limit
      description goes into the agent tool docs if any.

## Notes

- Ground truth (pi sources consulted): `read.ts` `formatReadCall`/`formatReadLineRange`
  (`read <accent path><warning :start[-end]>`), `ls.ts`/`find.ts`/`grep.ts` `format*Call`
  (accent pattern/path + gray ` in …`/` (limit N)` suffixes), `edit.ts`/`write.ts`
  (bold name + accent path), `bash.ts` `formatShellCall` (bold `$ command`), and
  `tool-execution.ts` fallback (bold name + pretty JSON). Colors: name/command in
  `toolTitle` bold, paths/patterns in `accent`, line ranges in `warning`, secondary
  info in `toolOutput`, empty-path default `.`.
- pi hides `read`'s content when collapsed (its result renderer returns `""` unless
  expanded/error); the user asked for content below the title, so slimcode keeps its
  10-line gray preview + Ctrl+O collapse for `read` — note the deliberate difference.
- Deviation from the closed spec: "pretty args" as a displayed section is replaced by
  per-tool titles for built-ins only; the raw args JSON still travels end-to-end
  (history unchanged) and is what non-built-in tools show.
- Watch layout math: title line count shrinks; scrollbar `total_lines` derived from
  `all_rows` so tests asserting scroll thumb positions with tool blocks may need
  updates.
