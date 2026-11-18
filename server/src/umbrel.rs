//! umbrelOS as a backup source. /Home and /Apps are read directly from the folders the umbrelOS
//! package mounts (see [`crate::local`]), no token needed; folders are packed as `.tar.gz`.
//!
//! Optional: a MCP token (`umbrelmcp_…`, set up in the settings, stored sealed) adds /External and
//! /Network through the umbrelOS file API and lets nrzBackup stop apps for a consistent copy
//! (MCP tools `stop_app` / `start_app`).

use std::sync::Arc;
use std::time::Duration;

use nrzbackup_shared::*;
use serde_json::{Value, json};

use crate::db::Source;
use crate::mcp_client::McpClient;
use crate::AppState;

const DEFAULT_URL: &str = "http://host.docker.internal";
const TOOL_TIMEOUT: Duration = Duration::from_secs(60);
/// The folder of nrzBackup itself must never be backed up (the spool would grow forever).
const SELF_PATH: &str = "/Apps/nrz-backup";
const SELF_APP: &str = "nrz-backup";

pub struct Conn {
    pub url: String,
    pub token: String,
}

pub fn url(state: &AppState) -> String {
    state
        .db
        .get("umbrel_url")
        .filter(|u| !u.trim().is_empty())
        .or_else(|| {
            std::env::var("NRZBACKUP_UMBREL_URL")
                .ok()
                .filter(|u| !u.trim().is_empty())
        })
        .unwrap_or_else(|| DEFAULT_URL.into())
        .trim()
        .trim_end_matches('/')
        .to_string()
}

fn token(state: &AppState) -> String {
    state
        .db
        .get("umbrel_token")
        .and_then(|t| state.secrets.open(&t))
        .unwrap_or_default()
}

pub fn conn(state: &AppState) -> Result<Conn, String> {
    let token = token(state);
    if token.is_empty() {
        return Err(
            "Die umbrelOS-Verbindung ist nicht eingerichtet (Einstellungen → umbrelOS-Verbindung)."
                .into(),
        );
    }
    Ok(Conn {
        url: url(state),
        token,
    })
}

/// Current status without contacting umbrelOS (last test result).
pub fn status(state: &AppState) -> UmbrelStatus {
    let token = token(state);
    let mut st: UmbrelStatus = state
        .db
        .get("umbrel_status")
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    st.url = url(state);
    st.local = crate::local::mounted().iter().map(|r| format!("/{r}")).collect();
    st.has_token = !token.is_empty();
    st.token_hint = if token.is_empty() {
        String::new()
    } else {
        format!("{}…", token.chars().take(14).collect::<String>())
    };
    if token.is_empty() {
        st.connected = false;
        st.device.clear();
        st.error = None;
    }
    st
}

/// Saves the connection and tests it.
pub async fn save(state: &AppState, input: &UmbrelConnect) -> Result<UmbrelStatus, String> {
    let url = input.url.trim().trim_end_matches('/');
    if !url.is_empty() {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| "Die umbrelOS-Adresse ist keine gültige URL".to_string())?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err("Die umbrelOS-Adresse muss mit http:// oder https:// beginnen".into());
        }
    }
    state.db.set("umbrel_url", url);
    if input.clear {
        state.db.del("umbrel_token");
        state.db.del("umbrel_status");
        state
            .db
            .event(EventLevel::Info, None, "umbrelOS-Verbindung entfernt");
        return Ok(status(state));
    }
    let t = input.token.trim();
    if !t.is_empty() {
        state.db.set("umbrel_token", &state.secrets.seal(t));
    }
    Ok(test(state).await)
}

/// Contacts umbrelOS and stores the result.
pub async fn test(state: &AppState) -> UmbrelStatus {
    let result = match conn(state) {
        Ok(c) => call(state, &c, "get_system_info", json!({})).await,
        Err(e) => Err(e),
    };
    let mut st = status(state);
    match result {
        Ok(info) => {
            let name = info
                .get("versionName")
                .and_then(Value::as_str)
                .unwrap_or("umbrelOS");
            let host = info.get("hostname").and_then(Value::as_str).unwrap_or("");
            st.connected = true;
            st.device = if host.is_empty() {
                name.to_string()
            } else {
                format!("{name} · {host}")
            };
            st.error = None;
        }
        Err(e) => {
            st.connected = false;
            st.error = Some(e);
        }
    }
    if st.has_token {
        state.db.set(
            "umbrel_status",
            &serde_json::to_string(&st).unwrap_or_default(),
        );
    }
    st
}

