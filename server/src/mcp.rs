//! MCP server (JSON-RPC 2.0 over Streamable HTTP, stateless) at `POST /mcp`.
//!
//! - Access token `nrzbka_…` (Settings → tokens; for nrzDashboard): status, sources, backups,
//!   events, request a backup from an app, deliver files for any source.
//! - Push token `nrzbk_…` of a source (for the app itself): deliver files, own status and copies.
//!
//! Tool results come as JSON text in `content` (what nrzDashboard parses) and as
//! `structuredContent`.

use std::sync::Arc;

use axum::body::{Bytes, to_bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use nrzbackup_shared::*;
use serde_json::{Value, json};

use crate::api::{Caller, machine_caller};
use crate::db::Source;
use crate::{AppState, api, pull, status, worker};

const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "nrzBackup stores backups of other apps encrypted (end-to-end, AES-256-GCM) \
in Filen.io and keeps a configurable number of copies per source (last, daily, weekly, monthly, yearly). \
Start with get_status for the traffic light (ok/warning/error), the Filen storage and every source. \
To deliver a file use backup_file (small files, base64) or backup_from_url (nrzBackup downloads it). \
request_backup asks an app to send a backup now. Timestamps are Unix seconds, sizes bytes. \
`source` accepts the id, the slug or the name.";

pub async fn post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Response {
    let Some(caller) = machine_caller(&state, &headers) else {
        return unauthorized();
    };
    let body: Bytes = match to_bytes(body, 256 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return reply(error(
                Value::Null,
                -32700,
                &format!("Body too large or broken: {e}"),
            ));
        }
    };
    let request: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => return reply(error(Value::Null, -32700, &format!("Parse error: {e}"))),
    };
    match request {
        Value::Array(items) => {
            let mut out = vec![];
            for m in items {
                if let Some(v) = handle(&state, &caller, m).await {
                    out.push(v);
                }
            }
            if out.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                reply(Value::Array(out))
            }
        }
        m => match handle(&state, &caller, m).await {
            Some(v) => reply(v),
            None => StatusCode::ACCEPTED.into_response(),
        },
    }
}

pub async fn not_allowed() -> Response {
    let mut res = (
        StatusCode::METHOD_NOT_ALLOWED,
        "nrzBackup MCP: send JSON-RPC requests with POST",
    )
        .into_response();
    res.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST"));
    res
}

fn unauthorized() -> Response {
    let body = json!({
        "jsonrpc": "2.0",
        "id": Value::Null,
        "error": { "code": -32001, "message": "Missing or invalid token. Create an access token in nrzBackup under Settings, or use the push token of a source." }
    });
    let mut res = (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response();
    res.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"nrzBackup MCP\""),
    );
    res
}

fn reply(v: Value) -> Response {
    axum::Json(v).into_response()
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

async fn handle(state: &Arc<AppState>, caller: &Caller, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return id.map(|id| error(id, -32600, "Invalid request"));
    };
    let id = id?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let admin = matches!(caller, Caller::Admin);
    Some(match method {
        "initialize" => {
            let wanted = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOLS[0]);
            let version = PROTOCOLS
                .iter()
                .find(|v| **v == wanted)
                .unwrap_or(&PROTOCOLS[0]);
            ok(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "nrzbackup", "title": "nrzBackup", "version": crate::VERSION },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({ "tools": tools(admin) })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if !tools(admin)
                .iter()
                .any(|t| t["name"].as_str() == Some(name))
            {
                return Some(error(id, -32602, &format!("Unknown tool: {name}")));
            }
            ok(
                id,
                match call(state, caller, name, &args).await {
                    Ok(v) => json!({
                        "content": [{ "type": "text", "text": v.to_string() }],
                        "structuredContent": v,
                        "isError": false,
                    }),
                    Err(e) => {
                        json!({ "content": [{ "type": "text", "text": e }], "isError": true })
                    }
                },
            )
        }
        "resources/list" => ok(id, json!({ "resources": [] })),
        "prompts/list" => ok(id, json!({ "prompts": [] })),
        _ => error(id, -32601, &format!("Method not found: {method}")),
    })
}

fn tool(
    name: &str,
    title: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required, "additionalProperties": false },
        "annotations": { "title": title, "readOnlyHint": read_only, "idempotentHint": read_only, "openWorldHint": false },
    })
}

fn source_prop(admin: bool) -> Value {
    if admin {
        json!({ "type": ["integer", "string"], "description": "Source id, slug or name." })
    } else {
        json!({ "type": ["integer", "string"], "description": "Ignored: a push token always means its own source." })
    }
}

