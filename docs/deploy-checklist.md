# 部署验收清单

按顺序执行；每步的 `<...>` 替换为实际域名/路径。未通过不要进入下一步。

## 1. 证书链与 TLS

### 1.1 本地验证 leaf + intermediate 链

准备 `root-ca.pem`、`intermediate-ca.pem`、`leaf.pem`、`leaf.key`；leaf 的 SAN 必须包含访问名（本地示例为 `localhost`）。

```bash
openssl verify -CAfile root-ca.pem -untrusted intermediate-ca.pem leaf.pem
```

期望输出：

```text
leaf.pem: OK
```

组装链并启动前端服务器（后端需同时在 `8080` 提供 `/api/v1/health`）：

```bash
cat leaf.pem intermediate-ca.pem > fullchain.pem
./release/cloudroom-frontend-server \
  --host 127.0.0.1 --port 8443 \
  --root release/frontend \
  --target http://127.0.0.1:8080 \
  --tls-cert fullchain.pem --tls-key leaf.key
```

另开终端验证完整链，不使用 `-k`/`--ignore-certificate-errors`：

```bash
curl --cacert root-ca.pem -fsS https://localhost:8443/api/v1/health
```

期望：HTTP 200，响应包含 `"status":"ok"`；`openssl s_client` 能看到 leaf + intermediate，且 `Verify return code: 0 (ok)`：

```bash
openssl s_client -connect localhost:8443 -servername localhost -showcerts </dev/null
```

### 1.2 生产 TLS

```bash
openssl s_client -connect <domain>:443 -servername <domain> -showcerts </dev/null
curl -fsS https://<domain>/api/v1/health
```

期望：证书链验证 `Verify return code: 0 (ok)`；健康检查返回 `{"status":"ok",...}`；浏览器无证书/混合内容告警，WSS 与控制面同源。

## 2. Nginx

```bash
sudo nginx -t
sudo systemctl reload nginx
curl -fsS https://<domain>/api/v1/health
curl -fsS https://<domain>/api/v1/config/public
```

期望：`nginx: configuration file ... test is successful`；两个请求均 HTTP 200，配置响应包含 `"voice":true` 与 `"voice_modes":["p2p","sfu"]`。确认 Nginx 只代理 HTTP/WSS，不代理 WebRTC UDP。

## 3. systemd

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now cloudroom-server
systemctl is-active cloudroom-server
systemctl is-active livekit-server
journalctl -u cloudroom-server -n 30 --no-pager
```

期望：两个 `is-active` 都输出 `active`；日志无持续重启/启动校验错误。首次启动应出现一次性 bootstrap 注册链接（若已有 super_admin 则不再出现）。

## 4. `release/install.sh`

```bash
bash -n release/install.sh
sudo ./release/install.sh
sudo systemctl status cloudroom-server --no-pager
curl -fsS https://<domain>/api/v1/health
```

期望：安装退出码 0；服务 `active`；健康检查 200。`CHANGE_ME` 未替换时脚本会拒绝自动启动，先编辑 `/etc/cloudroom/config.toml` 与 `/etc/livekit.yaml`。

## 5. 历史 owner 数据修正

**先备份；只在部署库副本或已确认备份后执行。** 统计不一致：

```bash
export DB=/var/lib/cloudroom/cloudroom.db
sqlite3 "$DB" "SELECT COUNT(*) AS owner_role_mismatches FROM room_members m JOIN rooms r ON r.room_id=m.room_id WHERE m.room_role='owner' AND r.kind='standard' AND r.owner_user_id IS NOT NULL AND m.user_id<>r.owner_user_id;"
sqlite3 "$DB" "SELECT room_id, room_code FROM rooms WHERE kind='standard' AND owner_user_id IS NULL;"
```

期望：第一项为 `0`；若有 `owner_user_id IS NULL` 的房，逐条人工处理，不自动猜测 owner。

若不等于 0，按 `docs/plans/2026-10-21-owner-transfer.md` 的备份/更新/回滚 SQL 执行，然后验证只有一个 active owner：

```bash
sqlite3 "$DB" "SELECT room_id, COUNT(*) FROM room_members WHERE room_role='owner' AND left_at IS NULL AND room_id IN (SELECT room_id FROM rooms WHERE kind='standard') GROUP BY room_id HAVING COUNT(*) <> 1;"
```

期望：无输出。直接房不参与此检查。

## 6. 真实麦克风

`ui-voice-check.mjs` 使用 Chrome fake device，不能证明真实设备。手动验证：

1. 用真实 Chrome（非 headless）打开 `https://<domain>/<room>`，登录并加入语音。
2. 允许麦克风权限，正常说话；观察本机/远端音量与音频。
3. 打开 `chrome://webrtc-internals`，确认音频 outbound RTP 增长、无持续 ICE/DTLS 失败。
4. 两个账号做回声测试：A 外放、B 说话，确认 A 的语音栏没有“回声消除不可用”提示；分别切换 A 的设置“回声消除”开/关，B 不应在开启时听到明显自回声，关闭时应能复现。
5. 保持 A 的 rnnoise 模式重复第 4 步，确认 rnnoise 只降噪、不改变 AEC 开关语义。

期望：权限提示正常；可听到本端与远端音频；A 实际获得 AEC 时无“回声消除不可用”警告；AEC 开启时 B 自回声明显降低，关闭时可复现；`webrtc-internals` 无持续重连、无静音断流；浏览器控制台无错误。

## 通过标准汇总

- 证书链验证为 `OK`，生产浏览器/curl 不使用跳过证书校验参数。
- Nginx、systemd、install.sh 均成功，健康检查 200。
- owner 数据不一致计数为 0，或已逐条人工记录。
- 真实麦克风可采集/发布/播放，无持续 ICE/DTLS 错误。
