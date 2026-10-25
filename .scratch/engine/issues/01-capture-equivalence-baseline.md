# 01: 固化等效性金标准基线

**What to build:** 一套端到端集成测试 harness，把迁移前 slimcode 的公共可观测行为固化为金标准快照：含工具回合的脚本化 provider 跑一轮后，记录有序事件流（Display/Message/TurnFinished/Compacted）、JSONL 会话日志逐记录字节、usage 累加、title 推断、取消/报错两路的收尾行为、越阈值历史的 compaction checkpoint、以及模型 read skill `SKILL.md` 得到的激活块文本。此后所有迁移工单以比对这套基线为验收门（red 阶段先跑通基线本身）。

**Blocked by:** None (can start immediately)

**Status:** resolved

- [x] 基线 harness 落在 sdk/app 的集成测试层：真实 `AgentSession` + 脚本化 provider + 真实或内存 SessionStore，只读现有行为、不加任何新功能
- [x] 覆盖 spec 测试清单 13–18 对应的每一类外部输出（事件序列、日志字节与回放、usage/title/stop reason、compaction 两路、cancel/error 收尾、skill 拦截文本）
- [x] 全绿通过于当前未改动的代码（即基线自身成立）
- [x] 基线断言写成后续工单可直接复用的比对入口（同一组脚本输入 → 期望外部输出）
