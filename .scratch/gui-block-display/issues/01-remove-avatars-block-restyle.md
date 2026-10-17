# 01: 去 You/AI 头像 + user/assistant 块化重排

**What to build:** GUI 转录区不再出现 "You"/"AI" 圆形头像；每条消息以块为呈现单位。用户输入块用粗体 +
浅靛蓝底深靛蓝字（比助手正文更醒目）；助手块为无边框正文流式（reasoning 折叠与流式光标保留）。工具卡、
用量行、压缩提示、错误块外观不变。这是后续 Transcript chip 落进的容器。

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [x] 转录中任何位置不再渲染 "You"/"AI" 头像（直播流与 resume 回放同样如此）
- [x] user 块以粗体 + 浅靛蓝底（靛蓝体系内的加深版本）呈现，assistant 块无边框
- [x] assistant 块保留 reasoning 折叠与未完成的流式光标行为
- [x] tool / usage / compacted / error 块的呈现不回退（去掉头像后左缩进等对齐关系仍然成立）
- [ ] 手动验收：新会话发消息、恢复旧会话、跑一轮工具调用，三种场景转录观感符合上述各条

## Comments
- 已实现：`Transcript.tsx` 删除 You/AI 头像；user 块粗体 + `bg-indigo-50`/`text-indigo-900`；assistant 无边框正文流（reasoning 折叠 + 光标保留）；tool/usage/compacted/error 保持 `ml-9` 缩进。待手动验收。
