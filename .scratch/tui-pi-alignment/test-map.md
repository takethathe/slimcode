# Feature → test coverage map (ticket 04)

Every spec User Story (US) and Implementation Decision is covered by at least one
test. Leaf-level pure functions carry their own unit tests; App behavior is
asserted on the **frame buffer** (ratatui `TestBackend`) per spec; the thin
terminal shell's only logic (worker channel hand-off, quit-after-turn wiring) is
covered end-to-end; and the real-terminal story is exercised by the tmux smoke
test (`crates/cli/tests/tui_smoke.rs`, auto-skip without tmux). Counts at the
ticket-04 commit: commands 26 · common 107 · agent 48 · ai 27 · cli 24 (+ 1
tmux) · tui 106 = 339.

Legend: `app::` = `crates/tui/src/app.rs` frame-buffer tests; `markdown::` /
`theme::` / `text::` / `footer::` / `git::` = pure unit tests in
`crates/tui/src/…`; `terminal::` = `crates/tui/src/terminal.rs`; **tmux** = the
smoke test (skipped when `tmux -V` fails).

| Spec | Feature | Tests |
| --- | --- | --- |
| US 1 | startup header (bold accent `slimcode` + dim version + hint line) | `app::header_renders_at_startup_and_new_clears_it`; tmux waits for `slimcode` + `/help for commands` |
| US 2 | boxed user prompt (`userMessageBg`) | `app::user_prompt_renders_as_boxed_message_with_bg`; tmux asserts prompt text present after Enter |
| US 3 | assistant markdown in pi token colors | `markdown::` 15 tests: heading color w/o markers, bold/italic modifiers, inline code, fenced/indented code + bare fences, blockquote prefix, ul/ol bullets (incl. explicit start), link colors, rule, wrap, mixed order, nested modifiers |
| US 4 | thinking as italic gray markdown | `app::thinking_renders_italic_gray`; tmux waits for `Thinking about the directory…` |
| US 5 | tool call as one state-colored block (bold title, pretty JSON args, gray output) | `app::tool_start_pairs_with_result_into_one_block`; `app::tool_block_pending_then_success_backgrounds`; `app::tool_block_error_background_and_pretty_args`; `app::failed_tool_result_sets_error_block`; tmux asserts real `ls` output + pretty args visible |
| US 6 | collapse to 10 lines with `… (N more lines, …)` hint | `app::tool_output_collapses_to_ten_lines_and_ctrl_o_expands`; `app::tool_output_under_ten_lines_has_no_hint` |
| US 7 | Ctrl+O expand/collapse | same two tests above (`tool_output_expanded` global) |
| US 8 | streamed text still merges into one block | `app::transcript_accumulates_streamed_text`; `app::transcript_accumulates_streamed_reasoning`; `terminal::channel_worker_streams_ordered_items_and_returns_messages` |
| US 9 | turn markers / `✓ done` / per-turn usage line gone | `app::completed_stop_renders_nothing_and_turn_marker_is_ignored`; usage now feeds footer (`footer::from_token_usage_maps_cache_fields`) + `/usage` dim notice (`app::usage_renders_token_counts`, `app::usage_renders_cached_count`, `app::notice_renders_dim`) |
| US 10 | two-line dim footer, stats + right-aligned model | `app::footer_renders_two_dim_lines`; `footer::` all 6 (format_tokens boundaries, format_cwd, stats_parts, hit %, stats_line right-align, From<TokenUsage>); tmux asserts line1 ` • ` + line2 stats with `mock-model` right-aligned |
| US 11 | branch dim in parens next to cwd (in a repo) | `git::current_branch_reads_checked_out_branch` (temp `git init`); `git::current_branch_none_outside_git_repo`; `app::footer_omits_branch_and_usage_when_absent`; `footer::format_cwd_for_footer_shortens_home` |
| US 12 | spinner row above editor (braille 80ms, accent + muted `Working...`), hidden idle | `app::status_indicator_only_while_running_and_animates`; `app::footer_omits_branch_and_usage_when_absent` idle-status row width 0 covered by layout; tmux asserts distinct spinner chars across captures while running and no `Working` once idle |
| US 13 | spinner keeps animating during long tool runs (worker thread) | `app::status_indicator_only_while_running_and_animates` (`tick()` advances frames); `terminal::channel_worker_streams_ordered_items_and_returns_messages` (turn runs on worker while UI loop ticks); tmux captures distinct frames while the mock drip-feeds an HTTP wait |
| US 14 | Ctrl+C/D during run → quit after turn | `app::running_keys_ignore_all_except_ctrl_c_d` (pure `Effect::QuitAfterTurn` transition; other keys ignored); `terminal::` idle Ctrl+C quit wiring; tmux Ctrl+C exits and restores the pane |
| US 15 | editor border blue rest / cyan running | `app::editor_border_color_reflects_running_state` |
| US 16 | popup SelectList tokens (accent `→` row, muted description, muted `(i/n)`) | `app::completion_popup_uses_select_list_tokens`; `app::completion_popup_shows_scroll_info_when_overflowing`; tmux opens `/` popup and Esc closes it |
| US 17 | right-edge scrollbar, `selectedBg` thumb, auto-fade | `app::scrollbar_appears_on_scroll_and_fades_on_ticks`; tmux PageUp shows header, PageDown back to bottom |
| US 18 | OSC 0 title `slimcode - <session> - <cwd basename>`, updated on /new /load | `git::terminal_title_uses_cwd_basename`; tmux `#{pane_title}` assert |
| US 19 | `/help` `/history` `/sessions` `/skills` `/usage` + confirmations as dim notices | `app::slash_help_and_slash_skills_render_lists`; `app::slash_history_lists_prompts_as_transcript_entries`; `app::usage_renders_token_counts`; `app::usage_renders_cached_count`; `app::notice_renders_dim` |
| US 20 | failures in red | `app::error_renders_red`; `app::abnormal_stop_renders_red_error_text`; `app::failed_turn_appends_error_and_returns_to_input` |
| US 21 | resize keeps dock fixed, transcript fills above | `app::resize_re_renders_layout`; tmux resize 100×34 → 80×14 → 100×30 keeps footer last line |
| US 22 | pure `App` core + `TestBackend` frame-buffer tests | all `app::` tests above (70 tests) run headless via `TestBackend`; no real terminal required |
| US 23 | theme/markdown/footer pure unit-tested leaves | `theme::` 5 (fg/bg hex == pi dark.json, styles, modifiers, distinctness); `markdown::` 15; `text::` 6 (width/wrap incl. CJK); `footer::` 6; `git::` 3 |
| US 24 | one-shot CLI byte-identical | `crates/cli/src/main.rs` 24 tests (dispatch/parse/flags) green; one-shot render/runner untouched by this alignment (git diff: only `crates/agent` `+ Send` widening + `crates/tui` + tests; no edits under one-shot output paths); shared `common::render::map_event` / `usage_summary` unchanged (CLI consumes the same lines it did) |
| D1 theme | semantic tokens == pi dark.json hexes, dark only | `theme::` 5 |
| D2 blocks | block-aware transcript, merge rules, boxes, header, notices | US 1/2/4/5/8/9 rows above |
| D3 header/dock/layout | five-region dock, status row collapses idle | `app::footer_renders_two_dim_lines` + layout assertions in running-state tests; `app::resize_re_renders_layout`; tmux resize |
| D4 select-list | popup + editor border semantics | US 15/16 rows above |
| D5 footer | both lines dim, no gray branch; formatTokens/formatCwd port | `footer::` 6 + `git::` 3 + US 10/11 |
| D6 spinner/worker | worker thread + mpsc channel + 80ms frame; quit-after-turn; keys ignored while running | `terminal::channel_worker_streams_ordered_items_and_returns_messages`; `app::status_indicator_only_while_running_and_animates`; `app::running_keys_ignore_all_except_ctrl_c_d`; tmux spinner/Ctrl+C |
| D7 terminal title / branch | OSC 0 + best-effort branch; both pure wrappers | `git::terminal_title_uses_cwd_basename`; `git::current_branch_*`; tmux title |

