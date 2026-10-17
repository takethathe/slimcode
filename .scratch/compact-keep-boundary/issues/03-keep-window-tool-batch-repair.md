# 03: 保留窗口处的 tool-batch 配对与先裁剪后修复

**What to build:** 当被保留的近期尾部以一个带 tool 调用的 assistant 开头时，重新加载后的历史里每个 `tool_call_id` 都仍有配对的 tool result；并且悬空 tool-batch 的内存修复只作用在真正保留下来的历史上——不会因为"给即将被丢弃的旧区段凭空补 `Error: interrupted` 结果"而虚报修复计数。

**Blocked by:** 01（保留边界落地）

**Status:** done

- [x] `SessionStore::load` 调整为**先按 `kept_count` 裁剪到保留窗口，再对存活历史做悬空 tool-batch 修复**（`close_batch`）。
- [x] 保留窗口的起点由 `compact_messages` 侧保证不落在 tool result 上（Kept tail 起点是 assistant，其 tool result 全在窗口内），因此存活历史自带配对。
- [x] `LoadOutcome.repaired_tool_calls` 不把被丢弃旧区段的凭空修复计入。
- [x] `slimcode_app::session` 的 load 单元测试：构造"Kept tail 起点是带 tool 调用的 assistant、其后紧跟其 tool result"的原始日志，断言 reload 后每个 `tool_call_id` 配对、存活历史合法。
- [x] `slimcode_app::session` 的 load 单元测试：旧区段存在悬空批次时，断言该批次被丢弃且不计入 `repaired_tool_calls`。
- [x] `cargo test` 全绿；`cargo clippy` 无 error/warning。
