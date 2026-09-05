# 04: docs sync + ADR-0007

**What to build:** Sync the docs with the second-round TUI alignment (AGENTS.md rule 3) and record ADR-0007 for the new layout decisions. No behavior change.

**Blocked by:** 01 — borderless input box + runner status embedded in the top border, 02 — completion popup style/position

**Status:** resolved

- [ ] `docs/development.md`: the layout description changes from the five-region dock `[transcript(Min0) | status(Length 0|1) | input | popup | footer(2)]` to the four-region `[transcript(Min0) | popup(Length 0|n) | input | footer(2)]`; the input box description drops the bordered form and notes top/bottom-only lines; the status indicator section changes from "a spinner row above the editor" to "the runner status is embedded in the input box's top border (`── ⠋ Working...`)"; the completion popup section notes it renders above the input box, borderless (pi SelectList tokens).
- [ ] `docs/user-manual.md`: update the 编辑器边框 bullet (borderless sides), the 状态指示器 bullet (status now on the input box's top border line, left), and the `/` 命令补全弹框 section (opens **above** the input box, no box/title, pi SelectList tokens).
- [ ] `CONTEXT.md` glossary: rewrite `Status indicator` ("spinner row above the editor" → "runner status embedded in the input box's top border"), `Completion popup` ("shown below the TUI input box" → "shown above the TUI input box"), and `Dock` ("status indicator row, editor, completion popup, footer" → "editor, completion popup, footer").
- [ ] `docs/explanation.md` / `docs/index.md`: update any dock/status/popup description; index gains ADR-0007.
- [ ] New `docs/adr/0007-tui-second-round-pi-alignment.md`: records the decisions — borderless input box (top/bottom only), runner status embedded in the input top border (pi `embedWorkingStatus`), borderless pi-SelectList completion popup, and the deliberate popup-above-input preference; supersedes ADR-0006 D4/D5 wording for the dock/status/popup.
- [ ] `cargo fmt --all` clean and `cargo clippy --all-targets --all-features --message-format=json -- -D warnings` zero errors/warnings before committing.

## Notes

- Docs may be Chinese or English but keep the glossary terms consistent (AGENTS.md rule 6, `CONTEXT.md`).
- Commit as `docs(...)` / `feat(...)` per AGENTS.md git conventions; local commits only, no push.

## Answer

Implemented and green (Ticket 04):
- New `docs/adr/0007-tui-r2-alignment-with-pi.md` (D1 borderless editor, D2 status embedded in top border, D3 borderless SelectList, D4 popup above editor).
- Supersede pointers added to ADR-0005 (popup form/position) and ADR-0006 (dock order/status row/editor border).
- `CONTEXT.md`: Completion popup now "above", Dock order updated, Status indicator redefined as embedded-in-border.
- `docs/user-manual.md`: editor border bullet (no sides/corners, status in top border), status indicator bullet, popup bullet + `/` 补全弹框 section (above the input).
- `docs/development.md`: dock layout `[transcript | popup | input | footer]`, status-in-border wording, smoke-test description.
- `docs/index.md`: ADR-0007 listed.
