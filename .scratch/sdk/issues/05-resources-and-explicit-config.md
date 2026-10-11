# 05: 资源组装与显式配置（环境 / context files / skills / 选项覆盖）

**What to build:** SDK 会话与 CLI 行为对齐的剩余拼图：从 cwd 发现并注入 AGENTS.md 上下文文件与环境信息，发现 skills 并在系统提示中广告、把模型对 skill 自身 `SKILL.md` 的 read 拦截为 skill 激活块；显式 `base_url` / `model` / `api_key` / `cache` 覆盖四层配置解析，让未来 FFI 宿主不依赖配置文件即可指向自建端点。

**Blocked by:** 02（会话对象与回合编排）

**Status:** resolved

- [x] 会话从 cwd 发现并注入 AGENTS.md 上下文文件与环境信息（与 CLI 逐字段一致；无文件时整个章节省略、系统提示逐字节不变）
- [x] skills 从 user/project 两 scope 发现并在系统提示中广告（`disable-model-invocation` 语义保留）；模型 read 某 skill 的 `SKILL.md` 被拦截为 skill 激活块（已加载时给 already-loaded 提示），复用既有 hook seam
- [x] 显式 `base_url` / `model` / `api_key` / `cache` 覆盖四层解析，且经 `chat` seam 原样传给 provider（对齐「配置跨 seam 传递」语义）
- [x] 注入自定义 provider / tools 生效（脚本化 provider + 自定义工具被执行）
