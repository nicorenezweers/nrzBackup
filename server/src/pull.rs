//! "Send me a backup": nrzBackup asks an app for a file, over HTTP or through an MCP tool.
//!
//! HTTP: `GET`/`POST <url>` with `Authorization: Bearer <token>`; the response body is the file
//! (name from `Content-Disposition`, else from the URL).
//!
//! MCP: `tools/call <tool>` with the configured arguments. The placeholders `{{push_url}}`,
//! `{{push_token}}` and `{{source}}` in the arguments are replaced, so an app can also answer
//! "will do" and push the file later with the one-time token (valid 24 h). The tool result may be:
//! - an embedded resource with `blob` (base64),
//! - JSON with `content_base64` (or `data_base64`/`base64`) and `filename`,
//! - JSON with `url` (or `download_url`), optional `filename` and `headers` – nrzBackup downloads it,
//! - anything else: taken as "the app pushes by itself".

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use nrzbackup_shared::*;
use serde_json::{Value, json};

use crate::AppState;
use crate::db::{Source, now};
use crate::mcp_client::McpClient;
use crate::secret;
use crate::worker::{self, next_run};

const PULL_TIMEOUT: Duration = Duration::from_secs(6 * 3600);

/// Pulls one backup from the source and records the result. Returns a message for the UI.
pub async fn run(state: &Arc<AppState>, source_id: i64) -> Result<String, String> {
    {
        let mut busy = state.pulling.lock().unwrap();
        if !busy.insert(source_id) {
            return Err("Für diese Quelle läuft schon eine Anforderung.".into());
        }
    }
    let result = match state.db.source(source_id) {
        Some(source) => {
            let r = tokio::time::timeout(PULL_TIMEOUT, pull(state, &source))
                .await
                .unwrap_or_else(|_| Err("Zeitüberschreitung beim Anfordern".into()));
            let t = now();
            let next = next_run(&source.schedule, t, state.tz)
                .filter(|_| source.pull_kind != PullKind::None);
            match &r {
                Ok(msg) => {
                    state.db.set_pull_result(source.id, t, next, None);
                    state.db.event(EventLevel::Info, Some(source.id), msg);
                }
                Err(e) => {
                    state.db.set_pull_result(source.id, t, next, Some(e));
                    state.db.event(
                        EventLevel::Warn,
                        Some(source.id),
                        &format!("Anfordern fehlgeschlagen: {e}"),
                    );
                }
            }
            r
        }
        None => Err("Quelle nicht gefunden".into()),
    };
    state.pulling.lock().unwrap().remove(&source_id);
    result
}

async fn pull(state: &Arc<AppState>, source: &Source) -> Result<String, String> {
    let token = state.secrets.open(&source.pull_token).unwrap_or_default();
    match source.pull_kind {
        PullKind::None => Err("Für diese Quelle ist kein Anfordern eingerichtet.".into()),
        PullKind::Http => {
            let req = if source.pull_method.eq_ignore_ascii_case("POST") {
                state.http.post(&source.pull_url)
            } else {
                state.http.get(&source.pull_url)
            };
            let req = if token.is_empty() {
                req
            } else {
                req.bearer_auth(&token)
            };
            let res = req
                .send()
                .await
                .map_err(|e| format!("{} nicht erreichbar: {e}", source.pull_url))?;
            download_response(state, source, res, None).await
        }
        PullKind::Mcp => pull_mcp(state, source, &token).await,
        PullKind::Umbrel => crate::umbrel::backup(state, source).await,
    }
}

