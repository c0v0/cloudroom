# Close check.sh gate gaps（2026-10-22）

- 分支：`chore/close-gate-gaps`
- 基线：`main` @ `b9ed77c` / `baseline-20261021-14`

## 验收标准与结果

| # | 验收标准 | 命令 | 结果 |
|---|---|---|---|
| 1 | ESLint devDependencies 安装并提交 lockfile | `npm install --save-dev eslint typescript-eslint eslint-plugin-react-hooks @eslint/js globals` | PASS |
| 2 | js/ts 推荐规则 + react-hooks；exhaustive-deps=error | `npm run lint` | PASS（0 problems） |
| 3 | 记录初次 lint 问题数并修复 | 初次 `npm run lint` | 9 problems（6 errors / 3 warnings），已清零 |
| 4 | Room joinVoice/leaveVoice 无 exhaustive-deps 警告 | `npx eslint src/pages/Room.tsx --max-warnings=0` | PASS |
| 5 | ESLint 步骤强制运行，缺配置/警告即失败 | `scripts/check.sh` 第 7 步 | PASS |
| 6 | cargo-audit 安装并扫描两个锁文件 | `cargo audit --file backend/Cargo.lock && cargo audit --file frontend-server/Cargo.lock` | PASS（仅 warning） |
| 7 | 全门禁 | `scripts/check.sh` | **18 PASS / 0 FAIL / 0 SKIP** |
| 8 | 发布静态构建回归 | `scripts/build-musl.sh all` | PASS |

## ESLint 初次问题与修复

初次输出：`9 problems (6 errors, 3 warnings)`。

- `useLiveKitVoice.ts`：给两个重新抛出的错误补 `cause: e`；`join`/`rebuildMic` 补 `effBitrate`。
- `useP2PVoice.ts`：`join` 补 `openMicrophone` 依赖（该 callback 本身稳定）。
- `useVoiceSession.ts` / `Admin.tsx` / `Room.tsx`：删除未使用的 exhaustive-deps disable。
- `Room.tsx`：`startDirect` 的依赖改为 `[p]`，消除缺失依赖；`joinVoice` 的旧 disable 删除后仍无警告。
- 新增 `frontend/eslint.config.js`：`@eslint/js` recommended、`typescript-eslint` recommended、browser/vitest globals、`react-hooks/rules-of-hooks=error`、`react-hooks/exhaustive-deps=error`。
- `frontend/package.json` 增加 `"lint": "eslint src --max-warnings=0"`。

## cargo-audit

- 安装 `cargo-audit 0.22.2`（`cargo install cargo-audit --locked`；全局工具，约 20 分钟编译）。
- 初次扫描发现 backend 12 个漏洞、frontend-server 6 个漏洞；升级：
  - `h2 0.4.15 → 0.4.16`
  - `quinn-proto 0.11.16 → 0.11.18`
  - `rustls 0.23.43 → 0.23.46`
  - `rustls-webpki 0.103.13 → 0.103.16`
  - `webpki-root-certs 1.0.9 → 1.0.10`
  - backend 的 `aws-lc-rs/aws-lc-sys` 随 rustls 升级
- 两个锁文件均通过 audit；保留的非阻塞 warning：`chacha20 0.10.1 yanked`、`rustls-pemfile 2.2.0 unmaintained`。
- `scripts/check.sh` 的 audit gate 同时扫描 `backend/Cargo.lock` 与 `frontend-server/Cargo.lock`；未安装时输出明确写“未执行”。

## 提交列表

- `chore(lint): add eslint with react-hooks rules`
- `chore(ci): enable cargo-audit in check`

## 关键命令输出

```text
$ npm run lint
（0 problems）

$ npm test
Test Files  13 passed (13)
Tests       55 passed (55)

$ cargo audit --file backend/Cargo.lock
warning: 1 allowed warning found
（exit 0）

$ cargo audit --file frontend-server/Cargo.lock
warning: 1 allowed warning found
（exit 0）

$ scripts/check.sh
结果：18 PASS / 0 FAIL / 0 SKIP
```

## 遗留与回退

- 遗留：`cargo-audit` 是用户级全局工具，干净机器需先安装；`check.sh` 未安装时明确输出 SKIP，不会静默通过。
- 回退：回退两个提交并恢复锁文件；ESLint 配置/依赖与 lint 修复、audit 配置和依赖升级分别随对应提交回退。
