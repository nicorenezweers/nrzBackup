//! HTTP handlers: the web UI (session cookie) and the machine API (Bearer tokens).
#![allow(clippy::result_large_err)] // axum handlers return Response as error

use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{StreamExt, stream};
use nrzbackup_shared::*;
use serde_json::json;

use crate::db::{Backup, Source, now};
use crate::{AppState, VERSION, pull, secret, status, worker};

type S = State<Arc<AppState>>;
pub type ApiResult<T> = Result<Json<T>, Response>;

const COOKIE: &str = "nrzbackup_session";
const SESSION_DAYS: i64 = 30;

pub fn err(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(ApiError { error: msg.into() })).into_response()
}

fn bad(msg: impl Into<String>) -> Response {
    err(StatusCode::BAD_REQUEST, msg)
}

fn not_found() -> Response {
    err(StatusCode::NOT_FOUND, "Nicht gefunden")
}

fn empty() -> Json<serde_json::Value> {
    Json(json!({}))
}

// ─────────────────────────── Auth ───────────────────────────

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|p| p.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

fn logged_in(state: &AppState, headers: &HeaderMap) -> bool {
    cookie_token(headers).is_some_and(|t| state.db.session_valid(&secret::token_hash(&t)))
}

fn require(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    if logged_in(state, headers) {
        Ok(())
    } else {
        Err(err(StatusCode::UNAUTHORIZED, "Bitte anmelden"))
    }
}

pub fn bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let t = v
        .strip_prefix("Bearer ")
        .or_else(|| v.strip_prefix("bearer "))?
        .trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Who is calling the machine API.
pub enum Caller {
    /// Access token (nrzDashboard, MCP clients): may see and control everything.
    Admin,
    /// Push token of one source (or a one-time pull ticket).
    Source(Box<Source>),
}

pub fn machine_caller(state: &AppState, headers: &HeaderMap) -> Option<Caller> {
    let token = bearer(headers)?;
    let hash = secret::token_hash(&token);
    if token.starts_with(ACCESS_TOKEN_PREFIX) {
        return state.db.access_token_valid(&hash).then_some(Caller::Admin);
    }
    if token.starts_with(SOURCE_TOKEN_PREFIX) {
        return state
            .db
            .source_by_token(&hash)
            .map(|s| Caller::Source(Box::new(s)));
    }
    None
}

fn unauthorized_machine() -> Response {
    let mut r = err(
        StatusCode::UNAUTHORIZED,
        "Token fehlt oder ist ungültig (Authorization: Bearer nrzbk_… oder nrzbka_…)",
    );
    r.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"nrzBackup\""),
    );
    r
}

pub async fn session(State(st): S, headers: HeaderMap) -> Json<SessionInfo> {
    Json(SessionInfo {
        authenticated: logged_in(&st, &headers),
        version: VERSION.into(),
    })
}

pub async fn login(State(st): S, Json(req): Json<LoginRequest>) -> Response {
    let user = req.username.trim();
    if !user.is_empty() && !user.eq_ignore_ascii_case("admin") {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        return err(
            StatusCode::UNAUTHORIZED,
            "Unbekannter Benutzer – der Benutzer heisst admin",
        );
    }
    // pasted passwords often carry a space or line break
    if secret::token_hash(req.password.trim()) != st.password_hash {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        return err(
            StatusCode::UNAUTHORIZED,
            "Falsches Passwort – bitte das Passwort aus umbrelOS (nrzBackup → Zugangsdaten) verwenden",
        );
    }
    let token = secret::new_token("s_");
    st.db
        .add_session(&secret::token_hash(&token), now() + SESSION_DAYS * 86_400);
    let cookie = format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        SESSION_DAYS * 86_400
    );
    let mut res = Json(SessionInfo {
        authenticated: true,
        version: VERSION.into(),
    })
    .into_response();
    res.headers_mut()
        .insert(header::SET_COOKIE, HeaderValue::from_str(&cookie).unwrap());
    res
}

pub async fn logout(State(st): S, headers: HeaderMap) -> Response {
    if let Some(t) = cookie_token(&headers) {
        st.db.delete_session(&secret::token_hash(&t));
    }
    let mut res = empty().into_response();
    res.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static("nrzbackup_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
    );
    res
}

// ─────────────────────────── Overview / sources ───────────────────────────

pub async fn overview(State(st): S, headers: HeaderMap) -> ApiResult<Overview> {
    require(&st, &headers)?;
    Ok(Json(status::overview(&st).await))
}

