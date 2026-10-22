# AEC 回声消除增强（2026-10-22）

- 分支：`fix/aec-echo-cancellation`；基线 tag：`baseline-20261022-4`
- 关联计划：本轮对话批准；目标为浏览器 AEC 生效校验、rnnoise 开关一致、失败提示，并评估更强插件。

## 验收标准与结果

| # | 验收标准 | 命令 | 结果 |
|---|---|---|---|
| 1 | 采集 constraints 正确，AEC 实际状态可判定 | `cd frontend && npx vitest run src/hooks/audioCapture.test.ts` | PASS（6 tests） |
| 2 | AEC 状态从 P2P/SFU 透传到会话 | `cd frontend && npx vitest run src/hooks/useVoiceSession.test.tsx` | PASS（2 tests） |
| 3 | 前端全量测试 | `cd frontend && npm test` | PASS（14 files / 61 tests） |
| 4 | 类型检查、构建、ESLint | `npm run typecheck && npm run build && npm run lint` | PASS |
| 5 | 本机 UI 语音冒烟 | `scripts/ui-smoke.sh` | PASS（PC connected、RTP 36 packets、0 console errors） |
| 6 | 全量门禁 | `scripts/check.sh` | **18 PASS / 0 FAIL / 0 SKIP** |
| 7 | 真实外放/耳机回声效果 | 手动按 `docs/deploy-checklist.md` 第 6 节 | 未验证（本机无 `/dev/snd`） |

## 改动

- 新增 `frontend/src/hooks/audioCapture.ts`：
  - `buildMicConstraints` 统一 P2P/SFU 的 `echoCancellation`、`noiseSuppression`、`autoGainControl`。
  - `ensureEchoCancellation` 用 `getSettings()` 校验实际 AEC，失败时尝试一次 `applyConstraints({echoCancellation:true})`。
  - `openMicrophone` 返回原始流和 `AecStatus`。
- `useP2PVoice.ts`、`useLiveKitVoice.ts`：
  - 接入统一采集辅助。
  - 修正 rnnoise 路径硬编码 `echoCancellation:true`，改为尊重 `aecRef.current`。
  - 暴露 `aecStatus`，离开语音时重置为 `unknown`。
- `useVoiceSession.ts`、`Room.tsx`：
  - 透传 `aecStatus`；不支持时显示“回声消除不可用”，不阻断进房。
- `Settings.tsx`、`docs/features.md`、`docs/deploy-checklist.md`：
  - 明确 rnnoise 只降噪、不消除回声；补真实设备双账号验收步骤。
- `useVoiceSession.test.tsx`：补 AEC 状态透传断言。

## 插件评估

硬门槛：开源/免费/可信 WASM，且实测效果优于当前浏览器 AEC。

- 候选 `@livekit/krisp-noise-filter`：`0.4.5`，许可证为 LiveKit Terms of Service（非开源/免费许可），解包体积约 12.5MB，定位为浏览器麦克风 track processor（噪声/语音隔离，不是专用 AEC）。
- 未找到满足许可条件且能被证明优于浏览器 AEC 的候选；没有真实音频设备，无法完成对比测量。
- **本轮不引入第三方插件，不改服务端媒体链路。** 浏览器 AEC 仍是主路径；后续若拿到真实设备，再按同一门槛重新评估。

## 关键命令输出

```text
$ npx vitest run src/hooks/audioCapture.test.ts src/hooks/useVoiceSession.test.tsx
Test Files  2 passed (2)
Tests       8 passed (8)

$ npm test
Test Files  14 passed (14)
Tests       61 passed (61)

$ scripts/ui-smoke.sh
UI voice smoke: PASS
media-token: POST 200
/rtc/: 101
pc: connected
publish: outbound audio RTP 36 packets
console: 0 errors

$ scripts/check.sh
结果：18 PASS / 0 FAIL / 0 SKIP
```

## 提交列表

- `18148d1` fix(voice): verify echo cancellation state

## 遗留与回退

- 遗留：真实外放/耳机回声效果需 D-017 设备验证；第三方 AEC/语音隔离插件未引入，等有真实设备且满足许可/效果门槛后再评估。
- 回退：回退 `18148d1` 即可恢复旧采集路径和 Room 提示；无数据库、API、WS 或服务端变更。
