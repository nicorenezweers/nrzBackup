//! API types shared by the nrzBackup server and the web app.
//!
//! All timestamps are Unix seconds (UTC). Sizes are bytes.
//!
//! UI API (session cookie `nrzbackup_session`, JSON):
//!
//! | Method + path                      | Body              | Answer            |
//! |------------------------------------|-------------------|-------------------|
//! | `GET  /api/session`                |                   | [`SessionInfo`]   |
//! | `POST /api/login`                  | [`LoginRequest`]  | [`SessionInfo`]   |
//! | `POST /api/logout`                 |                   | `{}`              |
//! | `GET  /api/overview`               |                   | [`Overview`]      |
//! | `GET  /api/sources`                |                   | `Vec<SourceSummary>` |
//! | `POST /api/sources`                | [`SourceInput`]   | [`SourceCreated`] |
//! | `GET  /api/sources/{id}`           |                   | [`SourceDetail`]  |
//! | `PUT  /api/sources/{id}`           | [`SourceInput`]   | [`SourceSummary`] |
//! | `DELETE /api/sources/{id}`         |                   | `{}` (copies in Filen stay) |
//! | `POST /api/sources/{id}/token`     |                   | [`TokenCreated`] (new push token, old one invalid) |
//! | `POST /api/sources/{id}/request`   |                   | [`ActionResult`] (asks the app for a backup now) |
//! | `POST /api/sources/{id}/upload?name=<file>` | raw file bytes | [`BackupAccepted`] (manual upload) |
//! | `POST /api/sources/{id}/prune`     |                   | [`ActionResult`] (apply retention now) |
//! | `GET  /api/sources/{id}/integration` |                 | [`IntegrationPrompt`] (prompt for a coding agent, no token) |
//! | `DELETE /api/backups/{id}`         |                   | `{}` (deletes the copy in Filen) |
//! | `GET  /api/backups/{id}/download`  |                   | file (decrypted from Filen) |
//! | `POST /api/backups/{id}/retry`     |                   | [`ActionResult`]  |
//! | `GET  /api/settings`               |                   | [`Settings`]      |
//! | `PUT  /api/settings`               | [`SettingsInput`] | [`Settings`]      |
//! | `POST /api/filen/connect`          | [`FilenConnect`]  | [`FilenStatus`]   |
//! | `POST /api/filen/refresh`          |                   | [`FilenStatus`]   |
//! | `POST /api/filen/disconnect`       |                   | [`FilenStatus`]   |
//! | `POST /api/tokens`                 | [`TokenInput`]    | [`TokenCreated`]  |
//! | `DELETE /api/tokens/{id}`          |                   | `{}`              |
//! | `GET  /api/events?limit=100`       |                   | `Vec<Event>`      |
//! | `GET  /api/umbrel`                 |                   | [`UmbrelStatus`]  |
//! | `PUT  /api/umbrel`                 | [`UmbrelConnect`] | [`UmbrelStatus`] (saves, then tests) |
//! | `GET  /api/umbrel/apps`            |                   | `Vec<UmbrelApp>`  |
//! | `GET  /api/umbrel/browse?path=/Home` |                 | [`UmbrelDir`]     |
//! | `POST /api/umbrel/sources`         | [`UmbrelBulk`]    | `Vec<SourceSummary>` |
//! | `GET  /api/umbrel/download?path=/Home/x` |             | file (folders as .tar.gz) |
//! | `POST /api/masterkey/reveal`       | [`MasterKeyReveal`] | [`MasterKeySecret`] (needs the app password) |
//! | `POST /api/masterkey/saved`        | form (username, password) | 303 → `/settings` (lets the browser offer to save the key in Google / iCloud passwords) |
//! | `GET  /api/stats`                  |                   | [`Stats`] (storage, app coverage, forecast) |
//! | `GET  /api/calendar?from=&to=`     |                   | `Vec<BackupItem>` (backups in that time range) |
//! | `POST /api/backup-all`             |                   | [`ActionResult`] (requests a backup from every source that can be asked) |
//! | `POST /api/masterkey/confirm`      |                   | `{}`              |
//! | `PUT  /api/sources/{id}/stop`      | [`StopAppInput`]  | [`SourceDetail`]  |
//! | `GET  /api/filen/browse?path=/nrzBackup` |             | [`FilenDir`]      |
//! | `GET  /api/filen/download?folder=<uuid>&uuid=<uuid>` |  | file (decrypted)  |
//!
//! Errors: HTTP 4xx/5xx with [`ApiError`] as body. HTTP 401 = not logged in.
//!
//! Machine API (Bearer token): `POST /api/v1/backup?name=<file>` (push, raw body),
//! `GET /api/v1/status`, `GET /api/v1/backups/{id}/download`, `POST /mcp` (MCP).

