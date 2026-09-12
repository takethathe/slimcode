# 04: the TUI speaks its own display vocabulary

**What to build:** The TUI no longer consumes the application layer's display contract. The CLI
converts each `DisplayItem` into a `RenderItem` the TUI owns, the transcript is rebuilt from those,
and the rendering is pixel-for-pixel what it is today.

**Blocked by:** 01 (independent of 03)

**Status:** ready-for-agent

- [ ] `slimcode_tui::RenderItem` carries no `ai`/`core`/`app` type:
      `Text`, `Reasoning`, `ToolStart{tool_call_id,name,arguments}` and
      `ToolResult{tool_call_id,name,ok,result}` for the streamed agent output, plus
      `Notice`, `Error`, `UserPrompt`, `Usage(FooterUsage)`, `Skills`, `Branch`, `SessionChanged`
      for state the CLI owns.
- [ ] `App::apply(RenderItem)` replaces `impl Renderer for App`; the transcript merge/pairing rules
      (streaming text and reasoning merged into the last entry, tool start/result paired on
      `tool_call_id`, status colours) stay private to the TUI.
- [ ] The CLI owns the `DisplayItem` → `RenderItem` adapter: it runs on the turn's worker thread and
      is what the worker's channel carries; `DisplayItem::Turn` and `DisplayItem::Stop` are dropped.
      CLI-originated state is emitted as `RenderItem`s too, so the TUI has exactly one input channel.
- [ ] The token-usage conversion is done by the CLI: `FooterUsage` keeps a plain constructor and the
      TUI source no longer names the provider's usage type.
- [ ] `app`'s display contract is untouched: `DisplayItem`, `map_event`, `Renderer`, `usage_summary`
      and the shared turn runner stay exactly as they are (ADR-0004).
- [ ] Tests: a table-driven adapter test covering every `DisplayItem` variant (including the two that
      are dropped); the TUI's `TestBackend` frame snapshots pass with `RenderItem` inputs; the TUI's
      visible output is unchanged.
- [ ] `cargo test` green, `cargo fmt --all`, `cargo clippy --all-targets --all-features --
      -D warnings` clean.
- [ ] Docs: the TUI section of `development.md` gains the `RenderItem`/adapter paragraph; the
      decision is ADR-0014 plus the amendment note on ADR-0004.

## Notes

- Domain terms (per `CONTEXT.md`): `RenderItem`, `DisplayItem`, `Renderer`, `Entry`, `Transcript`.
- This ticket only adds the vocabulary and the adapter; the frame loop and who owns the services
  move in ticket 05.

## Comments
