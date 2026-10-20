//! cloudroom-frontend-server: 单可执行文件前端托管服务器。
//!
//! 用途：没有 Nginx / Node 的环境里快速托管 CloudRoom 前端并联通后端。
//!   - 静态文件托管（SPA 路由回退到 index.html）
//!   - `/api/v1/*` HTTP 反代到 cloudroom-server（补 X-Forwarded-*）
//!   - `/api/v1/*` WebSocket 升级 = 原始 TCP 隧道（无需解析 WS 帧）
//!
//! 静态编译: cargo build --release --target x86_64-unknown-linux-musl

use std::convert::Infallible;
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use bytes::Bytes;
use clap::Parser;
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioExecutor;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Parser)]
#[command(
    name = "cloudroom-frontend-server",
    about = "CloudRoom 前端托管 + API/WS 反代（单文件）"
)]
struct Args {
    /// 监听端口
    #[arg(long, default_value_t = 8081)]
    port: u16,

    /// 监听地址（0.0.0.0 = 所有网卡；默认仅本机）
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// TLS 证书（PEM）。提供后以 HTTPS/WSS 运行。
    #[arg(long)]
    tls_cert: Option<String>,

    /// TLS 私钥（PEM）。与 --tls-cert 成对使用。
    #[arg(long)]
    tls_key: Option<String>,

    /// 前端静态文件目录（npm run build 的产物）
    #[arg(long, default_value = "frontend")]
    root: PathBuf,

    /// 后端地址（cloudroom-server）
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    target: String,

    /// LiveKit 信令地址（如 127.0.0.1:7880）。提供后 /rtc/* 的 WSS 升级
    /// 隧道到该地址，使 https 页面可用同源 wss:// 连接 LiveKit（无需其自带 TLS）。
    #[arg(long)]
    rtc_target: Option<String>,
}

