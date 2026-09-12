# 04: Docs sync for the appended JSONL session log

**What to build:** Every document that describes session storage, the `/save` command or the old
whole-file JSON format is brought in line with the shipped behaviour. The ADR
(`docs/adr/0009-appended-jsonl-session-log.md`), the ADR index row in `docs/index.md` and the
glossary entries in `CONTEXT.md` (Session, Session log, Log header, Dangling tool batch, Session
store, Eviction) already exist and only need checking against the code.

**Blocked by:** 03 (switch session persistence to the appended log)

**Status:** ready-for-agent

- [ ] `README.md`: the command list no longer advertises `/save`; the session file example uses a
      `.jsonl` log.
- [ ] `docs/user-manual.md`: the 会话文件 section describes the log (header + one record per
      message, appended per message, created at the first assistant message, lenient load with the
      skipped-records notice, in-memory repair of dangling tool batches); the 磁盘清理 section uses
      the new empty-log rule; the session command table drops `/save`.
- [ ] `docs/configuration.md`: the 会话存储配额 section refers to session logs (`.jsonl`) and keeps
      the "legacy `.json` invisible but counted" statement.
- [ ] `docs/development.md`: the `slimcode-common` session notes and the `slimcode-agent` session
      notes cover the log format, append-per-message, the new per-message event and the removal of
      the whole-file save API.
- [ ] No stale wording left behind (grep for the old session path, `/save`, "空会话（messages 为空、
      0 字节或 JSON 损坏）", "每轮后自动保存", "unparseable").
- [ ] ADR-0008 is not rewritten: if a pointer helps, add a "superseded in part by ADR-0009" note
      rather than editing its decisions.
- [ ] `cargo fmt --all`; `cargo clippy --all-targets --all-features --message-format=json --
      -D warnings` reports 0 errors / 0 warnings; `cargo test` is green.

## Notes

- The docs are the only place where the old behaviour is described; the code no longer emits it.
- Keep the glossary wording exactly as the ADR uses it (session log, log header, dangling tool
  batch, project home) so the vocabulary stays single-sourced.
