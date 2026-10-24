# 02: 瀑布流不再出现 usage 行，`/usage` 改为让 HUD 闪一下

**What to build:** 一次带两次工具调用的 turn 跑完后，对话正文里**一行** `tokens: … prompt (… cached, …%)
+ … completion = … total` 都不再有——转录模型里没有 usage 这个 block，渲染组件与追加入口一并消失，恢复
历史会话也不会回放出来。敲 `/usage` 时不再往对话里贴汇总，而是让右上角那组徽标脉冲一下（约 600ms 的
ring；系统开启"减少动效"时退化为静态描边），同时拉一次快照校准数字。wire 的 usage 事件与 Rust 侧
（`DisplayItem::Usage`、`/usage` 拦截集合）保持不动，只改前端如何消费它们。

**Blocked by:** 01（HUD 必须先接上实时数据，否则这一张会让用量在界面上无处可看）

**Status:** done

- [x] 单轮多工具调用的 turn 结束后，转录里没有任何 usage 行（spec D6）
- [x] 恢复一个跑过 usage 的历史会话，转录同样不出现 usage 行
- [x] `/usage` 不追加 block，只让 HUD 脉冲，且数字与宿主快照一致（spec D7）
- [x] `prefers-reduced-motion: reduce` 下脉冲退化为无动画的静态描边
- [x] reducer 单测固化新契约：收到 usage 事件后 block 数量与内容不变
- [x] 全仓不再有旧的 usage 行组件/追加入口的引用；`/usage` 仍在 GUI 拦截集合里，不会作为 prompt 发给模型
