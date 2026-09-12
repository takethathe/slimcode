# 02: BailianConfig 更名为 ProviderConfig（expand–contract）

**What to build:** provider 配置对外暴露为泛化的 `ProviderConfig`，`BailianConfig`
命名不再泄漏到 app/cli 层。这是 wide refactor（机械改名、波及全部 crate、无独立
用户可见行为），按 expand–contract 顺序落地，全程 CI 保持绿：先引入新名并保留旧名
别名，再分批迁移调用点，最后删除旧名。

**Blocked by:** 01 (needs the wire marking shape settled first — 01 touches the
same ai crate tests; sequencing avoids parallel churn)

**Status:** resolved

- [x] expand：`ProviderConfig` 成为正式类型（字段/建造器/默认值不变，cache 默认
      true），`BailianConfig` 暂作别名，现有全部调用照常编译。
- [x] migrate：app 层（配置解析、共享 setup、runner 签名）调用点切到
      `ProviderConfig`，CI 保持绿（别名仍在）。
- [x] migrate：cli 层（one-shot 与 TUI）调用点切到 `ProviderConfig`，CI 保持绿。
- [x] contract：无调用者残留后删除 `BailianConfig` 别名。
- [x] CONTEXT.md 的 `Config` 词条与相关文档不再引用 `BailianConfig`。
- [x] 全量测试通过；`cargo fmt --all` 与 clippy（`-D warnings`）干净。