pub async fn list_sources(State(st): S, headers: HeaderMap) -> ApiResult<Vec<SourceSummary>> {
    require(&st, &headers)?;
    Ok(Json(
        st.db
            .sources()
            .iter()
            .map(|s| status::summary(&st, s))
            .collect(),
    ))
}

fn validate(input: &SourceInput) -> Result<(), Response> {
    if input.name.trim().is_empty() {
        return Err(bad("Bitte einen Namen eingeben"));
    }
    let p = &input.pull;
    if p.kind == PullKind::Umbrel {
        crate::umbrel::check_path(&p.umbrel_path).map_err(bad)?;
    } else if p.kind != PullKind::None {
        let url = reqwest::Url::parse(p.url.trim())
            .map_err(|_| bad("Die URL zum Anfordern ist ungültig"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(bad(
                "Die URL zum Anfordern muss mit http:// oder https:// beginnen",
            ));
        }
    }
    if p.kind == PullKind::Mcp && !p.arguments.trim().is_empty() {
        let v: serde_json::Value =
            serde_json::from_str(&p.arguments.replace("{{", "").replace("}}", ""))
                .map_err(|e| bad(format!("Argumente sind kein gültiges JSON: {e}")))?;
        if !v.is_object() {
            return Err(bad("Argumente müssen ein JSON-Objekt sein, z. B. {}"));
        }
    }
    if p.schedule.mode == ScheduleMode::Daily && worker::parse_hhmm(&p.schedule.time).is_none() {
        return Err(bad("Uhrzeit bitte als HH:MM angeben"));
    }
    Ok(())
}

fn apply_input(st: &AppState, s: &mut Source, input: &SourceInput) {
    s.name = input.name.trim().to_string();
    s.description = input.description.trim().to_string();
    s.retention = input.retention;
    s.expect_hours = input.expect_hours;
    let p = &input.pull;
    let schedule_changed = s.schedule != p.schedule || s.pull_kind != p.kind;
    s.pull_kind = p.kind;
    s.pull_url = p.url.trim().to_string();
    if p.clear_token {
        s.pull_token.clear();
    } else if !p.token.trim().is_empty() {
        s.pull_token = st.secrets.seal(p.token.trim());
    }
    s.pull_method = if p.method.eq_ignore_ascii_case("POST") {
        "POST".into()
    } else {
        "GET".into()
    };
    s.pull_tool = if p.tool.trim().is_empty() {
        "create_backup".into()
    } else {
        p.tool.trim().to_string()
    };
    s.pull_args = p.arguments.trim().to_string();
    if p.kind == PullKind::Umbrel {
        s.umbrel_path = crate::umbrel::normalize(&p.umbrel_path);
        s.umbrel_app = p.umbrel_app.trim().to_string();
        s.stop_app = p.stop_app && !s.umbrel_app.is_empty();
    } else {
        s.umbrel_path.clear();
        s.umbrel_app.clear();
        s.stop_app = false;
    }
    let mut schedule = p.schedule.clone();
    schedule.every_hours = schedule.every_hours.max(1);
    s.schedule = schedule;
    if schedule_changed {
        s.next_pull = if s.pull_kind == PullKind::None {
            None
        } else {
            worker::next_run(&s.schedule, now(), st.tz)
        };
    }
}

fn unique_slug(st: &AppState, name: &str, except: i64) -> String {
    let base = slugify(name);
    let mut slug = base.clone();
    let mut n = 2;
    while st.db.slug_taken(&slug, except) {
        slug = format!("{base}-{n}");
        n += 1;
    }
    slug
}

pub async fn create_source(
    State(st): S,
    headers: HeaderMap,
    Json(input): Json<SourceInput>,
) -> ApiResult<SourceCreated> {
    require(&st, &headers)?;
    validate(&input)?;
    let (s, token) = insert_new_source(&st, &input)?;
    Ok(Json(SourceCreated {
        source: status::summary(&st, &s),
        token,
    }))
}

/// Creates a source from validated input; returns it with its push token.
fn insert_new_source(st: &AppState, input: &SourceInput) -> Result<(Source, String), Response> {
    let token = secret::new_token(SOURCE_TOKEN_PREFIX);
    let mut s = Source {
        id: 0,
        slug: unique_slug(st, &input.name, 0),
        name: String::new(),
        description: String::new(),
        token_hint: secret::token_hint(&token),
        retention: Retention::default(),
        expect_hours: 0,
        pull_kind: PullKind::None,
        pull_url: String::new(),
        pull_token: String::new(),
        pull_method: "GET".into(),
        pull_tool: "create_backup".into(),
        pull_args: String::new(),
        schedule: Schedule::default(),
        last_pull: None,
        next_pull: None,
        pull_error: None,
        created_at: now(),
        umbrel_path: String::new(),
        umbrel_app: String::new(),
        stop_app: false,
    };
    apply_input(st, &mut s, input);
    s.next_pull = if s.pull_kind == PullKind::None {
        None
    } else {
        worker::next_run(&s.schedule, now(), st.tz)
    };
    let id = st
        .db
        .insert_source(&s, &secret::token_hash(&token))
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    st.db.event(
        EventLevel::Info,
        Some(id),
        &format!("Quelle {} angelegt", s.name),
    );
    let s = st.db.source(id).ok_or_else(not_found)?;
    Ok((s, token))
}

fn source_of(st: &AppState, id: i64) -> Result<Source, Response> {
    st.db.source(id).ok_or_else(not_found)
}

pub async fn get_source(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<SourceDetail> {
    require(&st, &headers)?;
    let s = source_of(&st, id)?;
    let backups = st.db.backups_of(id, 200);
    let base = st.public_url();
    Ok(Json(SourceDetail {
        source: status::summary_with(&st, &s, &backups),
        backups: backups.into_iter().map(|b| b.item).collect(),
        push_url: format!("{base}/api/v1/backup"),
        mcp_url: format!("{base}/mcp"),
    }))
}

pub async fn integration(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<IntegrationPrompt> {
    require(&st, &headers)?;
    let s = source_of(&st, id)?;
    Ok(Json(IntegrationPrompt {
        text: crate::prompt::build(&s, &st.public_url(), &st.root_folder()),
    }))
}

pub async fn update_source(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<SourceInput>,
) -> ApiResult<SourceSummary> {
    require(&st, &headers)?;
    validate(&input)?;
    let mut s = source_of(&st, id)?;
    apply_input(&st, &mut s, &input);
    st.db
        .update_source(&s)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let s = source_of(&st, id)?;
    Ok(Json(status::summary(&st, &s)))
}

pub async fn delete_source(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    require(&st, &headers)?;
    let s = source_of(&st, id)?;
    for f in st.db.delete_source(id) {
        let _ = tokio::fs::remove_file(f).await;
    }
    st.db.event(
        EventLevel::Info,
        None,
        &format!("Quelle {} gelöscht (Kopien in Filen bleiben)", s.name),
    );
    Ok(empty())
}

pub async fn rotate_token(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<TokenCreated> {
    require(&st, &headers)?;
    let s = source_of(&st, id)?;
    let token = secret::new_token(SOURCE_TOKEN_PREFIX);
    let hint = secret::token_hint(&token);
    st.db
        .set_source_token(id, &secret::token_hash(&token), &hint);
    st.db.event(
        EventLevel::Info,
        Some(id),
        "Neuer Push-Token erstellt, der alte gilt nicht mehr",
    );
    Ok(Json(TokenCreated {
        token,
        info: TokenInfo {
            id,
            name: s.name,
            hint,
            created_at: now(),
            last_used: None,
        },
    }))
}

pub async fn request_backup(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<ActionResult> {
    require(&st, &headers)?;
    let s = source_of(&st, id)?;
    Ok(Json(start_pull(&st, &s)))
}

/// Starts a pull in the background.
pub fn start_pull(st: &Arc<AppState>, s: &Source) -> ActionResult {
    if s.pull_kind == PullKind::None {
        return ActionResult {
            ok: false,
            message: format!(
                "Für {} ist kein Anfordern eingerichtet – die App muss selbst senden.",
                s.name
            ),
        };
    }
    if st.pulling.lock().unwrap().contains(&s.id) {
        return ActionResult {
            ok: false,
            message: "Für diese Quelle läuft schon eine Anforderung.".into(),
        };
    }
    let st2 = st.clone();
    let id = s.id;
    tokio::spawn(async move {
        let _ = pull::run(&st2, id).await;
    });
    ActionResult {
        ok: true,
        message: format!(
            "Backup bei {} angefordert – das Ergebnis erscheint im Protokoll.",
            s.name
        ),
    }
}

pub async fn prune(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<ActionResult> {
    require(&st, &headers)?;
    source_of(&st, id)?;
    Ok(Json(match worker::prune(&st, id).await {
        Ok(0) => ActionResult {
            ok: true,
            message: "Nichts zu löschen – alle Kopien werden noch gebraucht.".into(),
        },
        Ok(n) => ActionResult {
            ok: true,
            message: format!("{n} alte Kopie(n) entfernt."),
        },
        Err(e) => ActionResult {
            ok: false,
            message: e,
        },
    }))
}

#[derive(serde::Deserialize)]
pub struct NameQuery {
    name: Option<String>,
    filename: Option<String>,
}

fn filename_from(q: &NameQuery, headers: &HeaderMap) -> String {
    q.name
        .clone()
        .or_else(|| q.filename.clone())
        .or_else(|| {
            headers
                .get("x-filename")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "backup.bin".into())
}

pub async fn upload(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(q): Query<NameQuery>,
    body: Body,
) -> ApiResult<BackupAccepted> {
    require(&st, &headers)?;
    let s = source_of(&st, id)?;
    let name = filename_from(&q, &headers);
    worker::ingest(
        &st,
        &s,
        &name,
        BackupOrigin::Upload,
        body.into_data_stream(),
    )
    .await
    .map(Json)
    .map_err(bad)
}

// ─────────────────────────── Backups ───────────────────────────

fn backup_of(st: &AppState, id: i64) -> Result<Backup, Response> {
    st.db.backup(id).ok_or_else(not_found)
}

pub async fn delete_backup(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    require(&st, &headers)?;
    let b = backup_of(&st, id)?;
    worker::delete_backup(&st, &b).await.map_err(bad)?;
    Ok(empty())
}

pub async fn retry(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<ActionResult> {
    require(&st, &headers)?;
    backup_of(&st, id)?;
    let ok = st.db.retry_now(id);
    if ok {
        st.wake.notify_one();
    }
    Ok(Json(ActionResult {
        ok,
        message: if ok {
            "Wird gleich erneut hochgeladen.".into()
        } else {
            "Dieses Backup kann nicht erneut hochgeladen werden.".into()
        },
    }))
}

pub async fn download(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Err(r) = require(&st, &headers) {
        return r;
    }
    match st.db.backup(id) {
        Some(b) => stream_download(&st, b, raw(&q)).await,
        None => not_found(),
    }
}

/// `?raw=1`: keep the master-key encryption (download the `.age` file as stored).
fn raw(q: &HashMap<String, String>) -> bool {
    q.get("raw").is_some_and(|v| v == "1")
}

/// Streams the decrypted file from Filen (also without the master-key layer unless `raw`).
async fn stream_download(st: &Arc<AppState>, b: Backup, raw: bool) -> Response {
    if b.item.status != BackupStatus::Stored {
        return bad("Diese Kopie liegt nicht in Filen");
    }
    let (Some(uuid), Some(key)) = (b.filen_uuid.clone(), st.secrets.open(&b.filen_key)) else {
        return bad("Zu dieser Kopie fehlen die Schlüssel");
    };
    let Some(session) = st.filen_session().await else {
        return bad("Filen ist nicht verbunden");
    };
    filen_stream(
        st,
        session,
        FilenObject {
            region: b.region.clone(),
            bucket: b.bucket.clone(),
            uuid,
            key,
            chunks: b.chunks,
            // the size of the original; an encrypted file is a bit larger
            size: if b.item.stored_name.ends_with(crate::masterkey::SUFFIX) { None } else { Some(b.item.size) },
            plain_size: Some(b.item.size),
            name: b.item.stored_name.clone(),
            mime: String::new(),
        },
        true,
        !raw,
    )
}

struct FilenObject {
    region: String,
    bucket: String,
    uuid: String,
    key: String,
    chunks: u64,
    /// Size as stored in Filen, if exactly known.
    size: Option<u64>,
    /// Size after removing the master-key layer, if known.
    plain_size: Option<u64>,
    name: String,
    mime: String,
}

/// Streams and decrypts a Filen file chunk by chunk; `decrypt` also removes the master-key
/// layer of `.age` files.
fn filen_stream(
    st: &Arc<AppState>,
    session: crate::filen::Session,
    o: FilenObject,
    attachment: bool,
    decrypt: bool,
) -> Response {
    let st2 = st.clone();
    let (region, bucket, uuid, key) = (o.region, o.bucket, o.uuid, o.key);
    let body = stream::iter(0..o.chunks)
        .map(move |i| {
            let st = st2.clone();
            let (session, region, bucket, uuid, key) = (
                session.clone(),
                region.clone(),
                bucket.clone(),
                uuid.clone(),
                key.clone(),
            );
            async move {
                st.filen
                    .download_chunk(&session, &region, &bucket, &uuid, i, &key)
                    .await
                    .map(Bytes::from)
                    .map_err(std::io::Error::other)
            }
        })
        .buffered(3)
        .boxed();
    if decrypt && let Some(plain) = o.name.strip_suffix(crate::masterkey::SUFFIX) {
        let Some(id) = crate::masterkey::identity(st) else {
            return bad("Der Hauptschlüssel fehlt – diese Datei kann nur mit dem gesicherten Schlüssel entschlüsselt werden");
        };
        let body = crate::masterkey::decrypt_stream(id, body);
        let mime = mime_guess::from_path(plain).first_or_octet_stream().to_string();
        let mut res = Response::new(Body::from_stream(body));
        set_file_headers(&mut res, plain, &mime, o.plain_size, attachment);
        return res;
    }
    let mut res = Response::new(Body::from_stream(body));
    let mime = if o.mime.is_empty() {
        "application/octet-stream"
    } else {
        o.mime.as_str()
    };
    set_file_headers(&mut res, &o.name, mime, o.size, attachment);
    res
}

/// Content-Type, Content-Disposition (ASCII + UTF-8 name) and Content-Length.
fn set_file_headers(res: &mut Response, name: &str, mime: &str, size: Option<u64>, attachment: bool) {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let kind = if attachment { "attachment" } else { "inline" };
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    if let Ok(v) = HeaderValue::from_str(&format!(
        "{kind}; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"
    )) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    if let Some(size) = size
        && let Ok(v) = HeaderValue::from_str(&size.to_string())
    {
        h.insert(header::CONTENT_LENGTH, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
}

// ─────────────────────────── Master key ───────────────────────────

/// `POST /api/masterkey/reveal`: the key is shown only with the app password (any time).
pub async fn reveal_masterkey(
    State(st): S,
    headers: HeaderMap,
    Json(req): Json<MasterKeyReveal>,
) -> ApiResult<MasterKeySecret> {
    require(&st, &headers)?;
    if secret::token_hash(req.password.trim()) != st.password_hash {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        return Err(bad("Falsches Passwort – bitte das App-Passwort aus umbrelOS (nrzBackup → Zugangsdaten) eingeben"));
    }
    let secret = crate::masterkey::secret_text(&st)
        .ok_or_else(|| bad("Der Hauptschlüssel ist nicht lesbar"))?;
    st.db
        .event(EventLevel::Info, None, "Hauptschlüssel in der App angezeigt");
    Ok(Json(MasterKeySecret {
        secret_key: secret,
        public_key: crate::masterkey::status(&st).public_key,
    }))
}

/// `POST /api/masterkey/saved`: target of a real form submit, so that Safari (iCloud
/// Keychain), Chrome (Google Password Manager) and others offer to save the key. The body is
/// ignored; only redirects back.
pub async fn masterkey_saved(State(st): S, headers: HeaderMap) -> Response {
    if !logged_in(&st, &headers) {
        return axum::response::Redirect::to("/").into_response();
    }
    st.db.event(
        EventLevel::Info,
        None,
        "Hauptschlüssel an den Passwort-Manager des Browsers übergeben",
    );
    axum::response::Redirect::to("/settings?key=saved").into_response()
}

/// `GET /api/stats`
pub async fn get_stats(State(st): S, headers: HeaderMap) -> ApiResult<Stats> {
    require(&st, &headers)?;
    Ok(Json(crate::stats::stats(&st).await))
}

#[derive(serde::Deserialize)]
pub struct RangeQuery {
    from: i64,
    to: i64,
}

/// `GET /api/calendar?from=&to=`
pub async fn calendar(
    State(st): S,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<RangeQuery>,
) -> ApiResult<Vec<BackupItem>> {
    require(&st, &headers)?;
    Ok(Json(st.db.backups_between(q.from, q.to).into_iter().map(|b| b.item).collect()))
}

/// `POST /api/backup-all`
pub async fn backup_all(State(st): S, headers: HeaderMap) -> ApiResult<ActionResult> {
    require(&st, &headers)?;
    Ok(Json(crate::stats::backup_all(&st)))
}

pub async fn confirm_masterkey(State(st): S, headers: HeaderMap) -> ApiResult<serde_json::Value> {
    require(&st, &headers)?;
    crate::masterkey::confirm(&st);
    Ok(empty())
}

/// `PUT /api/sources/{id}/stop`: stop the umbrelOS app during its backups.
pub async fn set_stop_app(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(input): Json<StopAppInput>,
) -> ApiResult<SourceDetail> {
    require(&st, &headers)?;
    let mut s = source_of(&st, id)?;
    if input.stop_app && (s.pull_kind != PullKind::Umbrel || s.umbrel_app.trim().is_empty()) {
        return Err(bad("Anhalten geht nur bei Quellen, die eine umbrelOS-App sichern"));
    }
    s.stop_app = input.stop_app;
    st.db.update_source(&s).map_err(|e| bad(e.to_string()))?;
    st.db.event(
        EventLevel::Info,
        Some(id),
        &if input.stop_app {
            format!("{} wird bei Backups angehalten und danach wieder gestartet", s.umbrel_app)
        } else {
            format!("{} läuft bei Backups weiter", s.umbrel_app)
        },
    );
    get_source(State(st), headers, Path(id)).await
}

// ─────────────────────────── Explorer ───────────────────────────

pub async fn filen_browse(
    State(st): S,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<FilenDir> {
    require(&st, &headers)?;
    let Some(session) = st.filen_session().await else {
        return Err(bad("Filen ist nicht verbunden"));
    };
    let path = q.get("path").map(String::as_str).unwrap_or("/");
    let chain = st.filen.resolve_path(&session, path).await.map_err(bad)?;
    let (path, uuid) = chain.last().cloned().unwrap();
    let parent = (chain.len() > 1).then(|| chain[chain.len() - 2].0.clone());
    let (folders, files) = st.filen.list_dir(&session, &uuid).await.map_err(bad)?;
    let mut entries: Vec<FilenEntry> = folders
        .into_iter()
        .map(|f| FilenEntry {
            uuid: f.uuid,
            name: f.name,
            is_dir: true,
            size: None,
            modified: f.modified,
            mime: String::new(),
        })
        .collect();
    entries.sort_by_key(|e| e.name.to_lowercase());
    let mut fs: Vec<FilenEntry> = files
        .into_iter()
        .map(|f| FilenEntry {
            uuid: f.uuid,
            name: f.name,
            is_dir: false,
            size: Some(f.size),
            modified: f.modified,
            mime: f.mime,
        })
        .collect();
    fs.sort_by_key(|e| e.name.to_lowercase());
    entries.extend(fs);
    Ok(Json(FilenDir {
        path,
        uuid,
        parent,
        entries,
    }))
}

pub async fn filen_download(
    State(st): S,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Err(r) = require(&st, &headers) {
        return r;
    }
    let (Some(folder), Some(uuid)) = (q.get("folder"), q.get("uuid")) else {
        return bad("folder und uuid fehlen");
    };
    let Some(session) = st.filen_session().await else {
        return bad("Filen ist nicht verbunden");
    };
    let files = match st.filen.list_dir(&session, folder).await {
        Ok((_, files)) => files,
        Err(e) => return bad(e),
    };
    let Some(f) = files.into_iter().find(|f| &f.uuid == uuid) else {
        return not_found();
    };
    if f.key.is_empty() {
        return bad("Der Schlüssel dieser Datei ist nicht lesbar");
    }
    if f.version == 1 {
        return bad("Diese Datei nutzt das alte Filen-Format v1, bitte direkt in Filen öffnen");
    }
    let inline = q.get("inline").is_some_and(|v| v == "1");
    filen_stream(
        &st,
        session,
        FilenObject {
            region: f.region,
            bucket: f.bucket,
            uuid: f.uuid,
            key: f.key,
            chunks: f.chunks,
            size: Some(f.size),
            plain_size: None,
            name: f.name,
            mime: f.mime,
        },
        !inline,
        !raw(&q),
    )
}

pub async fn umbrel_download(
    State(st): S,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if let Err(r) = require(&st, &headers) {
        return r;
    }
    let path = q.get("path").map(String::as_str).unwrap_or("/");
    let norm = crate::umbrel::normalize(path);
    if norm != "/"
        && let Some((stream, name, size, _)) = crate::umbrel::open_local(&norm)
    {
        let mime = if name.ends_with(".tar.gz") && size.is_none() {
            "application/gzip".to_string()
        } else {
            mime_guess::from_path(&name).first_or_octet_stream().to_string()
        };
        let mut out = Response::new(Body::from_stream(stream));
        set_file_headers(&mut out, &name, &mime, size, true);
        return out;
    }
    let res = match crate::umbrel::open_download(&st, path).await {
        Ok(r) => r,
        Err(e) => return bad(e),
    };
    let base = crate::umbrel::normalize(path)
        .rsplit('/')
        .next()
        .unwrap_or("download")
        .to_string();
    let mime = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let name = if mime.contains("zip") && !base.to_lowercase().ends_with(".zip") {
        format!("{base}.zip")
    } else {
        base
    };
    let size = res.content_length();
    let mut out = Response::new(Body::from_stream(res.bytes_stream()));
    set_file_headers(&mut out, &name, &mime, size, true);
    out
}

// ─────────────────────────── Settings / Filen / tokens ───────────────────────────

async fn settings_view(st: &Arc<AppState>) -> Settings {
    let public = st.public_url();
    Settings {
        filen: status::filen_status(st).await,
        root_folder: st.root_folder(),
        default_retention: st.default_retention(),
        public_url: public.clone(),
        delete_permanently: st.delete_permanently(),
        tokens: st.db.tokens(),
        mcp_url: format!("{public}/mcp"),
        version: VERSION.into(),
        umbrel: crate::umbrel::status(st),
        master_key: crate::masterkey::status(st),
    }
}

pub async fn get_settings(State(st): S, headers: HeaderMap) -> ApiResult<Settings> {
    require(&st, &headers)?;
    Ok(Json(settings_view(&st).await))
}

pub async fn put_settings(
    State(st): S,
    headers: HeaderMap,
    Json(input): Json<SettingsInput>,
) -> ApiResult<Settings> {
    require(&st, &headers)?;
    let root = input.root_folder.trim().trim_matches('/').to_string();
    if root.is_empty() || root.contains('/') {
        return Err(bad("Der Stammordner braucht einen Namen ohne „/“"));
    }
    let public = input.public_url.trim().trim_end_matches('/').to_string();
    if !public.is_empty() && reqwest::Url::parse(&public).is_err() {
        return Err(bad("Die Adresse für andere Apps ist keine gültige URL"));
    }
    if root != st.root_folder() {
        st.filen.forget_dirs();
    }
    st.db.set("root_folder", &root);
    st.db.set(
        "default_retention",
        &serde_json::to_string(&input.default_retention).unwrap(),
    );
    st.db.set("public_url", &public);
    st.db.set(
        "delete_permanently",
        if input.delete_permanently { "1" } else { "0" },
    );
    Ok(Json(settings_view(&st).await))
}

pub async fn filen_connect(
    State(st): S,
    headers: HeaderMap,
    Json(req): Json<FilenConnect>,
) -> ApiResult<FilenStatus> {
    require(&st, &headers)?;
    if req.email.trim().is_empty() || req.password.is_empty() {
        return Err(bad("Bitte E-Mail und Passwort eingeben"));
    }
    let session = st
        .filen
        .login(&req.email, &req.password, &req.two_factor_code)
        .await
        .map_err(bad)?;
    worker::store_session(&st, &session).await;
    // the password allows a new login if Filen ever ends the session (not possible with 2FA)
    if req.two_factor_code.trim().is_empty() {
        st.db.set("filen_password", &st.secrets.seal(&req.password));
    } else {
        st.db.del("filen_password");
    }
    {
        let mut fs = st.filen_status.write().await;
        fs.error = None;
    }
    worker::refresh_quota(&st).await;
    st.db.event(
        EventLevel::Info,
        None,
        &format!("Mit Filen verbunden ({})", session.email),
    );
    Ok(Json(status::filen_status(&st).await))
}

pub async fn filen_refresh(State(st): S, headers: HeaderMap) -> ApiResult<FilenStatus> {
    require(&st, &headers)?;
    worker::refresh_quota(&st).await;
    Ok(Json(status::filen_status(&st).await))
}

pub async fn filen_disconnect(State(st): S, headers: HeaderMap) -> ApiResult<FilenStatus> {
    require(&st, &headers)?;
    st.db.del("filen_session");
    st.db.del("filen_password");
    st.db.del("filen_status");
    *st.session.write().await = None;
    *st.filen_status.write().await = FilenStatus::default();
    st.filen.forget_dirs();
    st.db.event(
        EventLevel::Warn,
        None,
        "Filen getrennt – neue Backups warten lokal",
    );
    Ok(Json(status::filen_status(&st).await))
}

pub async fn create_token(
    State(st): S,
    headers: HeaderMap,
    Json(input): Json<TokenInput>,
) -> ApiResult<TokenCreated> {
    require(&st, &headers)?;
    let name = if input.name.trim().is_empty() {
        "nrzDashboard"
    } else {
        input.name.trim()
    };
    let token = secret::new_token(ACCESS_TOKEN_PREFIX);
    let info = st.db.add_token(
        name,
        &secret::token_hash(&token),
        &secret::token_hint(&token),
    );
    Ok(Json(TokenCreated { token, info }))
}

pub async fn delete_token(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> ApiResult<serde_json::Value> {
    require(&st, &headers)?;
    st.db.delete_token(id);
    Ok(empty())
}

pub async fn events(
    State(st): S,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<Vec<Event>> {
    require(&st, &headers)?;
    let limit = q
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(100u32)
        .clamp(1, 1000);
    Ok(Json(st.db.events(limit)))
}

// ─────────────────────────── Machine API ───────────────────────────

/// `POST /api/v1/backup?name=<file>` – an app delivers a backup (raw body).
pub async fn push(
    State(st): S,
    headers: HeaderMap,
    Query(q): Query<NameQuery>,
    body: Body,
) -> Response {
    let Some(Caller::Source(source)) = machine_caller(&st, &headers) else {
        return unauthorized_machine();
    };
    let name = filename_from(&q, &headers);
    match worker::ingest(
        &st,
        &source,
        &name,
        BackupOrigin::Push,
        body.into_data_stream(),
    )
    .await
    {
        Ok(acc) => (StatusCode::CREATED, Json(acc)).into_response(),
        Err(e) => bad(e),
    }
}

/// `GET /api/v1/status`
pub async fn machine_status(State(st): S, headers: HeaderMap) -> Response {
    match machine_caller(&st, &headers) {
        Some(Caller::Admin) => Json(status::overview(&st).await).into_response(),
        Some(Caller::Source(s)) => Json(status::summary(&st, &s)).into_response(),
        None => unauthorized_machine(),
    }
}

/// `GET /api/v1/backups/{id}/download` – restore: an app gets its own copy back.
pub async fn machine_download(
    State(st): S,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(caller) = machine_caller(&st, &headers) else {
        return unauthorized_machine();
    };
    let Some(b) = st.db.backup(id) else {
        return not_found();
    };
    if let Caller::Source(s) = &caller
        && s.id != b.item.source_id
    {
        return not_found();
    }
    stream_download(&st, b, raw(&q)).await
}

// ─────────────────────────── umbrelOS ───────────────────────────

pub async fn get_umbrel(State(st): S, headers: HeaderMap) -> ApiResult<UmbrelStatus> {
    require(&st, &headers)?;
    Ok(Json(crate::umbrel::status(&st)))
}

pub async fn put_umbrel(
    State(st): S,
    headers: HeaderMap,
    Json(input): Json<UmbrelConnect>,
) -> ApiResult<UmbrelStatus> {
    require(&st, &headers)?;
    let status = crate::umbrel::save(&st, &input).await.map_err(bad)?;
    if status.connected {
        st.db.event(
            EventLevel::Info,
            None,
            &format!("Mit umbrelOS verbunden ({})", status.device),
        );
    }
    Ok(Json(status))
}

pub async fn umbrel_apps(State(st): S, headers: HeaderMap) -> ApiResult<Vec<UmbrelApp>> {
    require(&st, &headers)?;
    crate::umbrel::apps(&st).await.map(Json).map_err(bad)
}

pub async fn umbrel_browse(
    State(st): S,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<UmbrelDir> {
    require(&st, &headers)?;
    let path = q.get("path").map(String::as_str).unwrap_or("/");
    crate::umbrel::browse(&st, path)
        .await
        .map(Json)
        .map_err(bad)
}

pub async fn umbrel_bulk(
    State(st): S,
    headers: HeaderMap,
    Json(bulk): Json<UmbrelBulk>,
) -> ApiResult<Vec<SourceSummary>> {
    require(&st, &headers)?;
    if bulk.apps.is_empty() && bulk.folders.is_empty() {
        return Err(bad("Bitte mindestens eine App oder einen Ordner auswählen"));
    }
    let apps = if bulk.apps.is_empty() {
        vec![]
    } else {
        crate::umbrel::apps(&st).await.map_err(bad)?
    };
    let mut created = vec![];
    for folder in &bulk.folders {
        crate::umbrel::check_path(folder).map_err(bad)?;
        let input = crate::umbrel::folder_input(folder, &bulk);
        validate(&input)?;
        let (s, _token) = insert_new_source(&st, &input)?;
        created.push(status::summary(&st, &s));
    }
    for id in &bulk.apps {
        let Some(app) = apps.iter().find(|a| &a.id == id) else {
            return Err(bad(format!("App {id} ist nicht installiert")));
        };
        let input = crate::umbrel::app_input(app, &bulk);
        if crate::umbrel::check_path(&input.pull.umbrel_path).is_err() {
            continue; // nrzBackup itself
        }
        validate(&input)?;
        let (s, _token) = insert_new_source(&st, &input)?;
        created.push(status::summary(&st, &s));
    }
    if created.is_empty() {
        return Err(bad("Keine der gewählten Apps oder Ordner kann gesichert werden"));
    }
    Ok(Json(created))
}
