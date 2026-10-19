//! cloudroom-server: self-hosted real-time room system backend.
//!
//! Static musl build: `cargo build --release --target x86_64-unknown-linux-musl`.

use cloudroom_server as lib;

use std::path::PathBuf;

use clap::Parser;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use lib::api::AppState;
use lib::config::{Config, ListenAddr};
use lib::error::ApiResult;

#[derive(Parser)]
#[command(
    name = "cloudroom-server",
    version,
    about = "Self-hosted real-time room system server"
)]
struct Args {
    /// Path to config.toml (default: /etc/cloudroom/config.toml, then ./config.toml)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Generate a new bootstrap registration link (local admin recovery).
    #[arg(long)]
    rotate_bootstrap: bool,
}

fn main() {
    let args = Args::parse();
    let cfg = load_config(args.config.as_deref());
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.log.level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    if args.rotate_bootstrap {
        runtime.block_on(run_rotate_bootstrap(&cfg));
        return;
    }
    runtime.block_on(run_server(cfg));
}

fn load_config(path: Option<&std::path::Path>) -> Config {
    let candidates: Vec<PathBuf> = match path {
        Some(p) => vec![p.to_path_buf()],
        None => vec![
            PathBuf::from("/etc/cloudroom/config.toml"),
            PathBuf::from("config.toml"),
        ],
    };
    for c in &candidates {
        if c.exists() {
            match Config::load(c) {
                Ok(cfg) => return cfg,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
    }
    eprintln!(
        "no config file found (looked at {}). Copy release/config.toml.example \
         (or docs/configuration.md) to one of those paths.",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    std::process::exit(1);
}

async fn run_rotate_bootstrap(cfg: &Config) {
    let data = &cfg.data.dir;
    std::fs::create_dir_all(data).expect("create data dir");
    let db = lib::db::Db::open(&data.join("cloudroom.db")).expect("open db");
    db.migrate().expect("migrate");
    let has_super = db
        .with_sync(|conn| lib::repo::super_admin_exists(conn))
        .expect("query");
    if has_super {
        eprintln!("a super_admin already exists; bootstrap is closed.");
        std::process::exit(1);
    }
    // Invalidate old tokens, mint a fresh one.
    db.with_sync(|conn| -> ApiResult<()> {
        conn.execute("DELETE FROM bootstrap_tokens", [])?;
        Ok(())
    })
    .expect("clear old tokens");
    let raw = lib::util::random_token();
    let expires =
        lib::util::now() + chrono::Duration::from_std(cfg.bootstrap_ttl()).unwrap_or_default();
    db.with_sync(|conn| -> ApiResult<()> {
        lib::repo::insert_bootstrap_token(conn, &lib::util::sha256_hex(raw.as_bytes()), &expires)?;
        Ok(())
    })
    .expect("insert token");
    println!("bootstrap URL (valid {}):", cfg.auth.bootstrap_ttl);
    println!("{}/register?bootstrap={raw}", cfg.server.public_origin);
}

async fn run_server(cfg: Config) {
    let data = cfg.data.dir.clone();
    if let Err(e) = std::fs::create_dir_all(&data) {
        eprintln!("cannot create data dir {}: {e}", data.display());
        std::process::exit(1);
    }
    std::fs::create_dir_all(data.join("avatars")).expect("create avatars dir");

    // Single-writer lock: refuse a second active server on the same data dir.
    let lock_path = data.join("instance.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open instance lock");
    use fs2::FileExt;
    if let Err(e) = lock_file.try_lock_exclusive() {
        eprintln!(
            "another cloudroom-server instance holds {} ({e}); refusing to start as a second writer",
            lock_path.display()
        );
        std::process::exit(1);
    }
    let _lock_guard = lock_file;

    let db = match lib::db::Db::open(&data.join("cloudroom.db")) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{}", e.message);
            std::process::exit(1);
        }
    };
    if let Err(e) = db.migrate() {
        eprintln!(
            "migration failed (old database left untouched): {}",
            e.message
        );
        std::process::exit(1);
    }

    // Bootstrap link on first start (no super_admin yet).
    let has_super = db
        .with_sync(|conn| lib::repo::super_admin_exists(conn))
        .unwrap_or(false);
    if !has_super {
        let raw = lib::util::random_token();
        let expires =
            lib::util::now() + chrono::Duration::from_std(cfg.bootstrap_ttl()).unwrap_or_default();
        let _ = db.with_sync(|conn| {
            lib::repo::insert_bootstrap_token(
                conn,
                &lib::util::sha256_hex(raw.as_bytes()),
                &expires,
            )?;
            Ok::<_, lib::error::ApiError>(())
        });
        println!("============================================================");
        println!("CloudRoom first-run bootstrap: no super_admin exists yet.");
        println!(
            "Register the first account at (valid {}):",
            cfg.auth.bootstrap_ttl
        );
        println!("  {}/register?bootstrap={raw}", cfg.server.public_origin);
        println!("The raw token is printed ONLY here, never logged.");
        println!("============================================================");
    }

    let cfg = Arc::new(cfg);
    let auth = Arc::new(lib::auth::Auth {
        cfg: (*cfg).clone(),
    });
    let state = AppState {
        db,
        cfg: cfg.clone(),
        auth,
        hub: Arc::new(lib::ws::Hub::new()),
        rl: Arc::new(lib::rate_limit::RateLimiter::new()),
        avatars_dir: data.join("avatars"),
    };

    let app = lib::api::use_state(state.clone());
    // Sweeper + rate-limiter pruning.
    let sweep_db = state.db.clone();
    let sweep_cfg = (*cfg).clone();
    tokio::spawn(lib::sweeper::run(sweep_db, sweep_cfg));
    let rl = state.rl.clone();
    tokio::spawn(async move {
        let mut t = tokio::time::interval(std::time::Duration::from_secs(600));
        loop {
            t.tick().await;
            rl.prune();
        }
    });

    let listen = cfg.listen_addr().expect("valid listen addr");
    let server: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>> =
        match &listen {
            ListenAddr::Tcp(addr) => {
                tracing::info!(%addr, "listening (loopback only; TLS terminates at Nginx)");
                let listener = tokio::net::TcpListener::bind(addr).await.expect("bind tcp");
                Box::pin(async move {
                    axum::serve(listener, app)
                        .with_graceful_shutdown(shutdown_signal())
                        .await
                })
            }
            ListenAddr::Unix(path) => {
                if path.exists() {
                    std::fs::remove_file(path).expect("remove stale unix socket");
                }
                tracing::info!(path = %path.display(), "listening on unix socket");
                let listener = tokio::net::UnixListener::bind(path).expect("bind unix socket");
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660));
                Box::pin(async move {
                    axum::serve(listener, app)
                        .with_graceful_shutdown(shutdown_signal())
                        .await
                })
            }
        };
    if let Err(e) = server.await {
        tracing::error!(error = %e, "server error");
        std::process::exit(1);
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("ctrl_c handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("sigterm handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}