trait IoStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Sync {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Sync> IoStream for T {}

#[derive(Clone)]
struct State {
    root: Arc<PathBuf>,
    target: Arc<String>,
    rtc_target: Arc<Option<String>>,
    is_tls: bool,
}

const MIME: &[(&str, &str)] = &[
    (".html", "text/html; charset=utf-8"),
    (".js", "text/javascript; charset=utf-8"),
    (".mjs", "text/javascript; charset=utf-8"),
    (".css", "text/css; charset=utf-8"),
    (".json", "application/json"),
    (".svg", "image/svg+xml"),
    (".png", "image/png"),
    (".jpg", "image/jpeg"),
    (".jpeg", "image/jpeg"),
    (".webp", "image/webp"),
    (".ico", "image/x-icon"),
    (".txt", "text/plain; charset=utf-8"),
    (".woff2", "font/woff2"),
    (".map", "application/json"),
];

fn mime_for(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    MIME.iter()
        .find(|(k, _)| *k == ext)
        .map(|(_, v)| *v)
        .unwrap_or("application/octet-stream")
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();

    let root = args.root.canonicalize().unwrap_or_else(|e| {
        eprintln!("静态目录不可用 {}: {e}", args.root.display());
        std::process::exit(1);
    });
    if !root.join("index.html").exists() {
        eprintln!(
            "{} 下没有 index.html（需要 npm run build 的产物）",
            root.display()
        );
        std::process::exit(1);
    }
    let is_tls = args.tls_cert.is_some() || args.tls_key.is_some();
    if args.tls_cert.is_some() != args.tls_key.is_some() {
        eprintln!("--tls-cert 和 --tls-key 必须成对提供");
        std::process::exit(1);
    }
    let state = Arc::new(State {
        root: Arc::new(root.clone()),
        target: Arc::new(args.target.clone()),
        rtc_target: Arc::new(args.rtc_target.clone()),
        is_tls,
    });

    let tls_config = match (&args.tls_cert, &args.tls_key) {
        (Some(cert), Some(key)) => {
            let certs: Vec<rustls_pemfile::Item> = rustls_pemfile::read_all(&mut BufReader::new(
                File::open(cert).unwrap_or_else(|e| {
                    eprintln!("读取证书失败 {cert}: {e}");
                    std::process::exit(1);
                }),
            ))
            .collect::<Result<_, _>>()
            .unwrap_or_else(|e| {
                eprintln!("解析证书失败: {e}");
                std::process::exit(1);
            });
            let certs: Vec<rustls::pki_types::CertificateDer<'static>> = certs
                .into_iter()
                .filter_map(|item| match item {
                    rustls_pemfile::Item::X509Certificate(c) => Some(c),
                    _ => None,
                })
                .collect();
            let key = rustls_pemfile::private_key(&mut BufReader::new(
                File::open(key).unwrap_or_else(|e| {
                    eprintln!("读取私钥失败 {key}: {e}");
                    std::process::exit(1);
                }),
            ))
            .unwrap_or_else(|e| {
                eprintln!("解析私钥失败: {e}");
                std::process::exit(1);
            })
            .ok_or_else(|| "私钥文件不含可用密钥".to_string())
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            let cfg = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .unwrap_or_else(|e| {
                    eprintln!("TLS 配置失败: {e}");
                    std::process::exit(1);
                });
            Some(Arc::new(cfg))
        }
        _ => None,
    };

    let ip: std::net::IpAddr = args.host.parse().unwrap_or_else(|e| {
        eprintln!("无效的 --host {}: {e}", args.host);
        std::process::exit(1);
    });
    let addr = SocketAddr::from((ip, args.port));
    let listener = TcpListener::bind(addr).await.unwrap_or_else(|e| {
        eprintln!("绑定 {addr} 失败: {e}");
        std::process::exit(1);
    });
    let scheme = if is_tls { "https" } else { "http" };
    tracing::info!(addr = %addr, root = %root.display(), target = %args.target,
        "前端服务器已启动: {scheme}://{}/", addr);

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                tracing::error!(error = %e, "accept 失败");
                continue;
            }
        };
        let st = state.clone();
        let tls = tls_config.clone();
        tokio::spawn(async move {
            let mut stream: std::pin::Pin<Box<dyn IoStream>> = match &tls {
                Some(cfg) => {
                    match tokio_rustls::TlsAcceptor::from(cfg.clone())
                        .accept(stream)
                        .await
                    {
                        Ok(s) => Box::pin(s),
                        Err(e) => {
                            tracing::debug!(error = %e, "TLS 握手失败");
                            return;
                        }
                    }
                }
                None => Box::pin(stream),
            };
            // 先读完整 HTTP 头：若是 /rtc/ 的 WebSocket 升级，说明目标是 LiveKit
            //（它需要自己完成 101 握手），把原始字节流整体转发；否则把读到的
            // 头部重放给 hyper 走正常 HTTP/升级代理流程。
            let mut head_buf: Vec<u8> = Vec::new();
            let mut tmp = [0u8; 4096];
            let head_end = loop {
                let n = match stream.read(&mut tmp).await {
                    Ok(n) => n,
                    Err(_) => return,
                };
                if n == 0 {
                    return;
                }
                head_buf.extend_from_slice(&tmp[..n]);
                if let Some(pos) = find_seq(&head_buf, b"\r\n\r\n") {
                    break pos + 4;
                }
                if head_buf.len() > 64 * 1024 {
                    return;
                }
            };
            if is_rtc_upgrade(&head_buf[..head_end]) {
                let Some(rtc) = st.rtc_target.as_ref().clone() else {
                    tracing::warn!("收到 /rtc/ 升级但没有配置 --rtc-target");
                    return;
                };
                let (host, port) = target_host_port(&rtc);
                let mut backend = match TcpStream::connect((host, port)).await {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(error = %e, "连接 LiveKit 信令失败");
                        return;
                    }
                };
                if backend.write_all(&head_buf[..head_end]).await.is_err() {
                    return;
                }
                if head_end < head_buf.len()
                    && backend.write_all(&head_buf[head_end..]).await.is_err()
                {
                    return;
                }
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut backend).await;
                return;
            }
            let prefixed = PrefixStream {
                prefix: Bytes::from(head_buf),
                inner: stream,
            };
            let stream = hyper_util::rt::TokioIo::new(prefixed);
            let svc = service_fn(move |req| handle(req, st.clone()));
            let builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
            if let Err(e) = builder.serve_connection_with_upgrades(stream, svc).await {
                tracing::debug!(error = %e, "连接结束");
            }
        });
    }
}

type Resp = Response<Full<Bytes>>;

fn ok_body(content_type: &str, cache: &str, data: Bytes) -> Resp {
    let mut r = Response::new(Full::new(data));
    *r.status_mut() = StatusCode::OK;
    r.headers_mut()
        .insert("content-type", HeaderValue::from_str(content_type).unwrap());
    r.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_str(cache).unwrap());
    r
}