/// Streams an HTTP response into the spool.
pub(crate) async fn download_response(
    state: &Arc<AppState>,
    source: &Source,
    res: reqwest::Response,
    name: Option<String>,
) -> Result<String, String> {
    let status = res.status();
    if !status.is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!(
            "HTTP {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    let filename = name
        .filter(|n| !n.trim().is_empty())
        .or_else(|| content_disposition_name(res.headers()))
        .or_else(|| {
            res.url()
                .path_segments()
                .and_then(|mut s| s.next_back().map(str::to_string))
                .filter(|s| s.contains('.'))
        })
        .unwrap_or_else(|| format!("{}.bin", source.slug));
    let stream = res.bytes_stream();
    let acc = worker::ingest(
        state,
        source,
        &filename,
        BackupOrigin::Pull,
        Box::pin(stream),
    )
    .await?;
    Ok(format!(
        "Backup {} angefordert und empfangen ({})",
        acc.filename,
        format_bytes(acc.size)
    ))
}

fn content_disposition_name(h: &reqwest::header::HeaderMap) -> Option<String> {
    let v = h.get(reqwest::header::CONTENT_DISPOSITION)?.to_str().ok()?;
    if let Some(i) = v.find("filename*=") {
        let rest = v[i + 10..].split(';').next()?.trim();
        let enc = rest.rsplit("''").next()?;
        return Some(percent_decode(enc));
    }
    let i = v.find("filename=")?;
    Some(
        v[i + 9..]
            .split(';')
            .next()?
            .trim()
            .trim_matches('"')
            .to_string(),
    )
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn pull_mcp(state: &Arc<AppState>, source: &Source, token: &str) -> Result<String, String> {
    if source.pull_url.trim().is_empty() {
        return Err("Keine MCP-URL eingetragen".into());
    }
    // one-time push token so the app can also deliver later by itself
    let ticket = secret::new_token(SOURCE_TOKEN_PREFIX);
    state
        .db
        .add_ticket(&secret::token_hash(&ticket), source.id, now() + 24 * 3600);
    let push_url = format!("{}/api/v1/backup", state.public_url());
    let args_text = source
        .pull_args
        .replace("{{push_url}}", &push_url)
        .replace("{{push_token}}", &ticket)
        .replace("{{source}}", &source.slug);
    let args: Value = if args_text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&args_text)
            .map_err(|e| format!("Argumente sind kein gültiges JSON: {e}"))?
    };
    let tool = if source.pull_tool.trim().is_empty() {
        "create_backup"
    } else {
        source.pull_tool.trim()
    };

    let mut client = McpClient::new(&state.http, source.pull_url.trim(), token);
    client.initialize().await;
    let result = client
        .request(
            "tools/call",
            json!({ "name": tool, "arguments": args }),
            PULL_TIMEOUT,
        )
        .await?;

    let content = result
        .get("content")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let text: String = content
        .iter()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(if text.is_empty() {
            "Die App meldet einen Fehler".into()
        } else {
            text
        });
    }

    // 1) embedded resource with a blob
    for c in &content {
        if let Some(res) = c.get("resource")
            && let Some(blob) = res.get("blob").and_then(Value::as_str)
        {
            let name = res
                .get("uri")
                .and_then(Value::as_str)
                .and_then(|u| u.rsplit('/').next())
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("{}.bin", source.slug));
            return store_base64(state, source, &name, blob).await;
        }
    }

    // 2) / 3) JSON answer
    let obj = result
        .get("structuredContent")
        .filter(|v| v.is_object())
        .cloned()
        .or_else(|| {
            serde_json::from_str::<Value>(&text)
                .ok()
                .filter(|v| v.is_object())
        });
    if let Some(obj) = obj {
        let filename = ["filename", "file_name", "name"]
            .iter()
            .find_map(|k| obj.get(*k).and_then(Value::as_str))
            .map(str::to_string);
        if let Some(b64) = ["content_base64", "data_base64", "base64"]
            .iter()
            .find_map(|k| obj.get(*k).and_then(Value::as_str))
        {
            let name = filename.unwrap_or_else(|| format!("{}.bin", source.slug));
            return store_base64(state, source, &name, b64).await;
        }
        if let Some(url) = ["url", "download_url", "downloadUrl"]
            .iter()
            .find_map(|k| obj.get(*k).and_then(Value::as_str))
        {
            let headers = obj.get("headers").cloned().unwrap_or(Value::Null);
            return fetch_url(
                state,
                source,
                url,
                filename,
                &headers,
                Some((source.pull_url.as_str(), token)),
            )
            .await;
        }
    }

    // 4) the app pushes by itself
    let short: String = text.chars().take(200).collect();
    Ok(if short.is_empty() {
        "Backup angefordert – die App sendet es selbst.".into()
    } else {
        format!("Backup angefordert – Antwort der App: {short}")
    })
}

async fn store_base64(
    state: &Arc<AppState>,
    source: &Source,
    name: &str,
    b64: &str,
) -> Result<String, String> {
    let data = B64
        .decode(b64.trim().as_bytes())
        .map_err(|e| format!("Inhalt ist kein gültiges Base64: {e}"))?;
    let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from(data))]);
    let acc = worker::ingest(state, source, name, BackupOrigin::Pull, stream).await?;
    Ok(format!(
        "Backup {} angefordert und empfangen ({})",
        acc.filename,
        format_bytes(acc.size)
    ))
}

/// Downloads a file from a URL into the spool. `auth`: (base URL, token) – the token is sent
/// along when the download URL is on the same host and no Authorization header was given.
pub async fn fetch_url(
    state: &Arc<AppState>,
    source: &Source,
    url: &str,
    filename: Option<String>,
    headers: &Value,
    auth: Option<(&str, &str)>,
) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("Ungültige URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Nur http(s)-URLs sind erlaubt".into());
    }
    let mut req = state.http.get(parsed.clone());
    let mut has_auth = false;
    if let Some(h) = headers.as_object() {
        for (k, v) in h {
            if let Some(v) = v.as_str() {
                has_auth |= k.eq_ignore_ascii_case("authorization");
                req = req.header(k.as_str(), v);
            }
        }
    }
    if !has_auth
        && let Some((base, token)) = auth
        && !token.is_empty()
        && reqwest::Url::parse(base)
            .ok()
            .map(|b| (b.host_str().map(str::to_string), b.port_or_known_default()))
            == Some((
                parsed.host_str().map(str::to_string),
                parsed.port_or_known_default(),
            ))
    {
        req = req.bearer_auth(token);
    }
    let res = req
        .send()
        .await
        .map_err(|e| format!("Download nicht möglich: {e}"))?;
    download_response(state, source, res, filename).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposition() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            "content-disposition",
            "attachment; filename=\"db.sqlite\"".parse().unwrap(),
        );
        assert_eq!(content_disposition_name(&h).as_deref(), Some("db.sqlite"));
        h.insert(
            "content-disposition",
            "attachment; filename*=UTF-8''gr%C3%BC%C3%9Fe.tar"
                .parse()
                .unwrap(),
        );
        assert_eq!(content_disposition_name(&h).as_deref(), Some("grüße.tar"));
    }
}