fn tools(admin: bool) -> Vec<Value> {
    let req_source: &[&str] = if admin { &["source"] } else { &[] };
    let mut t = vec![
        tool(
            "get_status",
            "Status",
            if admin {
                "Traffic light for a dashboard: overall health (ok, warning, error, unknown) with a one-line message, Filen connection and storage (used/max bytes), copies and bytes stored, upload queue, last backup and every source with its own health, copies, size, last backup, next scheduled request and retention."
            } else {
                "Health, copies, stored bytes, last backup and retention of this token's source."
            },
            json!({}),
            &[],
            true,
        ),
        tool(
            "list_backups",
            "Backups",
            "Backups of a source, newest first: id, file name, name in Filen, size, SHA-256, received/stored time, status (queued, uploading, stored, deleted, failed) and which retention rules keep it.",
            json!({
                "source": source_prop(admin),
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "Default 20." },
                "include_deleted": { "type": "boolean", "description": "Also list copies removed by retention (default false)." },
            }),
            req_source,
            true,
        ),
        tool(
            "backup_file",
            "Back up a file",
            "Deliver a backup file (base64, up to ~150 MB). It is stored locally until it is encrypted and uploaded to Filen, then old copies are thinned out. For big files prefer backup_from_url or the HTTP push (POST /api/v1/backup?name=<file> with the push token, raw body).",
            json!({
                "source": source_prop(admin),
                "filename": { "type": "string", "description": "File name, e.g. nrzbudget-2026-11-12.db" },
                "content_base64": { "type": "string", "description": "File content, base64 encoded." },
            }),
            &[&["filename", "content_base64"][..], req_source].concat(),
            false,
        ),
        tool(
            "backup_from_url",
            "Back up from URL",
            "nrzBackup downloads the file from the given http(s) URL (optionally with headers, e.g. Authorization) and backs it up. Returns when the file is safely received.",
            json!({
                "source": source_prop(admin),
                "url": { "type": "string", "description": "Download URL reachable from nrzBackup." },
                "filename": { "type": "string", "description": "File name (default: from the response)." },
                "headers": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Extra request headers." },
            }),
            &[&["url"][..], req_source].concat(),
            false,
        ),
    ];
    if admin {
        t.push(tool(
            "list_sources",
            "Sources",
            "Every app that sends backups: id, slug, name, health with message, copies, stored bytes, queued, last backup, how nrzBackup requests backups (none, http, mcp) and when, retention, folder in Filen.",
            json!({}),
            &[],
            true,
        ));
        t.push(tool(
            "request_backup",
            "Request a backup",
            "Ask an app to send a backup now (uses the source's HTTP or MCP request settings). Runs in the background; the result appears in get_events and the source's status.",
            json!({ "source": source_prop(true) }),
            &["source"],
            false,
        ));
        t.push(tool(
            "get_events",
            "Events",
            "The activity log, newest first: received, stored, deleted, failed, requested.",
            json!({
                "limit": { "type": "integer", "minimum": 1, "maximum": 500, "description": "Default 50." },
                "source": { "type": ["integer", "string"], "description": "Only this source (id, slug or name)." },
            }),
            &[],
            true,
        ));
    }
    t
}

fn resolve(state: &AppState, caller: &Caller, args: &Value) -> Result<Source, String> {
    match caller {
        Caller::Source(s) => Ok((**s).clone()),
        Caller::Admin => {
            let key = match args.get("source") {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                _ => return Err("`source` is required".into()),
            };
            state
                .db
                .find_source(&key)
                .ok_or_else(|| format!("Unknown source: {key}"))
        }
    }
}

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

async fn call(
    state: &Arc<AppState>,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, String> {
    match name {
        "get_status" => Ok(match caller {
            Caller::Admin => {
                let o = status::overview(state).await;
                let mut v = to_value(&o);
                if let Some(m) = v.as_object_mut() {
                    m.insert("health_label".into(), Value::from(o.health.label()));
                    m.insert(
                        "storage".into(),
                        json!({
                            "used": o.filen.storage_used,
                            "max": o.filen.storage_max,
                            "used_text": format_bytes(o.filen.storage_used),
                            "max_text": format_bytes(o.filen.storage_max),
                            "backups_text": format_bytes(o.stored_bytes),
                        }),
                    );
                }
                v
            }
            Caller::Source(s) => to_value(status::summary(state, s)),
        }),
        "list_sources" => Ok(to_value(
            state
                .db
                .sources()
                .iter()
                .map(|s| status::summary(state, s))
                .collect::<Vec<_>>(),
        )),
        "list_backups" => {
            let s = resolve(state, caller, args)?;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .clamp(1, 200) as usize;
            let deleted = args
                .get("include_deleted")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let items: Vec<BackupItem> = state
                .db
                .backups_of(s.id, 1000)
                .into_iter()
                .map(|b| b.item)
                .filter(|b| deleted || b.status != BackupStatus::Deleted)
                .take(limit)
                .collect();
            Ok(json!({ "source": s.name, "backups": items }))
        }
        "backup_file" => {
            let s = resolve(state, caller, args)?;
            let filename = args
                .get("filename")
                .and_then(Value::as_str)
                .ok_or("`filename` is required")?;
            let b64 = args
                .get("content_base64")
                .and_then(Value::as_str)
                .ok_or("`content_base64` is required")?;
            let data = B64
                .decode(b64.trim())
                .map_err(|e| format!("content_base64 is not valid base64: {e}"))?;
            let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from(data))]);
            let acc = worker::ingest(state, &s, filename, BackupOrigin::Mcp, stream).await?;
            Ok(to_value(acc))
        }
        "backup_from_url" => {
            let s = resolve(state, caller, args)?;
            let url = args
                .get("url")
                .and_then(Value::as_str)
                .ok_or("`url` is required")?;
            let filename = args
                .get("filename")
                .and_then(Value::as_str)
                .map(str::to_string);
            let headers = args.get("headers").cloned().unwrap_or(Value::Null);
            let msg = pull::fetch_url(state, &s, url, filename, &headers, None).await?;
            Ok(json!({ "ok": true, "message": msg }))
        }
        "request_backup" => {
            let s = resolve(state, caller, args)?;
            Ok(to_value(api::start_pull(state, &s)))
        }
        "get_events" => {
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .clamp(1, 500) as u32;
            let filter = match args.get("source") {
                Some(Value::Null) | None => None,
                Some(_) => Some(resolve(state, &Caller::Admin, args)?.id),
            };
            let events: Vec<Event> = state
                .db
                .events(if filter.is_some() { 2000 } else { limit })
                .into_iter()
                .filter(|e| filter.is_none() || e.source_id == filter)
                .take(limit as usize)
                .collect();
            Ok(to_value(events))
        }
        _ => Err(format!("Unknown tool: {name}")),
    }
}