use serde::{Deserialize, Serialize};

/// Prefix of push tokens of a source (an app that sends backups).
pub const SOURCE_TOKEN_PREFIX: &str = "nrzbk_";
/// Prefix of access tokens for nrzDashboard and other MCP clients (status + control).
pub const ACCESS_TOKEN_PREFIX: &str = "nrzbka_";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiError {
    pub error: String,
}

// ─────────────────────────────── Session ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SessionInfo {
    pub authenticated: bool,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LoginRequest {
    /// Always `admin`; empty is accepted too.
    #[serde(default)]
    pub username: String,
    pub password: String,
}

// ─────────────────────────────── Health ───────────────────────────────

/// Traffic light for the dashboard.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    /// Green: everything fine.
    Ok,
    /// Nothing to judge yet (no sources, no expectation set).
    #[default]
    Unknown,
    /// Yellow: works, but needs attention (overdue, retrying upload, pull failed).
    Warning,
    /// Red: backups are not reaching Filen.
    Error,
}

impl Health {
    pub fn label(self) -> &'static str {
        match self {
            Health::Ok => "Alles gut",
            Health::Unknown => "Unbekannt",
            Health::Warning => "Warnung",
            Health::Error => "Fehler",
        }
    }
}

// ─────────────────────────────── Retention ───────────────────────────────

/// How many copies stay in Filen (grandfather-father-son). A copy is kept when
/// any rule wants it; everything else is deleted after a new upload succeeded.
/// The newest copy is always kept.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Retention {
    /// The newest N copies, no matter when.
    pub keep_last: u32,
    /// Newest copy of each of the last N days that have a copy.
    pub daily: u32,
    /// Newest copy of each of the last N ISO weeks that have a copy.
    pub weekly: u32,
    /// Newest copy of each of the last N months that have a copy.
    pub monthly: u32,
    /// Newest copy of each of the last N years that have a copy.
    pub yearly: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            keep_last: 1,
            daily: 7,
            weekly: 4,
            monthly: 12,
            yearly: 2,
        }
    }
}

// ─────────────────────────────── Pull ───────────────────────────────

/// How nrzBackup asks an app for a backup ("send me a backup").
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PullKind {
    /// The app only pushes by itself.
    #[default]
    None,
    /// HTTP request; the response body is the backup file.
    Http,
    /// MCP tool call (`tools/call`) on the app's MCP server.
    Mcp,
    /// umbrelOS itself: nrzBackup downloads an app's data or any folder through the
    /// umbrelOS file API (as ZIP) – needs the umbrelOS connection in the settings.
    Umbrel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleMode {
    /// Only on request (button, MCP).
    #[default]
    Off,
    /// Every day at `time` (local time of the server, `TZ`).
    Daily,
    /// Every `every_hours` hours.
    Interval,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Schedule {
    pub mode: ScheduleMode,
    /// `HH:MM`, used with [`ScheduleMode::Daily`].
    pub time: String,
    /// Used with [`ScheduleMode::Interval`], at least 1.
    pub every_hours: u32,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule {
            mode: ScheduleMode::Off,
            time: "03:00".into(),
            every_hours: 24,
        }
    }
}

/// Pull settings as entered in the UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PullInput {
    pub kind: PullKind,
    /// HTTP: URL that returns the file. MCP: URL of the MCP endpoint (e.g. `http://nrz-budget_app_1:8080/mcp`).
    pub url: String,
    /// Bearer token for the app. Empty on update = keep the stored one.
    pub token: String,
    /// Set to true to remove the stored token.
    pub clear_token: bool,
    /// HTTP only: `GET` or `POST`.
    pub method: String,
    /// MCP only: tool name, default `create_backup`.
    pub tool: String,
    /// MCP only: extra tool arguments as JSON object text (may be empty).
    pub arguments: String,
    pub schedule: Schedule,
    /// umbrelOS only: virtual path to back up, e.g. `/Apps/nrz-budget/data` or `/Home/Documents`.
    #[serde(default)]
    pub umbrel_path: String,
    /// umbrelOS only: app ID when an app is backed up (for stopping it), empty for a plain folder.
    #[serde(default)]
    pub umbrel_app: String,
    /// umbrelOS only: stop the app during the backup (consistent database) and start it again.
    #[serde(default)]
    pub stop_app: bool,
}

