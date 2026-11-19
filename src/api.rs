//! Client for the nrzBackup server API (`/api/...`, see `nrzbackup_shared`).
//! Errors are readable German messages; HTTP 401 switches to the login page.

use dioxus::prelude::*;
use gloo_net::http::{Request, RequestBuilder, Response};
use nrzbackup_shared as shared;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub type Res<T> = Result<T, String>;

/// `None` = not checked yet, `Some(false)` = login needed.
pub static AUTH: GlobalSignal<Option<bool>> = Signal::global(|| None);
/// Server version for the sidebar.
pub static VERSION: GlobalSignal<String> = Signal::global(String::new);

const NETWORK: &str = "Keine Verbindung zum Server.";

async fn check(res: Result<Response, gloo_net::Error>) -> Res<Response> {
    let res = res.map_err(|_| NETWORK.to_string())?;
    if res.ok() {
        return Ok(res);
    }
    let status = res.status();
    let msg = match res.json::<shared::ApiError>().await {
        Ok(e) => e.error,
        Err(_) => format!("Fehler vom Server (HTTP {status})."),
    };
    if status == 401 && *AUTH.peek() != Some(false) {
        *AUTH.write() = Some(false);
    }
    Err(msg)
}

async fn parse<T: DeserializeOwned>(res: Response) -> Res<T> {
    res.json::<T>()
        .await
        .map_err(|_| "Unerwartete Antwort vom Server.".to_string())
}

async fn get<T: DeserializeOwned>(url: &str) -> Res<T> {
    parse(check(Request::get(url).send().await).await?).await
}

async fn send<B: Serialize, T: DeserializeOwned>(req: RequestBuilder, body: &B) -> Res<T> {
    let req = req.json(body).map_err(|_| NETWORK.to_string())?;
    parse(check(req.send().await).await?).await
}

async fn post<T: DeserializeOwned>(url: &str) -> Res<T> {
    parse(check(Request::post(url).send().await).await?).await
}

async fn delete(url: &str) -> Res<()> {
    check(Request::delete(url).send().await).await.map(|_| ())
}

fn enc(s: &str) -> String {
    String::from(js_sys::encode_uri_component(s))
}

// ── Session ──

pub async fn session() -> Res<shared::SessionInfo> {
    get("/api/session").await
}

pub async fn login(username: &str, password: &str) -> Res<shared::SessionInfo> {
    let body = shared::LoginRequest {
        username: username.into(),
        password: password.into(),
    };
    send(Request::post("/api/login"), &body).await
}

pub async fn logout() -> Res<()> {
    check(Request::post("/api/logout").send().await)
        .await
        .map(|_| ())
}

// ── Overview, sources, backups ──

pub async fn overview() -> Res<shared::Overview> {
    get("/api/overview").await
}

pub async fn sources() -> Res<Vec<shared::SourceSummary>> {
    get("/api/sources").await
}

pub async fn integration(id: i64) -> Res<shared::IntegrationPrompt> {
    get(&format!("/api/sources/{id}/integration")).await
}

pub async fn source(id: i64) -> Res<shared::SourceDetail> {
    get(&format!("/api/sources/{id}")).await
}

pub async fn create_source(input: &shared::SourceInput) -> Res<shared::SourceCreated> {
    send(Request::post("/api/sources"), input).await
}

pub async fn update_source(id: i64, input: &shared::SourceInput) -> Res<shared::SourceSummary> {
    send(Request::put(&format!("/api/sources/{id}")), input).await
}

pub async fn delete_source(id: i64) -> Res<()> {
    delete(&format!("/api/sources/{id}")).await
}

pub async fn new_source_token(id: i64) -> Res<shared::TokenCreated> {
    post(&format!("/api/sources/{id}/token")).await
}

pub async fn request_backup(id: i64) -> Res<shared::ActionResult> {
    post(&format!("/api/sources/{id}/request")).await
}

pub async fn prune(id: i64) -> Res<shared::ActionResult> {
    post(&format!("/api/sources/{id}/prune")).await
}

