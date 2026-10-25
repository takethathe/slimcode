# 08: 迭代/超时护栏 + 取消回滚游标

**What to build:** 就位 bot 刚需的循环护栏与回滚：Engine 的 run 配置新增**可选** `max_iterations` 与超时护栏（默认仍不设上限，供 code 型），到阈值即受控结束并给明确 stop 语义；per-run 记录消息数基线（`initial_message_count` 游标），在取消/中止（aborted，含护栏停止）时经默认 after_turn hook 回滚到该游标，使会话状态一致；provider 错误路径不触发 after_turn（有意，见 ADR-0031 D2），出错回合直接把 `Err` 交回宿主、由宿主负责恢复。跨会话队列/串行调度仍留宿主，本票只保证单会话可取消/可回滚。

**Blocked by:** 05

**Status:** resolved

- [x] 设 `max_iterations`/超时的 run 在阈值处受控停止、返回可区分的 stop reason；未设时 code 型行为不变（默认无上限）
- [x] 取消/中止（含护栏停止）回合按 `initial_message_count` 游标经 after_turn 回滚，会话与日志状态一致；出错回合不触发 after_turn、把 `Err` 交回宿主由其恢复（spec 清单 8 的回滚部分）
- [x] 护栏与回滚是 bot hook bundle 的可选件：code 默认路径不挂即无感，跑工单 01 基线仍**逐点等效**、全绿（spec 清单 19）