/// Pull settings as shown (token never leaves the server).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PullView {
    pub kind: PullKind,
    pub url: String,
    pub has_token: bool,
    pub method: String,
    pub tool: String,
    pub arguments: String,
    pub schedule: Schedule,
    pub last_pull: Option<i64>,
    pub next_pull: Option<i64>,
    /// Error of the last pull, `None` if it worked.
    pub last_error: Option<String>,
    #[serde(default)]
    pub umbrel_path: String,
    #[serde(default)]
    pub umbrel_app: String,
    #[serde(default)]
    pub stop_app: bool,
}

// ─────────────────────────────── Sources ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SourceInput {
    pub name: String,
    pub description: String,
    pub retention: Retention,
    /// A backup is expected at least every N hours; 0 = no expectation.
    /// Overdue → yellow, more than twice overdue → red.
    pub expect_hours: u32,
    pub pull: PullInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SourceSummary {
    pub id: i64,
    /// URL-safe name, also the folder name in Filen.
    pub slug: String,
    pub name: String,
    pub description: String,
    pub health: Health,
    pub health_message: String,
    /// Copies currently stored in Filen.
    pub copies: u32,
    /// Bytes of those copies.
    pub stored_bytes: u64,
    /// Waiting in the local queue (not yet in Filen).
    pub queued: u32,
    pub last_backup: Option<i64>,
    pub last_error: Option<String>,
    pub retention: Retention,
    pub expect_hours: u32,
    pub pull: PullView,
    /// Start of the push token, e.g. `nrzbk_3f9a…`.
    pub token_hint: String,
    /// Folder in Filen, e.g. `/nrzBackup/nrzbudget`.
    pub filen_path: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SourceCreated {
    pub source: SourceSummary,
    /// The push token in clear text – shown exactly once.
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SourceDetail {
    pub source: SourceSummary,
    /// Newest first, including queued, failed and deleted ones (last 200).
    pub backups: Vec<BackupItem>,
    /// Ready-to-copy examples for this source (curl, MCP).
    pub push_url: String,
    pub mcp_url: String,
}

/// Ready-to-paste instructions for an LLM / coding agent that integrates nrzBackup into an
/// app (push, pull, restore, settings UI, tests). Never contains a token.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct IntegrationPrompt {
    pub text: String,
}

// ─────────────────────────────── Backups ───────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackupStatus {
    /// Received, waiting in the local queue (also while retrying).
    #[default]
    Queued,
    Uploading,
    /// Encrypted copy is in Filen, local file is gone.
    Stored,
    /// Removed by retention or by hand.
    Deleted,
    /// Gave up (only for invalid input, uploads are retried forever).
    Failed,
}