/// Manual upload of a file chosen in the browser (raw body).
pub async fn upload(id: i64, file: &web_sys::File) -> Res<shared::BackupAccepted> {
    let url = format!("/api/sources/{id}/upload?name={}", enc(&file.name()));
    let req = Request::post(&url)
        .header("Content-Type", "application/octet-stream")
        .body(wasm_bindgen::JsValue::from(file.clone()))
        .map_err(|_| NETWORK.to_string())?;
    parse(check(req.send().await).await?).await
}

pub async fn delete_backup(id: i64) -> Res<()> {
    delete(&format!("/api/backups/{id}")).await
}

pub async fn retry_backup(id: i64) -> Res<shared::ActionResult> {
    post(&format!("/api/backups/{id}/retry")).await
}

pub fn download_url(id: i64) -> String {
    format!("/api/backups/{id}/download")
}

// ── Settings, Filen, tokens, events ──

pub async fn settings() -> Res<shared::Settings> {
    get("/api/settings").await
}

pub async fn save_settings(input: &shared::SettingsInput) -> Res<shared::Settings> {
    send(Request::put("/api/settings"), input).await
}

pub async fn filen_connect(input: &shared::FilenConnect) -> Res<shared::FilenStatus> {
    send(Request::post("/api/filen/connect"), input).await
}

pub async fn filen_refresh() -> Res<shared::FilenStatus> {
    post("/api/filen/refresh").await
}

pub async fn filen_disconnect() -> Res<shared::FilenStatus> {
    post("/api/filen/disconnect").await
}

pub async fn create_token(name: &str) -> Res<shared::TokenCreated> {
    let body = shared::TokenInput { name: name.into() };
    send(Request::post("/api/tokens"), &body).await
}

pub async fn delete_token(id: i64) -> Res<()> {
    delete(&format!("/api/tokens/{id}")).await
}

pub async fn events(limit: u32) -> Res<Vec<shared::Event>> {
    get(&format!("/api/events?limit={limit}")).await
}

// ── umbrelOS ──

pub async fn umbrel_connect(input: &shared::UmbrelConnect) -> Res<shared::UmbrelStatus> {
    send(Request::put("/api/umbrel"), input).await
}

pub async fn umbrel_apps() -> Res<Vec<shared::UmbrelApp>> {
    get("/api/umbrel/apps").await
}

pub async fn umbrel_browse(path: &str) -> Res<shared::UmbrelDir> {
    get(&format!("/api/umbrel/browse?path={}", enc(path))).await
}

pub async fn umbrel_sources(input: &shared::UmbrelBulk) -> Res<Vec<shared::SourceSummary>> {
    send(Request::post("/api/umbrel/sources"), input).await
}

// ── Explorer ──

pub async fn filen_browse(path: &str) -> Res<shared::FilenDir> {
    get(&format!("/api/filen/browse?path={}", enc(path))).await
}

/// Download link of a Filen file (`inline` opens it in the browser).
pub fn filen_download_url(folder: &str, uuid: &str, inline: bool) -> String {
    format!(
        "/api/filen/download?folder={}&uuid={}{}",
        enc(folder),
        enc(uuid),
        if inline { "&inline=1" } else { "" }
    )
}

/// Download link of an umbrelOS file or folder (folders come as ZIP).
pub fn umbrel_download_url(path: &str) -> String {
    format!("/api/umbrel/download?path={}", enc(path))
}

// ── Master key / stopping apps ──

/// Shows the master key after checking the app password.
pub async fn masterkey_reveal(password: &str) -> Res<shared::MasterKeySecret> {
    send(
        Request::post("/api/masterkey/reveal"),
        &shared::MasterKeyReveal { password: password.into() },
    )
    .await
}

// ── Statistics / calendar ──

pub async fn stats() -> Res<shared::Stats> {
    get("/api/stats").await
}

pub async fn calendar(from: i64, to: i64) -> Res<Vec<shared::BackupItem>> {
    get(&format!("/api/calendar?from={from}&to={to}")).await
}

pub async fn backup_all() -> Res<shared::ActionResult> {
    post("/api/backup-all").await
}

pub async fn masterkey_confirm() -> Res<()> {
    check(Request::post("/api/masterkey/confirm").send().await)
        .await
        .map(|_| ())
}

pub async fn set_stop_app(id: i64, stop_app: bool) -> Res<shared::SourceDetail> {
    send(
        Request::put(&format!("/api/sources/{id}/stop")),
        &shared::StopAppInput { stop_app },
    )
    .await
}
