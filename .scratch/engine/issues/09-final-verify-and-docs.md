# 09: 终验——FFI/GUI 面不变 + architecture/docs 同步 + 全绿报告

**What to build:** 把整条迁移收口成一份可交付证据：`cargo test` 全绿且工单 01 的等效性集成基线逐点通过（迁移前 vs 迁移后 code preset 外部输出一致）；确认非-Rust 与 web 消费面未被本轮改动破坏——FFI 纯 C ABI（同步窄接口、per-handle 事件队列、`include/slimcode.h` 契约）与 GUI wire 事件签名/词汇不变；同步受影响文档（`docs/development.md` 的 app 引擎接缝/async 门面/trace 组件、`docs/index.md` 的 ADR-0031 与 `.scratch/engine/spec.md` 索引）。产出简短的等效性与接缝就位报告。

**Blocked by:** 01, 02, 03, 04, 05, 06, 07, 08

**Status:** resolved

- [x] `cargo fmt --all`、`cargo clippy --all-targets --all-features -D warnings` 无 error/warning；`cargo test` 全绿
- [x] 工单 01 等效性集成基线全部逐点通过（迁移后 = 迁移前，spec 清单 13–19）
- [x] FFI C-ABI 与 GUI wire 事件对外签名/词汇未破坏（现有 ffi/gui 测试与 `slimcode.h` 契约仍成立，spec 清单 12）
- [x] `docs/development.md`、`docs/index.md` 与 ADR-0031 同步；`.scratch/engine/spec.md` 链接就位
- [x] architecture.rs 依赖矩阵反映加宽后的 core/app 接缝且无新增 crate、无 tokio 依赖
