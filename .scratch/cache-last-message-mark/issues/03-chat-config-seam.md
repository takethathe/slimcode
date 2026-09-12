# 03: chat 边界传 ProviderConfig + provider 无状态 + 穿线

**What to build:** 配置在 chat 边界经 `ProviderConfig` 传递给 provider：`chat`
接收 `&ProviderConfig` 入参（含 cache 开关），provider 实例变为无状态（构造只
构建 HTTP client）；`RunConfig` 保持纯运行时配置、不承载 cache；runner / 共享
turn 运行器 / CLI one-shot / TUI 完成穿线，测试替身 provider 适配。这是 wide
refactor 的收尾：配置真正跨 seam 传递，`RunConfig` 与 runner 无关的设置清零。

**Blocked by:** 02 (needs the `ProviderConfig` name in place)

**Status:** resolved

- [x] `chat` 签名携带 `&ProviderConfig`，provider 从入参读取 model / api_key /
      base_url / cache，不再依赖自身持有的配置。
- [x] provider 构造不再接收配置（只构建 HTTP client）；`RunConfig` 字段不变
      （仍只有 parallel_tools，不出现 cache）。
- [x] runner 持有 config 引用并在每轮 chat 时传递；共享 turn 运行器签名携带
      config。
- [x] CLI one-shot 与 TUI 各自把 config 传进每轮 turn（TUI 的运行时状态持有
      config）。
- [x] 测试替身 provider 适配新签名后全绿，证明 config 经 seam 到达 provider。
- [x] 全量测试通过；`cargo fmt --all` 与 clippy（`-D warnings`）干净。