async fn call(state: &AppState, c: &Conn, tool: &str, args: Value) -> Result<Value, String> {
    let endpoint = format!("{}/mcp", c.url);
    let mut client = McpClient::new(&state.http, &endpoint, &c.token);
    client.call_tool(tool, args, TOOL_TIMEOUT).await.map_err(|e| {
        if e.contains("401") || e.to_lowercase().contains("unauthorized") {
            "umbrelOS lehnt den Token ab – bitte in den Einstellungen einen gültigen MCP-Token eintragen.".into()
        } else {
            e
        }
    })
}

// ─────────────────────────── Folders and apps ───────────────────────────

async fn list_dir(state: &AppState, c: &Conn, path: &str) -> Result<Vec<UmbrelEntry>, String> {
    let mut out = vec![];
    let mut last: Option<String> = None;
    for _ in 0..40 {
        let mut args = json!({ "path": path, "limit": 250 });
        if let Some(l) = &last {
            args["lastFile"] = Value::from(l.as_str());
        }
        let v = call(state, c, "list_directory", args).await?;
        let files = v
            .get("files")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for f in &files {
            let name = f
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            out.push(UmbrelEntry {
                path: f
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{}/{name}", path.trim_end_matches('/'))),
                is_dir: f.get("type").and_then(Value::as_str) == Some("directory"),
                size: f.get("size").and_then(Value::as_u64),
                modified: modified_ms(f),
                name,
            });
        }
        last = files
            .last()
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if v.get("hasMore").and_then(Value::as_bool) != Some(true) || last.is_none() {
            break;
        }
    }
    Ok(out)
}

/// Last change of a listed file in ms (umbrelOS sends ms; seconds are converted).
fn modified_ms(f: &Value) -> Option<i64> {
    ["modified", "lastModified", "mtime", "modifiedAt"]
        .iter()
        .find_map(|k| {
            let v = f.get(*k)?;
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        })
        .filter(|n| *n > 0.0)
        .map(|n| if n < 1e11 { n * 1000.0 } else { n } as i64)
}

pub async fn browse(state: &AppState, path: &str) -> Result<UmbrelDir, String> {
    let path = normalize(path);
    let mut entries = if path == "/" {
        let local = crate::local::mounted();
        let mut v: Vec<UmbrelEntry> = local
            .iter()
            .map(|r| UmbrelEntry {
                name: r.to_string(),
                path: format!("/{r}"),
                is_dir: true,
                ..Default::default()
            })
            .collect();
        // /External, /Network (and everything when nothing is mounted) through the token
        match conn(state) {
            Ok(c) => match list_dir(state, &c, "/").await {
                Ok(remote) => v.extend(
                    remote
                        .into_iter()
                        .filter(|e| !local.iter().any(|r| e.name == *r)),
                ),
                Err(e) if local.is_empty() => return Err(e),
                Err(_) => {}
            },
            Err(e) if local.is_empty() => return Err(no_access(e)),
            Err(_) => {}
        }
        v
    } else if let Some(dir) = crate::local::resolve(&path) {
        let list = tokio::task::spawn_blocking(move || crate::local::list(&dir))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("{path} ist nicht lesbar: {e}"))?;
        list.into_iter()
            .map(|(name, is_dir, size, modified)| UmbrelEntry {
                path: format!("{}/{name}", path.trim_end_matches('/')),
                name,
                is_dir,
                size,
                modified,
            })
            .collect()
    } else {
        let c = conn(state).map_err(no_access)?;
        list_dir(state, &c, &path).await?
    };
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let parent = if path == "/" {
        None
    } else {
        Some(match path.rsplit_once('/') {
            Some(("", _)) | None => "/".to_string(),
            Some((p, _)) => p.to_string(),
        })
    };
    Ok(UmbrelDir {
        path,
        parent,
        entries,
    })
}

/// Error when neither the mounted folders nor a token are there.
fn no_access(e: String) -> String {
    if crate::local::available() {
        format!("Dieser Ordner ist nur mit umbrelOS-Token erreichbar ({e})")
    } else {
        "nrzBackup sieht die umbrelOS-Ordner nicht. Bitte nrzBackup im nrzStore aktualisieren (ab 1.1.0 sind /Home und /Apps direkt eingebunden).".into()
    }
}

/// `/Home//x/` → `/Home/x`
pub fn normalize(path: &str) -> String {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.contains(&"..") {
        return "/".into();
    }
    format!("/{}", parts.join("/"))
}

