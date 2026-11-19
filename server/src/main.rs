//! nrzBackup server: other apps push files (HTTP or MCP) or nrzBackup asks them for one
//! (HTTP or MCP, on a schedule or on request). Every file is spooled locally only until it
//! is encrypted and stored in Filen.io; old copies are thinned out per source
//! (last / daily / weekly / monthly / yearly). A small MCP server reports the state to
//! nrzDashboard.
//!
//! Environment:
//! - `NRZBACKUP_ADDR` (default `0.0.0.0:8080`)
//! - `NRZBACKUP_DATA` (default `./data`): database and spool folder
//! - `NRZBACKUP_PUBLIC` (default `./public`): the built web app
//! - `NRZBACKUP_PASSWORD`: password of the web UI (umbrelOS: `APP_PASSWORD`)
//! - `NRZBACKUP_SECRET`: seals stored secrets (umbrelOS: `APP_SEED`)
//! - `NRZBACKUP_PUBLIC_URL`: default address for other apps (default `http://nrz-backup_app_1:8080`)
//! - `TZ`: time zone for schedules and retention (default `Europe/Zurich`)

mod api;
mod db;
mod filen;
mod local;
mod masterkey;
mod mcp;
mod mcp_client;
mod prompt;
mod pull;
mod retention;
mod secret;
mod stats;
mod status;
mod umbrel;
mod worker;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use chrono_tz::Tz;
use nrzbackup_shared::{FilenStatus, Retention};
use tokio::sync::{Notify, RwLock};
use tower_http::services::{ServeDir, ServeFile};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct AppState {
    pub db: db::Db,
    pub filen: filen::Filen,
    pub secrets: secret::SecretBox,
    pub spool_dir: PathBuf,
    /// SHA-256 of the UI password.
    pub password_hash: String,
    pub tz: Tz,
    pub default_public_url: String,
    /// Wakes the upload worker.
    pub wake: Notify,
    pub session: RwLock<Option<filen::Session>>,
    pub filen_status: RwLock<FilenStatus>,
    /// Sources that are being pulled right now.
    pub pulling: Mutex<HashSet<i64>>,
    /// HTTP client for pulling from other apps (no total timeout: backups can be big).
    pub http: reqwest::Client,
}

impl AppState {
    pub fn root_folder(&self) -> String {
        self.db
            .get("root_folder")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "nrzBackup".into())
    }

    pub fn default_retention(&self) -> Retention {
        self.db
            .get("default_retention")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn public_url(&self) -> String {
        self.db
            .get("public_url")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| self.default_public_url.clone())
            .trim_end_matches('/')
            .to_string()
    }

    pub fn delete_permanently(&self) -> bool {
        self.db.get("delete_permanently").as_deref() != Some("0")
    }

    pub async fn filen_session(&self) -> Option<filen::Session> {
        self.session.read().await.clone()
    }
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

