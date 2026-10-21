# 房间消息列表抽取

## 背景

`Room.tsx` 同时承担消息状态、WS 更新、合并去重、滚动容器和全部消息 JSX，无法单独测试消息 UI。本任务把消息条目渲染抽成 `MessageList` / `MessageItem`，行为、DOM、class 和样式保持不变；Room 继续负责 socket、语音、消息状态、合并去重和滚动容器。

计划来源：`local://room-message-list-plan.md`。

## 变更

- 新增 `frontend/src/components/MessageList.tsx`
  - `MessageList` 按 `messages` 原顺序渲染，`key={message.id}`。
  - `MessageItem` 保留原 `<li>` 结构、class、系统消息分支、回复引用、时间、正文、回复/复制/emoji 回应和 reaction row。
  - 原 `REACT_EMOJIS`、`highlightMentions`、`renderBody` 原样迁入，`fmtMsgTime` 从 `lib/time` 引入。
  - 组件只调用 `onReply` / `onCopy` / `onReact`，不直接访问 socket、clipboard、DOM 或媒体。
- 修改 `frontend/src/pages/Room.tsx`
  - 引入 `MessageList`，保留 `<ul className="messages">` 作为滚动容器。
  - 新增 `replyToMessage`（设置引用并聚焦 composer）、`copyMessage`（clipboard）、`reactToMessage`（WS reaction）三个回调。
  - 删除已迁出的消息 JSX 和 `renderBody`/`highlightMentions`/`REACT_EMOJIS`；`fmtAgo`、`fmtMsgTime` 导入按实际使用调整。
  - 消息状态、`lastSeqRef`、`loadMessages`、`mergeMessages`、通知/提示音、发送 `reply_to`、语音和 WS 均未改动。
- 新增 `frontend/src/components/MessageList.test.tsx`，7 个 RTL 用例。

## 边界与不变量

- **进入 MessageList/MessageItem**：消息条目 DOM、系统消息、回复引用、时间格式、URL 链接、@提及、回复/复制/emoji 回调、reaction 计数和 `me` class。
- **留在 Room**：`messages` 状态与排序、`mergeMessages` 去重、`<ul className="messages">` 滚动容器、socket/WS、语音、composer、通知/提示音、clipboard 与 focus 的 DOM 副作用。
- `key` 仍为消息 `id`，不使用 index。
- 当前 Room 没有自动到底/`scrollTop` 逻辑；本次不新增也不移除任何滚动行为，仍由 `.messages` 的 CSS 原生滚动承担。
- 未改 `styles.css`、API/WS payload、`mergeMessages`、`fmtMsgTime` 或任何消息业务逻辑。
- 无 UI 快照、无 Room/WS/媒体 mock、无新依赖。

## 测试

`npm test`：10 个文件、47 个测试全部通过；原有 40 个测试未回退，新增 7 个：

1. 消息顺序 + `mergeMessages` 去重（重复 id 只渲染一次）。
2. 时间格式：同日 `HH:MM`，跨年使用 `fmtMsgTime` 期望值（避免时区硬编码）。
3. 空列表：保留空 `ul.messages`，不渲染占位文案。
4. 系统消息：`message system`、`who system`、无操作按钮/回复引用。
5. 回复引用：`.reply-quote` 内容和正常消息操作区。
6. 长文本：完整正文、`a.msg-link`、`span.mention`、`.body` class（CSS 未改）。
7. 回调：回复、复制、picker emoji、reaction row 分别调用 `onReply`/`onCopy`/`onReact`。

## 验收与证据

在 `frontend/`：

```text
npm test
→ Test Files 10 passed (10)
→ Tests 47 passed (47)

npm run typecheck
→ 通过（无输出错误）

npm run build
→ ✓ built in 16.28s
```

在仓库根目录：

```text
scripts/check.sh
→ 结果：16 PASS / 0 FAIL / 2 SKIP
```

跳过项为未配置 ESLint、未安装 cargo-audit；Rust、前端、端到端集成和密钥扫描均通过。

UI 冒烟（新 `dist` 复制到 `release/frontend`，测试环境用户 `alice`，SFU 房间 `voicecheck`）：

```text
node test-server/ui-voice-check.mjs --base-url https://localhost:8081 \
  --user alice --pass 'correct-horse-battery' --room voicecheck --timeout 60
→ UI voice smoke: PASS
→ login: alice (room /voicecheck)
→ media-token: POST 200
→ /rtc/: 101
→ pc: connected
→ publish: outbound audio RTP 9 packets
→ leave: PC closed; LiveKit WS closed
→ console: 0 errors (ignored 2 pre-login /api/v1/me 401)
```

说明：此冒烟验证 Room 在抽取后仍能登录、进入房间并完成语音链路；消息条目交互由上述 RTL 用例覆盖。TLS 冒烟使用测试脚本内置的 `--ignore-certificate-errors`，不作为生产 CA 验证。

## 提交

- `41847be` refactor(room): extract message list component
  - 3 files changed, 327 insertions(+), 112 deletions(-)
  - 实现 MessageList/MessageItem、Room 接线和 7 个 RTL 测试

## 回退

回退该提交即可恢复 Room 内联消息渲染；无样式、schema、API、WS 或运行时数据变更。删除 `MessageList.tsx` / `MessageList.test.tsx`，恢复 `Room.tsx` 原 import 与消息 JSX。