impl BackupStatus {
    pub fn label(self) -> &'static str {
        match self {
            BackupStatus::Queued => "Wartet",
            BackupStatus::Uploading => "Wird hochgeladen",
            BackupStatus::Stored => "In Filen",
            BackupStatus::Deleted => "Gelöscht",
            BackupStatus::Failed => "Fehlgeschlagen",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackupOrigin {
    /// The app sent it (HTTP push).
    #[default]
    Push,
    /// nrzBackup asked for it.
    Pull,
    /// Sent through MCP.
    Mcp,
    /// Uploaded by hand in the UI.
    Upload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BackupItem {
    pub id: i64,
    pub source_id: i64,
    pub source_name: String,
    /// Original file name as sent by the app.
    pub filename: String,
    /// Name in Filen (`<slug>_<YYYY-MM-DD_HHMMSS>_<filename>`).
    pub stored_name: String,
    pub size: u64,
    /// SHA-256 of the plain file, hex.
    pub sha256: String,
    pub created_at: i64,
    pub stored_at: Option<i64>,
    pub deleted_at: Option<i64>,
    pub status: BackupStatus,
    pub origin: BackupOrigin,
    pub attempts: u32,
    pub error: Option<String>,
    /// Retention rules that keep this copy: "last", "daily", "weekly", "monthly", "yearly".
    pub kept_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BackupAccepted {
    pub id: i64,
    pub filename: String,
    pub size: u64,
    pub sha256: String,
    pub status: BackupStatus,
    pub message: String,
}

// ─────────────────────────────── Filen ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FilenStatus {
    pub connected: bool,
    pub email: String,
    pub auth_version: u32,
    pub storage_used: u64,
    pub storage_max: u64,
    /// Root folder in Filen, e.g. `nrzBackup`.
    pub root_folder: String,
    pub error: Option<String>,
    pub checked_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FilenConnect {
    pub email: String,
    pub password: String,
    /// 6-digit code if two-factor authentication is on, otherwise empty.
    pub two_factor_code: String,
}

// ─────────────────────────────── Overview ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct QueueInfo {
    pub queued: u32,
    pub uploading: u32,
    /// Queued items that already failed at least once.
    pub retrying: u32,
    /// Bytes waiting locally.
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Overview {
    pub health: Health,
    /// One sentence for the dashboard, e.g. "Alle 3 Quellen gesichert".
    pub message: String,
    pub filen: FilenStatus,
    pub sources: Vec<SourceSummary>,
    /// Last 20 backups over all sources.
    pub recent: Vec<BackupItem>,
    pub queue: QueueInfo,
    /// Copies stored in Filen over all sources.
    pub copies: u32,
    pub stored_bytes: u64,
    pub last_backup: Option<i64>,
    pub version: String,
}

// ─────────────────────────────── Settings ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TokenInfo {
    pub id: i64,
    pub name: String,
    pub hint: String,
    pub created_at: i64,
    pub last_used: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TokenInput {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct TokenCreated {
    pub token: String,
    pub info: TokenInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Settings {
    pub filen: FilenStatus,
    pub root_folder: String,
    /// Retention for new sources.
    pub default_retention: Retention,
    /// Address under which other apps reach nrzBackup (push URL, callbacks),
    /// default `http://nrz-backup_app_1:8080`.
    pub public_url: String,
    /// Delete old copies permanently (true) or move them to the Filen trash (false).
    pub delete_permanently: bool,
    /// Access tokens for nrzDashboard / MCP clients.
    pub tokens: Vec<TokenInfo>,
    pub mcp_url: String,
    pub version: String,
    /// Connection to umbrelOS (for backing up apps and folders).
    #[serde(default)]
    pub umbrel: UmbrelStatus,
    /// The master key every backup is encrypted with.
    #[serde(default)]
    pub master_key: MasterKeyStatus,
}

/// The master key (age X25519). The secret part is only sent by `GET /api/masterkey`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MasterKeyStatus {
    pub exists: bool,
    /// `age1…` – public part, used for encrypting.
    pub public_key: String,
    /// The user confirmed that the key is stored safely.
    pub confirmed: bool,
    pub created_at: Option<i64>,
}

/// `GET /api/masterkey`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MasterKeySecret {
    /// `AGE-SECRET-KEY-1…`
    pub secret_key: String,
    pub public_key: String,
}

/// `POST /api/masterkey/reveal`: the master key is only shown after entering the app password.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MasterKeyReveal {
    pub password: String,
}

/// `PUT /api/sources/{id}/stop`: stop the app during its backups.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct StopAppInput {
    pub stop_app: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SettingsInput {
    pub root_folder: String,
    pub default_retention: Retention,
    pub public_url: String,
    pub delete_permanently: bool,
}

// ─────────────────────────────── umbrelOS ───────────────────────────────

/// Connection to the umbrelOS MCP server and file API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UmbrelStatus {
    /// e.g. `http://host.docker.internal` (from inside the app container).
    pub url: String,
    /// Folders read directly without a token, e.g. `["/Home", "/Apps"]`.
    #[serde(default)]
    pub local: Vec<String>,
    pub has_token: bool,
    /// `umbrelmcp_54a6…`
    pub token_hint: String,
    pub connected: bool,
    /// e.g. "umbrelOS 2.1 · umbrel".
    pub device: String,
    pub error: Option<String>,
}

/// `PUT /api/umbrel`: save and test the connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UmbrelConnect {
    pub url: String,
    /// Empty = keep the stored token.
    pub token: String,
    /// Remove the stored token.
    pub clear: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UmbrelApp {
    pub id: String,
    pub name: String,
    pub version: String,
    /// "ready", "stopped", …
    pub state: String,
    /// `/Apps/<id>/data` when the app has a data folder, else `/Apps/<id>`.
    pub data_path: String,
    /// `/Apps/<id>`
    pub app_path: String,
    /// Already backed up by this source (id), if any.
    pub source_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UmbrelEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    /// Last change, ms since the epoch.
    #[serde(default)]
    pub modified: Option<i64>,
}

/// `GET /api/umbrel/browse?path=/Home`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UmbrelDir {
    pub path: String,
    /// `None` at the root `/`.
    pub parent: Option<String>,
    /// Folders first, then files, by name.
    pub entries: Vec<UmbrelEntry>,
}

// ─────────────────────────────── Filen explorer ───────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FilenEntry {
    pub uuid: String,
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    /// Last change, ms since the epoch.
    pub modified: Option<i64>,
    pub mime: String,
}