#[tokio::main]
async fn main() {
    let addr: SocketAddr = env("NRZBACKUP_ADDR")
        .unwrap_or_else(|| "0.0.0.0:8080".into())
        .parse()
        .expect("NRZBACKUP_ADDR");

    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        let port = addr.port();
        let ok = std::net::TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], port)),
            std::time::Duration::from_secs(3),
        )
        .is_ok();
        std::process::exit(if ok { 0 } else { 1 });
    }

    let data_dir = PathBuf::from(env("NRZBACKUP_DATA").unwrap_or_else(|| "./data".into()));
    let public_dir = PathBuf::from(env("NRZBACKUP_PUBLIC").unwrap_or_else(|| "./public".into()));
    let spool_dir = data_dir.join("spool");
    std::fs::create_dir_all(&spool_dir).expect("cannot create data folder");

    let db = db::Db::open(&data_dir.join("nrzbackup.db")).expect("cannot open database");

    let seed = match env("NRZBACKUP_SECRET") {
        Some(s) => s,
        None => {
            // Development: a random secret in the data folder.
            let path = data_dir.join("secret.key");
            match std::fs::read_to_string(&path) {
                Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => {
                    let s = secret::new_token("");
                    let _ = std::fs::write(&path, &s);
                    eprintln!(
                        "nrzBackup: NRZBACKUP_SECRET not set – using {}",
                        path.display()
                    );
                    s
                }
            }
        }
    };
    let password = env("NRZBACKUP_PASSWORD").unwrap_or_else(|| {
        eprintln!("nrzBackup: NRZBACKUP_PASSWORD not set – the password is \"nrzbackup\"");
        "nrzbackup".into()
    });
    let tz: Tz = env("TZ")
        .and_then(|t| t.parse().ok())
        .unwrap_or(chrono_tz::Europe::Zurich);

    let secrets = secret::SecretBox::new(seed.as_bytes());
    let session = db
        .get("filen_session")
        .and_then(|s| secrets.open(&s))
        .and_then(|s| serde_json::from_str::<filen::Session>(&s).ok());
    let filen_status: FilenStatus = db
        .get("filen_status")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    db.reset_uploading();

    let state = Arc::new(AppState {
        db,
        filen: filen::Filen::new(),
        secrets,
        spool_dir,
        password_hash: secret::token_hash(password.trim()),
        tz,
        default_public_url: env("NRZBACKUP_PUBLIC_URL")
            .unwrap_or_else(|| "http://nrz-backup_app_1:8080".into()),
        wake: Notify::new(),
        session: RwLock::new(session),
        filen_status: RwLock::new(filen_status),
        pulling: Mutex::new(HashSet::new()),
        http: reqwest::Client::builder()
            .user_agent(concat!("nrzBackup/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("http client"),
    });
    worker::cleanup_spool(&state);
    masterkey::ensure(&state);
    state.db.event(
        nrzbackup_shared::EventLevel::Info,
        None,
        &format!("nrzBackup {VERSION} gestartet"),
    );

    tokio::spawn(worker::upload_loop(state.clone()));
    tokio::spawn(worker::scheduler_loop(state.clone()));
    tokio::spawn(worker::quota_loop(state.clone()));

    let index = public_dir.join("index.html");
    let ui = Router::new()
        .route("/api/session", get(api::session))
        .route("/api/login", post(api::login))
        .route("/api/logout", post(api::logout))
        .route("/api/masterkey/reveal", post(api::reveal_masterkey))
        .route("/api/masterkey/saved", post(api::masterkey_saved))
        .route("/api/stats", get(api::get_stats))
        .route("/api/calendar", get(api::calendar))
        .route("/api/backup-all", post(api::backup_all))
        .route("/api/masterkey/confirm", post(api::confirm_masterkey))
        .route("/api/sources/{id}/stop", put(api::set_stop_app))
        .route("/api/overview", get(api::overview))
        .route(
            "/api/sources",
            get(api::list_sources).post(api::create_source),
        )
        .route(
            "/api/sources/{id}",
            get(api::get_source)
                .put(api::update_source)
                .delete(api::delete_source),
        )
        .route("/api/sources/{id}/token", post(api::rotate_token))
        .route("/api/sources/{id}/request", post(api::request_backup))
        .route("/api/sources/{id}/prune", post(api::prune))
        .route("/api/sources/{id}/integration", get(api::integration))
        .route(
            "/api/sources/{id}/upload",
            post(api::upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/backups/{id}", delete(api::delete_backup))
        .route("/api/backups/{id}/download", get(api::download))
        .route("/api/backups/{id}/retry", post(api::retry))
        .route(
            "/api/settings",
            get(api::get_settings).put(api::put_settings),
        )
        .route("/api/filen/connect", post(api::filen_connect))
        .route("/api/filen/refresh", post(api::filen_refresh))
        .route("/api/filen/disconnect", post(api::filen_disconnect))
        .route("/api/tokens", post(api::create_token))
        .route("/api/tokens/{id}", delete(api::delete_token))
        .route("/api/events", get(api::events))
        .route("/api/umbrel", get(api::get_umbrel).put(api::put_umbrel))
        .route("/api/umbrel/apps", get(api::umbrel_apps))
        .route("/api/umbrel/browse", get(api::umbrel_browse))
        .route("/api/umbrel/sources", post(api::umbrel_bulk))
        .route("/api/umbrel/download", get(api::umbrel_download))
        .route("/api/filen/browse", get(api::filen_browse))
        .route("/api/filen/download", get(api::filen_download));

    let machine = Router::new()
        .route(
            "/api/v1/backup",
            post(api::push).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/v1/status", get(api::machine_status))
        .route("/api/v1/backups/{id}/download", get(api::machine_download))
        .route(
            "/mcp",
            post(mcp::post)
                .get(mcp::not_allowed)
                .delete(mcp::not_allowed)
                .layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        );

    let app = Router::new()
        .merge(ui)
        .merge(machine)
        .fallback_service(ServeDir::new(&public_dir).fallback(ServeFile::new(index)))
        .layer(axum::middleware::from_fn(cache_headers))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("cannot bind");
    eprintln!("nrzBackup {VERSION} listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server error");
}

/// Docker stops containers with SIGTERM.
async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}

/// Browsers must never keep an old `index.html` (after an update they would load the old web
/// app); the hashed files under `/assets/` never change and may be cached forever. API answers
/// are never cached.
async fn cache_headers(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    use axum::http::{HeaderValue, header};
    let path = req.uri().path().to_string();
    let mut res = next.run(req).await;
    let value = if path.starts_with("/assets/") && res.status().is_success() {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    res.headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static(value));
    res
}
