# 05: 设置面板（gui.toml）

**What to build:** 设置入口打开模态面板，可改 model / base_url / api_key / cache；保存写入 GUI 专属设置文件（api_key 写入后权限收紧 0600），新建会话时作为 SDK overrides（最高优先级）传入；设置只影响 GUI，绝不写共享的 `config.toml`；改完下一次 prompt 即生效（不必重启）。

**Blocked by:** 02

**Status:** resolved

- [x] 宿主：`get_settings` / `set_settings` 读写 GUI 专属设置文件（model / base_url / cache / 可选 api_key）；写 api_key 后文件权限 0600；缺省字段回落默认值
- [x] overrides 组装：设置值传入每个新建会话（`with_model` / `with_base_url` / `with_api_key` / `with_cache`）；设置变更后当前会话下一次 prompt 前重建 overrides
- [x] 绝不写 `config.toml`：设置路径与共享配置文件完全隔离
- [x] 前端设置模态面板：四个字段的表单、保存/取消、api_key 显示为密码输入
- [x] Rust 单测：写入后权限 0600、缺省回落、overrides 组装正确（FakeProvider 能看到新配置）、设置文件与 `config.toml` 互不影响
- [x] 手动验收：改模型/端点/api_key → 保存 → 重启应用 → 新配置生效；`config.toml` 内容未被触碰

**Gate:** `gui/` 内 `cargo test` 全绿；手动走通「改设置 → 保存 → 重启生效 → config.toml 未被触碰」。

## Answer

Implemented: `gui.toml` (model / base_url / cache / optional api_key) read at startup as the
highest-precedence SDK overrides and written only by the GUI (never `config.toml`); 0600
permission tightening on save (unix); settings changes rebuild the current session's overrides
before the next prompt. The `SettingsModal` edits the four fields with api_key as a password
input. Unit tests cover 0600, defaults, override assembly (FakeProvider observes new config),
and `config.toml` isolation.