async fn handle(req: Request<Incoming>, st: Arc<State>) -> Result<Resp, Infallible> {
    let is_ws = req
        .headers()
        .get(http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    if is_ws {
        // 关键时序（WSS 模式尤其重要）：
        //  1) 后端连接任务先行：向后端发 upgrade，确认其返回 101，只保留
        //     101 之后的帧数据（后端的 101 响应头绝不转发——hyper 已替
        //     客户端完成 101 握手，再发一份会让浏览器报 Invalid frame header）。
        //  2) service 立即返回 101；hyper 写完它并移交连接后，隧道任务
        //     await 到 upgraded 流，把帧数据写过去再双向 copy。
        let ws_key = req
            .headers()
            .get("sec-websocket-key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        let path_and_query = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
            .to_string();
        let pass_headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .filter(|(k, _)| {
                let key = k.as_str().to_ascii_lowercase();
                !matches!(
                    key.as_str(),
                    "host" | "connection" | "upgrade" | "content-length" | "transfer-encoding"
                ) && (key.starts_with("sec-websocket") || key == "cookie" || key == "origin")
            })
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    v.to_str().unwrap_or("").to_string(),
                )
            })
            .collect();
        let st2 = st.clone();
        // /rtc/* 的升级请求是 LiveKit 信令（经 --rtc-target 反代），其余走后端。
        let upstream = if path_and_query.starts_with("/rtc/") {
            st.rtc_target
                .as_ref()
                .clone()
                .unwrap_or_else(|| st.target.to_string())
        } else {
            st.target.to_string()
        };
        let (tx, rx) =
            tokio::sync::oneshot::channel::<Result<(tokio::net::TcpStream, Vec<u8>), String>>();
        tokio::spawn(async move {
            let r = setup_backend_ws(&st2, &path_and_query, &pass_headers, &upstream).await;
            let _ = tx.send(r);
        });
        let on_upgrade = hyper::upgrade::on(req);
        tokio::spawn(async move {
            let (mut backend, rest) = match rx.await {
                Ok(Ok(b)) => b,
                _ => {
                    tracing::warn!(
                        "后端 WebSocket 拒绝或连接失败（客户端侧 101 已发出，连接关闭）"
                    );
                    return;
                }
            };
            let mut upgraded = hyper_util::rt::TokioIo::new(match on_upgrade.await {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(error = %e, "hyper 未移交升级连接");
                    return;
                }
            });
            if !rest.is_empty() && upgraded.write_all(&rest).await.is_err() {
                return;
            }
            let _ = tokio::io::copy_bidirectional(&mut upgraded, &mut backend).await;
        });
        let mut resp = Response::new(Full::new(Bytes::new()));
        *resp.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
        resp.headers_mut()
            .insert("upgrade", HeaderValue::from_static("websocket"));
        resp.headers_mut()
            .insert("connection", HeaderValue::from_static("upgrade"));
        // hyper 只按我们给的响应头发送：必须自行计算 Sec-WebSocket-Accept
        if let Some(k) = ws_key {
            use sha1::Digest as _;
            let mut h = sha1::Sha1::new();
            h.update(k.as_bytes());
            h.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
            let accept = base64::engine::general_purpose::STANDARD.encode(h.finalize());
            resp.headers_mut().insert(
                "sec-websocket-accept",
                HeaderValue::from_str(&accept).unwrap(),
            );
        }
        return Ok(resp);
    }
    if req.uri().path().starts_with("/api/v1/") {
        return proxy_http(req, &st).await;
    }
    serve_static(req.uri().path(), &st).await
}

// ------------------------------------------------------------- 静态文件

async fn serve_static(path: &str, st: &State) -> Result<Resp, Infallible> {
    let rel = path.trim_start_matches('/');
    let file = st.root.join(rel);
    // 防穿越：逐段规范化（拒绝 .. 逃逸）
    let mut norm = PathBuf::new();
    for comp in file.components() {
        match comp {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            other => norm.push(other.as_os_str()),
        }
    }
    if !norm.starts_with(st.root.as_path()) {
        return Ok(ok_body(
            "text/plain; charset=utf-8",
            "no-cache",
            Bytes::from_static(b"forbidden"),
        ));
    }
    if norm.is_dir() {
        norm = norm.join("index.html");
    }
    match fs::read(&norm).await {
        Ok(data) => {
            let cache = if path.starts_with("/assets/") {
                "public, max-age=3600"
            } else {
                "no-cache"
            };
            Ok(ok_body(mime_for(&norm), cache, Bytes::from(data)))
        }
        Err(_) => {
            // SPA 回退：房间 URL（/abc1234）等未知路径返回 index.html
            match fs::read(st.root.join("index.html")).await {
                Ok(idx) => Ok(ok_body(
                    "text/html; charset=utf-8",
                    "no-cache",
                    Bytes::from(idx),
                )),
                Err(_) => Ok(ok_body(
                    "text/plain; charset=utf-8",
                    "no-cache",
                    Bytes::from_static(b"not found"),
                )),
            }
        }
    }
}

// ------------------------------------------------------------- HTTP 反代

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

fn target_host_port(target: &str) -> (&str, u16) {
    let t = target
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    match t.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().unwrap_or(80)),
        None => (t, 80),
    }
}

