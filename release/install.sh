#!/usr/bin/env bash
# CloudRoom 一键安装脚本（Debian/Ubuntu）
# 安装 CloudRoom 后端 + 前端静态文件 + LiveKit 媒体服务（二进制/配置/systemd 模板）。
# 用法：
#   sudo ./install.sh                  # 使用本目录 config.toml.example 生成配置
#   sudo ./install.sh /path/config.toml  # 使用自定义配置
#
# 脚本只做：系统用户/目录、二进制、前端文件、配置文件、systemd 服务。
# 已有配置不会被覆盖；含 CHANGE_ME 占位符时不会自动启动未配置好的服务，
# 编辑密钥/域名后仍需手动 systemctl enable --now。
# 完成后按文档配置 Nginx（域名/证书）与防火墙。
set -euo pipefail

SRC_DIR="$(cd "$(dirname "$0")" && pwd)"
CONFIG_SRC="${1:-$SRC_DIR/config.toml.example}"

if [[ $EUID -ne 0 ]]; then
  echo "请用 root 运行: sudo ./install.sh" >&2
  exit 1
fi

echo "==> 1/8 创建系统用户和目录"
id -u cloudroom >/dev/null 2>&1 || useradd --system --home /var/lib/cloudroom --shell /usr/sbin/nologin cloudroom
id -u livekit >/dev/null 2>&1 || useradd --system --home /var/lib/livekit --shell /usr/sbin/nologin livekit
install -d -o cloudroom -g cloudroom -m 0750 /var/lib/cloudroom /var/lib/cloudroom/avatars
install -d -o root -g root -m 0755 /etc/cloudroom
install -d -o cloudroom -g cloudroom -m 0750 /run/cloudroom
install -d -o www-data -g www-data -m 0755 /var/www/cloudroom

echo "==> 2/8 安装后端二进制（静态 musl，无运行依赖）"
if [[ -f /usr/local/bin/cloudroom-server ]]; then
  # 保留上一版本，升级失败可手动回滚：cp .prev 回去后 systemctl restart。
  cp -p /usr/local/bin/cloudroom-server /usr/local/bin/cloudroom-server.prev
  echo "    已备份上一版本到 /usr/local/bin/cloudroom-server.prev"
fi
install -m 0750 "$SRC_DIR/cloudroom-server" /usr/local/bin/cloudroom-server
chown root:cloudroom /usr/local/bin/cloudroom-server
chmod 0750 /usr/local/bin/cloudroom-server

echo "==> 3/8 安装 LiveKit 媒体服务二进制"
if [[ -x "$SRC_DIR/livekit-server" ]]; then
  if [[ -f "$SRC_DIR/livekit-server.sha256" ]]; then
    if ! ( cd "$SRC_DIR" && sha256sum -c livekit-server.sha256 >/dev/null 2>&1 ); then
      echo "    livekit-server 校验和失败，拒绝安装（请重新下载官方二进制）。" >&2
      exit 1
    fi
    echo "    livekit-server 校验和通过"
  else
    echo "    未提供 livekit-server.sha256，跳过校验（建议随发布包附带）。"
  fi
  install -m 0755 "$SRC_DIR/livekit-server" /usr/local/bin/livekit-server
  chown root:livekit /usr/local/bin/livekit-server
else
  echo "    未找到 release/livekit-server，跳过（请自行安装 LiveKit 并从 GitHub Releases 下载）。"
fi

echo "==> 4/8 安装前端静态文件"
if command -v rsync >/dev/null 2>&1; then
  # --delete：移除旧 hash 资源，避免旧包继续可访问。
  rsync -a --delete "$SRC_DIR/frontend/" /var/www/cloudroom/
