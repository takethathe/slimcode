# 04: ADR-0016 + CONTEXT.md + docs 同步

**What to build:** 本次确立的边界规则与实现方式落档，供未来加 provider 的人直接
引用：provider-owned concern 的判别测试（"换一个 provider，这一项的实现会不同
吗？"）、runtime 配置 vs provider config 的归属、cache mark placement 归
provider、不做工具 sort、字节确定性契约。CONTEXT.md 术语与相关 docs 同步到终态。

**Blocked by:** 03 (docs describe the final seam state)

**Status:** resolved

- [x] ADR-0016 记录边界规则与取舍：provider-owned concern / runtime 配置 /
      provider config 三分，mark placement 归 provider，无工具 sort，字节确定性
      契约，`RunConfig` 纯运行时。
- [x] CONTEXT.md 新增 `ProviderConfig` / provider-owned concern / cache mark
      词条，更新 `Config` 词条的 `BailianConfig` 引用。
- [x] docs/development.md、docs/explanation.md、docs/configuration.md 同步
      （wire 模型、缓存机制 system + 最后一条消息、配置引用）。
- [x] 本 effort 的 spec 与全部 issue 状态置 resolved。
