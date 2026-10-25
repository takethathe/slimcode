# 02: core 扩为七阶段管线（expand）

**What to build:** Loop skeleton 从现有 3 个 run hook 扩展为七阶段 phase-hook 管线——pre_turn / pre_chat / after_chat / pre_tool / after_tool / post_iteration / after_turn，贯穿一份不可变协作者集与一份可变 run state（含跨阶段类型化 extension map），返回流控（非工具阶段 Continue/Abort；pre_tool Execute/Skip/Abort；after_turn 无论成败/取消恒运行）。采用 expand–contract 的 expand：旧 3-hook 接缝原样保留、与新阶段并存，现有调用方（app runner、sdk、各测试）零破坏，CI 保持绿。

**Blocked by:** None (can start immediately；与 01 并行)

**Status:** resolved

- [x] 一处循环仍是唯一 Loop skeleton（扩展 AgentRunner，不新增第二套 loop）
- [x] 七个阶段各可注册 0..N 个 phase hook，按序调用；pre_tool 每 call 模型顺序、并行模式下批次全部先跑
- [x] phase hook 可改写即将进历史的 AgentMessage（模型/日志/显示三处看到同一份文本，延续 ADR-0015 语义）
- [x] 流控语义生效：Skip 仍产配对工具结果；Abort 使整轮跳到 after_turn；after_turn 在取消路径也运行
- [x] 旧 before_tool/after_tool/turn_end 三 hook 仍编译、仍生效（contract 留给 04）
- [x] 针对 core 的 phase-hook 单元/行为测试（注入脚本 provider）全绿；不依赖任何 sdk/app 改动
