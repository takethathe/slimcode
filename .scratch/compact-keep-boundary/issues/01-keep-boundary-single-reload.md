# 01: 保留边界落地 —— 单 checkpoint 的 Kept tail 跨 reload 存活

**What to build:** 让一个刚被压缩过的 session，关掉再用 `/session` 打开时，compaction 特意保留的那段近期上下文（Kept tail）重新出现在恢复出的消息历史里，喂给模型的顺序与实时 session 逐条一致：`[最新 CompactSummary, …Kept tail, …之后新消息]`。压缩前生成的旧 session（其 checkpoint 记录不带保留边界）重新加载时保持今天的既有行为——丢弃 checkpoint 之前的一切，不报错、不迁移。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] `CompactSummary` 变体新增 `kept_count` 字段（本 checkpoint 保留的、紧邻其前的非-checkpoint 消息条数），serde 取 `default`，纯 additive：新版读旧记录得 `0`、旧版读新记录忽略未知字段。
- [x] `compact_messages` 在生成 checkpoint 时把 `kept_count` 写为保留尾部的长度；"最新单条即超保留预算"的零保留情形下 `kept_count = 0` 且合法。
- [x] `SessionStore::load` 取代旧的 `drain(..index)`：定位最新 checkpoint 后，重建为 `[最新 checkpoint] ++ Kept tail ++ [其后记录]`，把 checkpoint 提到 Kept tail 之前。
- [x] legacy checkpoint（无 `kept_count` 字段）恢复为 `kept_count = 0`，等价于"丢弃 checkpoint 之前一切"，与当前行为逐字节一致。
- [x] `slimcode_app::session` 的 load 单元测试：写原始日志 `[…old…, kept…, compact(kept_count=2), after…]`，断言恢复为 `[compact, kept…, after…]`，Kept tail 存活。
- [x] `slimcode_app::session` 的 load 单元测试：legacy 裸 JSON checkpoint 断言退化为丢弃之前一切。
- [x] `slimcode_app::compaction` 的 `compact_messages` 单元测试扩展：断言 checkpoint 的 `kept_count == 保留尾部长度`。
- [x] `crates/sdk/tests/auto_compaction.rs`：断言 `compact` 后落盘再重载的 `session.messages` == live 历史（对称性）。
- [x] `ContextBuilder::build()` / `inject_compaction` 不改（仍是保位替换），据此模型读到 `[summary, kept…, new…]`。
- [x] `cargo test` 全绿；`cargo clippy --all-targets --all-features` 无 error/warning。