pub async fn apps(state: &AppState) -> Result<Vec<UmbrelApp>, String> {
    if let Some(root) = crate::local::resolve("/Apps") {
        return local_apps(state, root).await;
    }
    let c = conn(state).map_err(no_access)?;
    let list = call(state, &c, "list_apps", json!({})).await?;
    let items = list
        .as_array()
        .cloned()
        .or_else(|| list.get("apps").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let sources = state.db.sources();
    let mut out = vec![];
    // which apps have a data/ folder – asked in parallel
    let checks = items.iter().map(|a| {
        let id = a
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let c = &c;
        async move {
            let app_path = format!("/Apps/{id}");
            let has_data = list_dir(state, c, &app_path)
                .await
                .map(|e| e.iter().any(|x| x.is_dir && x.name == "data"))
                .unwrap_or(false);
            (id, has_data)
        }
    });
    let data: std::collections::HashMap<String, bool> = futures_util::future::join_all(checks)
        .await
        .into_iter()
        .collect();
    for a in &items {
        let id = a
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        let app_path = format!("/Apps/{id}");
        let data_path = if data.get(&id).copied().unwrap_or(false) {
            format!("{app_path}/data")
        } else {
            app_path.clone()
        };
        out.push(UmbrelApp {
            name: a
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&id)
                .to_string(),
            version: a
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            state: a
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            source_id: sources
                .iter()
                .find(|s| s.pull_kind == PullKind::Umbrel && s.umbrel_app == id)
                .map(|s| s.id),
            data_path,
            app_path,
            id,
        });
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

/// Apps from the mounted app-data folder; the state comes from umbrelOS when a token is set.
async fn local_apps(state: &AppState, root: std::path::PathBuf) -> Result<Vec<UmbrelApp>, String> {
    let found = tokio::task::spawn_blocking(move || {
        let mut v = vec![];
        for e in std::fs::read_dir(&root).map_err(|e| format!("/Apps ist nicht lesbar: {e}"))? {
            let Ok(e) = e else { continue };
            let dir = e.path();
            if !dir.is_dir() {
                continue;
            }
            let Some((name, version)) = crate::local::app_manifest(&dir) else {
                continue;
            };
            let id = e.file_name().to_string_lossy().to_string();
            let has_data = dir.join("data").is_dir();
            v.push((id, name, version, has_data));
        }
        Ok::<_, String>(v)
    })
    .await
    .map_err(|e| e.to_string())??;
    // optional: running state from umbrelOS
    let mut states = std::collections::HashMap::new();
    if let Ok(c) = conn(state)
        && let Ok(list) = call(state, &c, "list_apps", json!({})).await
    {
        let items = list
            .as_array()
            .cloned()
            .or_else(|| list.get("apps").and_then(Value::as_array).cloned())
            .unwrap_or_default();
        for a in items {
            if let (Some(id), Some(st)) = (
                a.get("id").and_then(Value::as_str),
                a.get("state").and_then(Value::as_str),
            ) {
                states.insert(id.to_string(), st.to_string());
            }
        }
    }
    let sources = state.db.sources();
    let mut out: Vec<UmbrelApp> = found
        .into_iter()
        .map(|(id, name, version, has_data)| {
            let app_path = format!("/Apps/{id}");
            UmbrelApp {
                name: if name.is_empty() { id.clone() } else { name },
                version,
                state: states.get(&id).cloned().unwrap_or_default(),
                source_id: sources
                    .iter()
                    .find(|s| s.pull_kind == PullKind::Umbrel && s.umbrel_app == id)
                    .map(|s| s.id),
                data_path: if has_data {
                    format!("{app_path}/data")
                } else {
                    app_path.clone()
                },
                app_path,
                id,
            }
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

pub fn check_path(path: &str) -> Result<(), String> {
    let p = normalize(path);
    if p == "/" || !path.trim().starts_with('/') {
        return Err(
            "Bitte einen Ordner oder eine Datei unter /Apps, /Home, /External oder /Network wählen"
                .into(),
        );
    }
    if p == SELF_PATH || p.starts_with(&format!("{SELF_PATH}/")) {
        return Err("nrzBackup kann seinen eigenen Ordner nicht sichern".into());
    }
    Ok(())
}

// ─────────────────────────── Backup ───────────────────────────

async fn app_state(state: &AppState, c: &Conn, app: &str) -> Result<String, String> {
    let v = call(state, c, "get_app_status", json!({ "appId": app })).await?;
    Ok(v.get("state")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string())
}

/// Stops the app and waits until it is stopped. Returns true when it was running before.
async fn stop(state: &AppState, c: &Conn, app: &str) -> Result<bool, String> {
    let before = app_state(state, c, app).await?;
    if before != "ready" && before != "running" {
        return Ok(false);
    }
    call(state, c, "stop_app", json!({ "appId": app })).await?;
    for _ in 0..90 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if app_state(state, c, app).await.unwrap_or_default() == "stopped" {
            return Ok(true);
        }
    }
    Err(format!(
        "{app} hat nicht innerhalb von 3 Minuten angehalten"
    ))
}

/// Backs up the configured app/folder: read directly when mounted, else through the umbrelOS
/// file API. Apps are stopped meanwhile only when asked for and a token is set.
pub async fn backup(state: &Arc<AppState>, source: &Source) -> Result<String, String> {
    let path = normalize(&source.umbrel_path);
    check_path(&source.umbrel_path)?;
    let local = crate::local::resolve(&path);
    let c = conn(state);
    if local.is_none() && c.is_err() {
        return Err(no_access(c.err().unwrap_or_default()));
    }
    let app = source.umbrel_app.trim();

    let mut stopped = false;
    if source.stop_app && !app.is_empty() && app != SELF_APP {
        match &c {
            Ok(c) => match stop(state, c, app).await {
                Ok(s) => stopped = s,
                Err(e) => {
                    // a backup without a stop is better than none
                    state.db.event(
                        EventLevel::Warn,
                        Some(source.id),
                        &format!("{app} konnte nicht angehalten werden, Backup läuft trotzdem: {e}"),
                    );
                }
            },
            Err(_) => state.db.event(
                EventLevel::Info,
                Some(source.id),
                &format!("{app} wird ohne umbrelOS-Token nicht angehalten, Backup läuft im Betrieb"),
            ),
        }
    }

    let result = match (&local, &c) {
        (Some(dir), _) => backup_local(state, source, &path, dir.clone()).await,
        (None, Ok(c)) => download(state, c, source, &path).await,
        (None, Err(e)) => Err(e.clone()),
    };

    if stopped && let Ok(c) = &c {
        let mut started = Err(String::new());
        for _ in 0..3 {
            started = call(state, c, "start_app", json!({ "appId": app }))
                .await
                .map(|_| ());
            if started.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        match started {
            Ok(()) => state.db.event(EventLevel::Info, Some(source.id), &format!("{app} wieder gestartet")),
            Err(e) => state.db.event(
                EventLevel::Error,
                Some(source.id),
                &format!("{app} konnte nach dem Backup NICHT wieder gestartet werden – bitte in umbrelOS starten: {e}"),
            ),
        }
    }
    result
}

/// Name of the archive for a virtual path: `/Apps/x/data` → `x-data`.
fn archive_base(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        ["Apps", id, rest @ ..] if !rest.is_empty() => format!("{id}-{}", rest.join("-")),
        [.., last] => last.to_string(),
        [] => "umbrel".into(),
    }
}

/// Opens a mounted file or folder: (stream, file name, size if known).
pub fn open_local(
    path: &str,
) -> Option<(
    std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<axum::body::Bytes, std::io::Error>> + Send>>,
    String,
    Option<u64>,
    tokio::sync::oneshot::Receiver<std::io::Result<crate::local::PackStats>>,
)> {
    let dir = crate::local::resolve(path)?;
    let meta = std::fs::metadata(&dir).ok()?;
    let base = archive_base(path);
    if meta.is_file() {
        let name = path.rsplit('/').next().unwrap_or("datei").to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(Ok(crate::local::PackStats { files: 1, skipped: vec![] }));
        let file = std::fs::File::open(&dir).ok()?;
        let stream = tokio_util::io::ReaderStream::with_capacity(tokio::fs::File::from_std(file), 1 << 20);
        return Some((Box::pin(stream), name, Some(meta.len()), rx));
    }
    let skip = crate::local::resolve(SELF_PATH);
    let top = path.rsplit('/').next().unwrap_or("umbrel").to_string();
    let (stream, done) = crate::local::pack(dir, top, skip);
    Some((Box::pin(stream), format!("{base}.tar.gz"), None, done))
}

async fn backup_local(
    state: &Arc<AppState>,
    source: &Source,
    path: &str,
    _dir: std::path::PathBuf,
) -> Result<String, String> {
    let (stream, name, _, done) =
        open_local(path).ok_or_else(|| format!("{path} gibt es nicht (mehr)"))?;
    let acc = crate::worker::ingest(state, source, &name, BackupOrigin::Pull, stream).await?;
    if let Ok(Ok(stats)) = done.await
        && !stats.skipped.is_empty()
    {
        let n = stats.skipped.len();
        let first: Vec<&str> = stats.skipped.iter().take(3).map(String::as_str).collect();
        state.db.event(
            EventLevel::Warn,
            Some(source.id),
            &format!("{n} Einträge waren nicht lesbar und fehlen im Backup: {}", first.join("; ")),
        );
    }
    Ok(format!(
        "{} direkt aus umbrelOS gesichert ({})",
        acc.filename,
        format_bytes(acc.size)
    ))
}

async fn download(
    state: &Arc<AppState>,
    c: &Conn,
    source: &Source,
    path: &str,
) -> Result<String, String> {
    let mut url =
        reqwest::Url::parse(&format!("{}/mcp/files/download", c.url)).map_err(|e| e.to_string())?;
    url.query_pairs_mut().append_pair("path", path);
    let res = state
        .http
        .get(url)
        .bearer_auth(&c.token)
        .send()
        .await
        .map_err(|e| format!("umbrelOS nicht erreichbar ({}): {e}", c.url))?;
    if res.status().as_u16() == 401 || res.status().as_u16() == 403 {
        return Err(format!(
            "umbrelOS verweigert den Zugriff auf {path} (HTTP {}) – hat der MCP-Token Dateizugriff?",
            res.status()
        ));
    }
    crate::pull::download_response(state, source, res, None).await
}

/// Opens a file or folder (as ZIP) from umbrelOS for the explorer download.
pub async fn open_download(state: &AppState, path: &str) -> Result<reqwest::Response, String> {
    let c = conn(state).map_err(no_access)?;
    let path = normalize(path);
    if path == "/" {
        return Err("Bitte einen Ordner oder eine Datei wählen".into());
    }
    let mut url =
        reqwest::Url::parse(&format!("{}/mcp/files/download", c.url)).map_err(|e| e.to_string())?;
    url.query_pairs_mut().append_pair("path", &path);
    let res = state
        .http
        .get(url)
        .bearer_auth(&c.token)
        .send()
        .await
        .map_err(|e| format!("umbrelOS nicht erreichbar ({}): {e}", c.url))?;
    if !res.status().is_success() {
        return Err(format!(
            "umbrelOS liefert {path} nicht (HTTP {})",
            res.status()
        ));
    }
    Ok(res)
}

/// Source input for an app, used by "umbrelOS-Apps hinzufügen".
pub fn app_input(app: &UmbrelApp, bulk: &UmbrelBulk) -> SourceInput {
    SourceInput {
        name: app.name.clone(),
        description: format!("umbrelOS-App {} ({})", app.name, app.id),
        retention: bulk.retention,
        expect_hours: bulk.expect_hours,
        pull: PullInput {
            kind: PullKind::Umbrel,
            schedule: bulk.schedule.clone(),
            umbrel_path: if bulk.whole_folder {
                app.app_path.clone()
            } else {
                app.data_path.clone()
            },
            umbrel_app: app.id.clone(),
            stop_app: (bulk.stop_app || bulk.stop_apps.contains(&app.id)) && app.id != SELF_APP,
            ..Default::default()
        },
    }
}

/// Source input for a folder such as `/Home/Documents`.
pub fn folder_input(path: &str, bulk: &UmbrelBulk) -> SourceInput {
    let path = normalize(path);
    let name = path.rsplit('/').next().unwrap_or("Ordner").to_string();
    SourceInput {
        name,
        description: format!("umbrelOS-Ordner {path}"),
        retention: bulk.retention,
        expect_hours: bulk.expect_hours,
        pull: PullInput {
            kind: PullKind::Umbrel,
            schedule: bulk.schedule.clone(),
            umbrel_path: path,
            ..Default::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(normalize("/Home//Docs/"), "/Home/Docs");
        assert_eq!(normalize(""), "/");
        assert_eq!(normalize("/Home/../Apps"), "/");
        assert!(check_path("/Apps/nrz-backup/data").is_err());
        assert!(check_path("/").is_err());
        assert!(check_path("Home").is_err());
        assert!(check_path("/Apps/nrz-budget/data").is_ok());
        assert_eq!(archive_base("/Apps/nrz-budget/data"), "nrz-budget-data");
        assert_eq!(archive_base("/Apps/nrz-budget"), "nrz-budget");
        assert_eq!(archive_base("/Home/Documents"), "Documents");
    }
}