Spec says the checklist must have **no line without a test**: the tmux row is a
test (auto-skip), and each US above lists ≥ 1 concrete test name. `cargo
llvm-cov` was not installed in this environment; the manual `cargo test
--workspace` + the tmux smoke serve as the acceptance gate instead.

---

## Revision-2 additions (tickets 05–07)

Rows added as each revision-2 ticket lands; all other rows above stay valid
unless a rewrite (06 replaces the pretty-args shape of US 5/6 for built-ins).

| Spec | Feature | Tests |
| --- | --- | --- |
| R1 (ticket 05) | every tool-block row padded to the pane width with the state bg (title / output / expand-hint; pending / success / error) | `app::tool_block_background_reaches_rightmost_column_for_every_row_kind` (per row kind × state, rightmost-column cell bg + solid band); `app::tool_block_padding_is_display_width_exact_for_wide_chars` (CJK rows pad by display width) |
| R2 (ticket 06) | `read` offset/limit slicing | `files::slice_lines_without_range_returns_content_unchanged` / `slice_lines_applies_1_indexed_offset_and_limit` / `slice_lines_clamps_at_eof_and_zero_limit` / `slice_lines_keeps_no_trailing_newline_content_exact` (agent); `files::read_tool_slices_by_offset_and_limit`, `files::read_tool_ignores_non_positive_offset_args`; `common::tools::read_tool_slices_by_offset_and_limit_via_build_tools` |
| R2 (ticket 06) | per-tool compact call titles | `toolcall::` 16 unit tests (read name/range in accent+warning, missing range = no suffix, limit-without-offset `:1-N`, zero-offset clamped to `:1`, missing/invalid fields → `None` fallback, empty-path `...`; ls path + `.` default + `(limit N)` suffix; grep `/pattern/ in scope`, missing pattern → fallback; find pattern+scope with `.` defaults; edit/write path; bash `$ command` bold + `$ ...` gray fallback; unknown/non-object args → fallback). Frame buffer: `app::builtin_tool_blocks_never_show_json_args_section` (read range title, `$ ls -la`, `grep /TODO/ in src`, no JSON); `app::unknown_tool_block_keeps_pretty_json_args_fallback`; `app::tool_block_error_background_and_compact_title`; `app::tool_call_title_wraps_at_pane_width_without_losing_text` (long bash title wraps, full-width band intact). tmux waits for `ls .` header |
| R3 (ticket 07) | Esc cancels the running turn | `agent::cancel_token_defaults_false_flips_resets_and_shares`; `agent::cancel_before_first_chat_stops_without_calling_provider` (zero provider calls); `agent::cancel_during_request_streams_deltas_but_skips_history`; `agent::provider_error_while_cancel_set_is_a_silent_cancelled_stop`; `agent::cancel_after_turn_one_tools_keeps_pushed_tool_result`; `agent::cancel_between_serial_tools_keeps_only_completed_results`; `agent::cancel_between_parallel_tools_stops_before_applying_the_batch`; `provider::read_body_*` 5 (complete/empty/already-cancelled zero-reads/between-chunks partial/io-error) + `trim_*` 2 + `partial_stream_salvages_deltas_received_before_the_cancel` (ai); `runner::cancel_flip_mid_run_stops_the_stream_with_cancelled` (common); `common::tools::cancellable_bash_kills_the_child_promptly_on_cancel` (no 30s wait) + `cancellable_bash_still_returns_normal_output_when_not_cancelled` + `build_tools_with_cancel_exposes_the_seven_tools`; `app::bare_escape_cancels_the_running_turn` (`Effect::CancelRunning`, shift+Esc ignored) + `app::cancelled_stop_renders_nothing_like_completed`; `render::stop_maps_to_stop_marker` incl. Cancelled; tmux `tmux_escape_cancels_a_running_turn` (Esc mid-drip: spinner gone, input usable, partial text kept, no error, next prompt runs) |
