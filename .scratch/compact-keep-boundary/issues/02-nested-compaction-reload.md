# 02: 多次压缩的嵌套 reload

**What to build:** 一个被压缩过多轮的长 session，用 `/session` 重新打开后，看到的是"最新摘要 + 它保留的近期尾部 + 之后新消息"，所有被取代的旧摘要与旧记录都干净消失——即使磁盘上旧 checkpoint 物理上夹在保留尾部和新消息之间，恢复结果也与实时 session 逐条一致。

**Blocked by:** 01（保留边界落地）

**Status:** done

- [x] `SessionStore::load` 重建 Kept tail 时，从最新 checkpoint 位置**向前**收集 `kept_count` 条**非-checkpoint** 消息，途中**跳过**更旧的 checkpoint；收集满即止，更早的一切（含所有旧 checkpoint）丢弃。
- [x] 依赖事实成立并可断言：`kept_start` 返回 `0` 会先触发 "nothing to compact" 错误，故新 checkpoint 恒为内存首位、上一个 checkpoint 必落入被总结区间、永不出现在 Kept tail 内。
- [x] `slimcode_app::session` 的 load 单元测试：写含两个 checkpoint 的原始日志，第二个的 Kept tail 跨过第一个 checkpoint，断言 load 跳过旧 checkpoint、只留最新 checkpoint 在首位、保留尾部条数正确、被取代记录全部丢弃。
- [x] `cargo test` 全绿；`cargo clippy` 无 error/warning。
