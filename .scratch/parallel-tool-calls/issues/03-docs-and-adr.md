# 03: 文档与 ADR 收尾

**What to build:** 实现落定后同步文档：agent 运行时循环与工具章节、ai provider 请求说明、文档索引，并新增 ADR 记录关键决策——为何用线程而非异步、结果顺序与事件顺序为何分离、默认开启与思考模型风险的已知项。最后按 spec 的验收标准逐项核对。

**Blocked by:** 02

**Status:** resolved

- [ ] `docs/development.md` 更新：agent 运行时循环（真并发、顺序语义、取消）、工具章节（闭包约束）、ai provider 请求（并行参数）
- [ ] `docs/index.md` 的 ADR 索引追加新 ADR
- [ ] 新增 ADR：线程并发 vs 异步的取舍、结果/事件顺序分离、默认开启与思考模型风险
- [ ] 按 spec 验收标准逐项核对并勾选
- [ ] `cargo fmt --all` 已应用；`cargo clippy` 0 error / 0 warning；`cargo test` 全绿

## Answer

已实现：`docs/development.md`（ai 请求参数、agent 运行时循环）、`docs/user-manual.md`（工具集并行说明）、
`docs/index.md`（ADR 索引）、新增 `docs/adr/0010-parallel-tool-calls.md`；spec 验收标准逐项核对通过。
