# 发版固定检查清单

每次发版按顺序执行；任何一步失败即停止，不发布。

## 0. 工作树与分支

```bash
git status --short --branch
git log --oneline -1
```

期望：分支正确、工作树干净；HEAD 是要发布的提交。

## 1. 依赖漏洞

```bash
cd backend && cargo audit --file Cargo.lock
cd ../frontend-server && cargo audit --file Cargo.lock
cd .. && npm --prefix frontend audit --omit=dev --audit-level=high
```

期望：三个命令 exit 0；无 vulnerability。`chacha20`/`rustls-pemfile` 的旧 warning 已由依赖升级消除；若审计数据库出现新 warning，先按 `docs/debt.md` 更新登记表再决定发布。

## 2. 测试与门禁

```bash
scripts/check.sh
```

期望：

```text
结果：18 PASS / 0 FAIL / 0 SKIP
```

如环境缺少 `cargo-audit`、Chrome 或 `release/livekit-server`，不得把 SKIP 当作通过；按 `docs/debt.md` 处理或明确记录降级。

可选本地 UI 语音冒烟（需 Chrome）：

```bash
scripts/ui-smoke.sh
```

期望：`UI voice smoke: PASS`，exit 0。

## 3. 发布构建

```bash
scripts/build-musl.sh all
```

期望：生成 `release/cloudroom-server`、`release/cloudroom-frontend-server`、`release/frontend/`；两个二进制为 static-pie + stripped。

## 4. 构建哈希与链接形态

```bash
sha256sum release/cloudroom-server release/cloudroom-frontend-server
file release/cloudroom-server release/cloudroom-frontend-server
readelf -d release/cloudroom-server | grep NEEDED || echo "no NEEDED"
readelf -l release/cloudroom-server | grep INTERP || echo "no INTERP"
readelf -d release/cloudroom-frontend-server | grep NEEDED || echo "no NEEDED"
readelf -l release/cloudroom-frontend-server | grep INTERP || echo "no INTERP"
```

期望：两个 `file` 输出 `static-pie linked, stripped`；四个 `readelf` 检查均无输出（打印 `no NEEDED` / `no INTERP`）；SHA-256 写入发版记录。

## 5. CHANGELOG

```bash
grep -n '^# Changelog' -A3 CHANGELOG.md
```

期望：最新一行覆盖本版用户可见变化、依赖/安全变化、门禁结果；破坏性变更已按 `docs/decisions.md` 写 ADR。

## 6. Tag

```bash
git tag -a v<version> -m "v<version>: <一句话>"
git show --no-patch --oneline --decorate v<version>
```

期望：annotated tag 指向本版提交；说明与 CHANGELOG 首行一致。不要重写已有 tag。

## 7. 已知限制

```bash
grep -n 'blocked\|accepted\|wontfix' docs/debt.md
```

期望：逐条确认 `blocked` 项的阻塞条件仍属实；`accepted`/`wontfix` 的理由仍成立。必须向部署方转述当前 blocked 项，尤其是生产 TLS、Nginx/systemd、真实麦克风与历史 owner 数据修正；不要把本地自签 TLS 或 fake device 结果写成生产验证。

## 8. 发布记录

记录以下内容到发布说明或 `docs/plans/`：

- 版本、tag、提交 SHA。
- `scripts/check.sh` 结果与跳过的步骤（如有）。
- 两个二进制的 SHA-256。
- `docs/debt.md` 中 blocked/accepted 项摘要。
- 是否执行 `scripts/ui-smoke.sh`，以及环境（fake device / 真实设备）。

发布前不要推送或发布产物；等明确批准后再执行。