async fn proxy_http(req: Request<Incoming>, st: &State) -> Result<Resp, Infallible> {
    let (host, port) = target_host_port(&st.target);
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");

    let mut builder = Request::builder()
        .method(req.method())
        .uri(format!("http://{host}:{port}{path_and_query}"));
    let mut hop_free = HeaderMap::new();
    for (k, v) in req.headers() {
        if HOP_BY_HOP.contains(&k.as_str()) {
            continue;
        }
        hop_free.insert(k.clone(), v.clone());
    }
    hop_free.insert(
        http::header::HOST,
        HeaderValue::from_str(&format!("{host}:{port}")).unwrap(),
    );
    hop_free.insert(
        "x-forwarded-proto",
        HeaderValue::from_static(if st.is_tls { "https" } else { "http" }),
    );
    for (k, v) in &hop_free {
        builder = builder.header(k.clone(), v.clone());
    }
    let upstream_req = builder.body(req.into_body()).unwrap();

    let client = hyper_util::client::legacy::Client::builder(TokioExecutor::new())
        .build::<_, Incoming>(hyper_util::client::legacy::connect::HttpConnector::new());

    match client.request(upstream_req).await {
        Ok(up_res) => {
            let mut clean = HeaderMap::new();
            for (k, v) in up_res.headers() {
                if !HOP_BY_HOP.contains(&k.as_str()) {
                    clean.insert(k.clone(), v.clone());
                }
            }
            let mut resp = Response::new(Full::new(Bytes::new()));
            *resp.status_mut() = up_res.status();
            *resp.headers_mut() = clean;
            let body = match http_body_util::BodyExt::collect(up_res.into_body()).await {
                Ok(b) => b.to_bytes(),
                Err(e) => {
                    tracing::error!(error = %e, "读取上游响应失败");
                    Bytes::new()
                }
            };
            *resp.body_mut() = Full::new(body);
            Ok(resp)
        }
        Err(e) => {
            tracing::error!(error = %e, "上游请求失败");
            let mut r = Response::new(Full::new(Bytes::from(format!("upstream error: {e}"))));
            *r.status_mut() = StatusCode::BAD_GATEWAY;
            Ok(r)
        }
    }
}

// ------------------------------------------------------------- WS 隧道

/// 连接后端并完成后端侧 upgrade 握手。
/// 成功返回 (backend 流, 101 响应头之后已读到的帧数据)；后端拒绝返回 Err。
async fn setup_backend_ws(
    _st: &State,
    path_and_query: &str,
    pass_headers: &[(String, String)],
    upstream: &str,
) -> Result<(TcpStream, Vec<u8>), String> {
    let (host, port) = target_host_port(upstream);
    let mut backend = TcpStream::connect((host, port))
        .await
        .map_err(|e| e.to_string())?;
    let mut head = format!("GET {path_and_query} HTTP/1.1\r\n");
    head.push_str(&format!("Host: {host}:{port}\r\n"));
    head.push_str("Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\n");
    for (k, v) in pass_headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    backend
        .write_all(head.as_bytes())
        .await
        .map_err(|e| e.to_string())?;

    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        let n = backend.read(&mut tmp).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("后端提前关闭连接".into());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_seq(&buf, b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            return Err("响应头过大".into());
        }
    };
    if !String::from_utf8_lossy(&buf[..head_end]).contains(" 101 ") {
        return Err(format!(
            "后端拒绝升级: {}",
            String::from_utf8_lossy(&buf[..head_end])
                .lines()
                .next()
                .unwrap_or("")
        ));
    }
    // 只返回 101 之后的帧数据；后端 101 响应头由 hyper 的客户端侧 101 替代
    let rest = buf[head_end..].to_vec();
    Ok((backend, rest))
}

fn find_seq(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 判断 HTTP 头是否为 LiveKit 信令升级（GET /rtc/ + Upgrade: websocket）。
fn is_rtc_upgrade(head: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    let mut lines = text.split("\r\n");
    let Some(request_line) = lines.next() else {
        return false;
    };
    if !request_line.starts_with("GET /rtc/") {
        return false;
    }
    lines.any(|l| l.to_ascii_lowercase() == "upgrade: websocket")
}

/// 把已读到的 HTTP 头重放给下游（hyper）的流包装。
struct PrefixStream<S> {
    prefix: Bytes,
    inner: S,
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> tokio::io::AsyncRead
    for PrefixStream<S>
{
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if !self.prefix.is_empty() {
            let n = std::cmp::min(buf.remaining(), self.prefix.len());
            buf.put_slice(&self.prefix[..n]);
            self.prefix = self.prefix.slice(n..);
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite
    for PrefixStream<S>
{
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