else
  rm -rf /var/www/cloudroom/*
  cp -r "$SRC_DIR/frontend/." /var/www/cloudroom/
fi
chown -R www-data:www-data /var/www/cloudroom
# certbot webroot（nginx 模板里的 ACME 路径）。
install -d -o www-data -g www-data -m 0755 /var/www/certbot

echo "==> 5/8 安装 CloudRoom 配置文件"
if [[ -f /etc/cloudroom/config.toml ]]; then
  echo "    /etc/cloudroom/config.toml 已存在，跳过（如需覆盖请手动处理）。"
else
  install -m 0640 -o root -g cloudroom "$CONFIG_SRC" /etc/cloudroom/config.toml
  echo "    已生成 /etc/cloudroom/config.toml，请编辑 public_origin 与 [media] 的 LiveKit 键。"
fi

echo "==> 6/8 安装 LiveKit 配置文件"
if [[ -f /etc/livekit.yaml ]]; then
  echo "    /etc/livekit.yaml 已存在，跳过（如需覆盖请手动处理）。"
elif [[ -f "$SRC_DIR/livekit/livekit.yaml.example" ]]; then
  install -m 0640 -o root -g livekit "$SRC_DIR/livekit/livekit.yaml.example" /etc/livekit.yaml
  echo "    已生成 /etc/livekit.yaml，请编辑 keys 与域名（含 CHANGE_ME 时不建议直接启动）。"
fi

echo "==> 7/8 安装 systemd 服务模板"
if [[ -f /etc/systemd/system/cloudroom-server.service ]]; then
  echo "    已存在 cloudroom-server.service，跳过覆盖（新模板写入 .example）。"
  install -m 0644 "$SRC_DIR/systemd/cloudroom-server.service" /etc/systemd/system/cloudroom-server.service.example
else
  install -m 0644 "$SRC_DIR/systemd/cloudroom-server.service" /etc/systemd/system/
fi
if [[ -f "$SRC_DIR/livekit/livekit-server.service.example" ]]; then
  if [[ -f /etc/systemd/system/livekit-server.service ]]; then
    install -m 0644 "$SRC_DIR/livekit/livekit-server.service.example" /etc/systemd/system/livekit-server.service.example
  else
    install -m 0644 "$SRC_DIR/livekit/livekit-server.service.example" /etc/systemd/system/livekit-server.service
  fi
fi
if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
  systemctl daemon-reload
  echo "    已注册服务。配置好密钥/域名后执行："
  echo "      sudo systemctl enable --now livekit-server"
  echo "      sudo systemctl enable --now cloudroom-server"
else
  echo "    未检测到 systemd（容器环境），请手动启动："
  echo "    sudo -u livekit /usr/local/bin/livekit-server --config /etc/livekit.yaml"
  echo "    sudo -u cloudroom /usr/local/bin/cloudroom-server --config /etc/cloudroom/config.toml"
fi

echo "==> 8/8 Nginx 站点（示例）"
if [[ -f /etc/nginx/sites-available/cloudroom.conf ]]; then
  install -m 0644 "$SRC_DIR/nginx/cloudroom.conf.example" /etc/nginx/sites-available/cloudroom.conf.example
  echo "    已存在 cloudroom.conf，跳过覆盖（新模板写入 cloudroom.conf.example，请手动 diff）。"
else
  install -m 0644 "$SRC_DIR/nginx/cloudroom.conf.example" /etc/nginx/sites-available/cloudroom.conf
  echo "    示例已复制到 /etc/nginx/sites-available/cloudroom.conf"
fi
echo "    下一步（见 release/deployment-zh.md）:"
echo "      1) 修改 server_name / 证书路径"
echo "      2) ln -s /etc/nginx/sites-available/cloudroom.conf /etc/nginx/sites-enabled/"
echo "      3) nginx -t && systemctl reload nginx"
echo "      4) 签发证书: certbot --nginx -d chat.example.com"
echo ""
echo "安装完成。查看首次引导链接："
echo "  sudo cat /var/lib/cloudroom/bootstrap-url"
echo "（bootstrap URL 见数据目录下的 bootstrap-url 文件）"