/// `GET /api/filen/browse?path=/nrzBackup`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct FilenDir {
    /// Normalised path, `/` = the drive root.
    pub path: String,
    /// Folder uuid (needed for downloads).
    pub uuid: String,
    pub parent: Option<String>,
    /// Folders first, then files, by name.
    pub entries: Vec<FilenEntry>,
}

/// `POST /api/umbrel/sources`: create one source per app.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct UmbrelBulk {
    pub apps: Vec<String>,
    /// Folders such as `/Home/Documents`, one source each.
    #[serde(default)]
    pub folders: Vec<String>,
    /// true = whole `/Apps/<id>` folder, false = `data_path`.
    pub whole_folder: bool,
    /// Stop every selected app (old clients); see `stop_apps`.
    pub stop_app: bool,
    /// Apps that are stopped during their backups.
    #[serde(default)]
    pub stop_apps: Vec<String>,
    pub schedule: Schedule,
    pub expect_hours: u32,
    pub retention: Retention,
}

// ─────────────────────────────── Events ───────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum EventLevel {
    #[default]
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Event {
    pub id: i64,
    pub ts: i64,
    pub level: EventLevel,
    pub source_id: Option<i64>,
    pub source_name: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ActionResult {
    pub ok: bool,
    pub message: String,
}

// ─────────────────────────────── Statistics ───────────────────────────────

/// `GET /api/stats`: storage, how many umbrelOS apps are backed up and a forecast.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Stats {
    /// Bytes of all copies in Filen.
    pub stored_bytes: u64,
    pub copies: u32,
    pub filen_connected: bool,
    pub storage_used: u64,
    pub storage_max: u64,
    /// `storage_max - storage_used`.
    pub storage_free: u64,
    pub coverage: AppCoverage,
    pub sources: Vec<SourceForecast>,
    /// Forecast for all sources together (with the measured growth).
    pub forecast: CalcResult,
    /// The input the forecast was made with (prefills the calculator).
    pub forecast_input: CalcInput,
}

/// How many of the installed umbrelOS apps are backed up.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AppCoverage {
    /// false = umbrelOS is not reachable (see `error`).
    pub available: bool,
    pub error: Option<String>,
    pub total: u32,
    /// Apps with a source in nrzBackup.
    pub configured: u32,
    /// Apps with at least one backup.
    pub backed_up: u32,
    /// `backed_up / total * 100`.
    pub percent: f64,
    pub apps: Vec<AppCoverageItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AppCoverageItem {
    pub id: String,
    pub name: String,
    pub source_id: Option<i64>,
    pub last_backup: Option<i64>,
    pub backed_up: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SourceForecast {
    pub source_id: i64,
    pub name: String,
    /// Size of the newest backup.
    pub latest_size: u64,
    pub stored_bytes: u64,
    pub copies: u32,
    /// Days between two backups (measured, else from the schedule).
    pub interval_days: f64,
    /// Copies the retention keeps once it is "full".
    pub steady_copies: u32,
    /// `latest_size * steady_copies`.
    pub projected_bytes: u64,
    /// Measured growth of the backup size in percent per month.
    pub growth_percent_month: f64,
}

/// Input of the backup calculator.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct CalcInput {
    /// Size of one backup (all sources together).
    pub backup_size: u64,
    /// Copies that are kept.
    pub copies: u32,
    /// Growth of the backup size in percent per month.
    pub growth_percent_month: f64,
    /// Used in Filen right now (backups and everything else).
    pub storage_used: u64,
    pub storage_max: u64,
    /// Of `storage_used`, the bytes that are nrzBackup copies.
    pub current_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct CalcResult {
    /// Space the copies need once the retention is full.
    pub projected_bytes: u64,
    /// `projected_bytes - current_bytes` (negative = it gets smaller).
    pub extra_bytes: i64,
    /// Free in Filen after that (negative = does not fit).
    pub free_after: i64,
    pub fits: bool,
    /// Months until Filen is full with the given growth; `None` = never (no growth).
    pub months_until_full: Option<f64>,
    /// Space needed in 12 months with the growth.
    pub in_12_months: u64,
}

/// Rough number of copies a retention keeps when backups come every `interval_days`.
/// Rules overlap (the newest copy counts for all of them), so the finer rules eat into
/// the coarser ones.
pub fn steady_copies(r: &Retention, interval_days: f64) -> u32 {
    let i = if interval_days.is_finite() && interval_days > 0.0 { interval_days.max(1.0 / 24.0) } else { 1.0 };
    let rules = [
        (r.keep_last, i),
        (r.daily, i.max(1.0)),
        (r.weekly, i.max(7.0)),
        (r.monthly, i.max(30.44)),
        (r.yearly, i.max(365.25)),
    ];
    let (mut span, mut total) = (0.0f64, 0.0f64);
    for (n, p) in rules {
        if n == 0 {
            continue;
        }
        let n = f64::from(n);
        total += n - (span / p).floor().min(n);
        span = span.max(n * p);
    }
    total.max(1.0).round() as u32
}

/// The backup calculator: how much space the copies need and when Filen is full.
pub fn calculate(i: &CalcInput) -> CalcResult {
    let projected = i.backup_size.saturating_mul(u64::from(i.copies.max(1)));
    let others = i.storage_used.saturating_sub(i.current_bytes);
    let room = i.storage_max as i64 - others as i64;
    let free_after = room - projected as i64;
    let g = (i.growth_percent_month / 100.0).max(0.0);
    let fits = i.storage_max == 0 || free_after >= 0;
    let months_until_full = if i.storage_max == 0 {
        None
    } else if !fits {
        Some(0.0)
    } else if g > 0.0 && projected > 0 {
        Some(((room as f64) / projected as f64).ln() / (1.0 + g).ln())
    } else {
        None
    };
    CalcResult {
        projected_bytes: projected,
        extra_bytes: projected as i64 - i.current_bytes as i64,
        free_after,
        fits,
        months_until_full,
        in_12_months: (projected as f64 * (1.0 + g).powi(12)) as u64,
    }
}

// ─────────────────────────────── Helpers ───────────────────────────────

/// `1536` → `1.5 KB` (decimal units, like Filen shows them).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Lowercase ASCII slug for folder names: `nrzBudget DB` → `nrzbudget-db`.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = match c {
            'ä' | 'Ä' => 'a',
            'ö' | 'Ö' => 'o',
            'ü' | 'Ü' => 'u',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() { "quelle".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1500), "1.5 KB");
        assert_eq!(format_bytes(250_000_000), "250 MB");
    }

    #[test]
    fn calc() {
        let r = Retention::default();
        let c = steady_copies(&r, 1.0);
        assert!((20..=24).contains(&c), "{c}");
        assert_eq!(steady_copies(&Retention { keep_last: 3, daily: 0, weekly: 0, monthly: 0, yearly: 0 }, 1.0), 3);
        let res = calculate(&CalcInput {
            backup_size: 1_000,
            copies: 10,
            growth_percent_month: 0.0,
            storage_used: 5_000,
            storage_max: 20_000,
            current_bytes: 3_000,
        });
        assert_eq!(res.projected_bytes, 10_000);
        assert_eq!(res.extra_bytes, 7_000);
        assert_eq!(res.free_after, 8_000);
        assert!(res.fits);
        assert_eq!(res.months_until_full, None);
        let grow = calculate(&CalcInput { growth_percent_month: 10.0, ..Default::default() });
        assert!(grow.fits);
        let full = calculate(&CalcInput { backup_size: 10, copies: 1, growth_percent_month: 100.0, storage_max: 40, ..Default::default() });
        assert!((full.months_until_full.unwrap() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn slug() {
        assert_eq!(slugify("nrzBudget DB"), "nrzbudget-db");
        assert_eq!(slugify("  Größe!! "), "gro-e");
        assert_eq!(slugify("***"), "quelle");
    }
}
