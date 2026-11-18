//! SQLite storage: sources, backup records (the files themselves live in Filen), tokens,
//! sessions, events and settings.

use std::path::Path;
use std::sync::Mutex;

use nrzbackup_shared::*;
use rusqlite::{Connection, OptionalExtension, Row, params};

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub struct Db {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub token_hint: String,
    pub retention: Retention,
    pub expect_hours: u32,
    pub pull_kind: PullKind,
    pub pull_url: String,
    /// sealed
    pub pull_token: String,
    pub pull_method: String,
    pub pull_tool: String,
    pub pull_args: String,
    pub schedule: Schedule,
    pub last_pull: Option<i64>,
    pub next_pull: Option<i64>,
    pub pull_error: Option<String>,
    pub created_at: i64,
    /// umbrelOS: path to download (app data or folder)
    pub umbrel_path: String,
    /// umbrelOS: app ID (empty for a plain folder)
    pub umbrel_app: String,
    /// umbrelOS: stop the app during the backup
    pub stop_app: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Backup {
    pub item: BackupItem,
    pub spool: Option<String>,
    pub next_try: i64,
    pub filen_uuid: Option<String>,
    /// sealed
    pub filen_key: String,
    pub region: String,
    pub bucket: String,
    pub chunks: u64,
}

fn enum_to_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn str_to_enum<T: serde::de::DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or_default()
}

const SOURCE_COLS: &str = "id, slug, name, description, token_hint, retention, expect_hours, pull_kind, pull_url, \
    pull_token, pull_method, pull_tool, pull_args, schedule, last_pull, next_pull, pull_error, created_at, umbrel_path, umbrel_app, stop_app";

fn source_row(r: &Row) -> rusqlite::Result<Source> {
    Ok(Source {
        id: r.get(0)?,
        slug: r.get(1)?,
        name: r.get(2)?,
        description: r.get(3)?,
        token_hint: r.get(4)?,
        retention: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
        expect_hours: r.get(6)?,
        pull_kind: str_to_enum(&r.get::<_, String>(7)?),
        pull_url: r.get(8)?,
        pull_token: r.get(9)?,
        pull_method: r.get(10)?,
        pull_tool: r.get(11)?,
        pull_args: r.get(12)?,
        schedule: serde_json::from_str(&r.get::<_, String>(13)?).unwrap_or_default(),
        last_pull: r.get(14)?,
        next_pull: r.get(15)?,
        pull_error: r.get(16)?,
        created_at: r.get(17)?,
        umbrel_path: r.get(18)?,
        umbrel_app: r.get(19)?,
        stop_app: r.get::<_, i64>(20)? != 0,
    })
}

const BACKUP_COLS: &str = "b.id, b.source_id, COALESCE(s.name, ''), b.filename, b.stored_name, b.size, b.sha256, b.created_at, \
    b.stored_at, b.deleted_at, b.status, b.origin, b.attempts, b.error, b.kept_by, b.spool, b.next_try, b.filen_uuid, \
    b.filen_key, b.filen_region, b.filen_bucket, b.filen_chunks";

fn backup_row(r: &Row) -> rusqlite::Result<Backup> {
    let kept: String = r.get(14)?;
    Ok(Backup {
        item: BackupItem {
            id: r.get(0)?,
            source_id: r.get(1)?,
            source_name: r.get(2)?,
            filename: r.get(3)?,
            stored_name: r.get(4)?,
            size: r.get::<_, i64>(5)? as u64,
            sha256: r.get(6)?,
            created_at: r.get(7)?,
            stored_at: r.get(8)?,
            deleted_at: r.get(9)?,
            status: str_to_enum(&r.get::<_, String>(10)?),
            origin: str_to_enum(&r.get::<_, String>(11)?),
            attempts: r.get(12)?,
            error: r.get(13)?,
            kept_by: kept
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        },
        spool: r.get(15)?,
        next_try: r.get(16)?,
        filen_uuid: r.get(17)?,
        filen_key: r.get(18)?,
        region: r.get(19)?,
        bucket: r.get(20)?,
        chunks: r.get::<_, i64>(21)? as u64,
    })
}

pub struct NewBackup<'a> {
    pub source_id: i64,
    pub filename: &'a str,
    pub stored_name: &'a str,
    pub size: u64,
    pub sha256: &'a str,
    pub origin: BackupOrigin,
    pub spool: &'a str,
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Db> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS sources (
                id INTEGER PRIMARY KEY,
                slug TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                token_hash TEXT NOT NULL UNIQUE,
                token_hint TEXT NOT NULL,
                retention TEXT NOT NULL,
                expect_hours INTEGER NOT NULL DEFAULT 0,
                pull_kind TEXT NOT NULL DEFAULT 'none',
                pull_url TEXT NOT NULL DEFAULT '',
                pull_token TEXT NOT NULL DEFAULT '',
                pull_method TEXT NOT NULL DEFAULT 'GET',
                pull_tool TEXT NOT NULL DEFAULT 'create_backup',
                pull_args TEXT NOT NULL DEFAULT '',
                schedule TEXT NOT NULL DEFAULT '{}',
                last_pull INTEGER,
                next_pull INTEGER,
                pull_error TEXT,
                created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS backups (
                id INTEGER PRIMARY KEY,
                source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
                filename TEXT NOT NULL,
                stored_name TEXT NOT NULL,
                size INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                stored_at INTEGER,
                deleted_at INTEGER,
                status TEXT NOT NULL,
                origin TEXT NOT NULL,
                attempts INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                kept_by TEXT NOT NULL DEFAULT '',
                spool TEXT,
                next_try INTEGER NOT NULL DEFAULT 0,
                filen_uuid TEXT,
                filen_key TEXT NOT NULL DEFAULT '',
                filen_region TEXT NOT NULL DEFAULT '',
                filen_bucket TEXT NOT NULL DEFAULT '',
                filen_chunks INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS backups_source ON backups(source_id, created_at);
             CREATE INDEX IF NOT EXISTS backups_status ON backups(status);
             CREATE TABLE IF NOT EXISTS tokens (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                hint TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                last_used INTEGER
             );
             CREATE TABLE IF NOT EXISTS sessions (token_hash TEXT PRIMARY KEY, expires INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS tickets (
                token_hash TEXT PRIMARY KEY,
                source_id INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
                expires INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS events (
                id INTEGER PRIMARY KEY,
                ts INTEGER NOT NULL,
                level TEXT NOT NULL,
                source_id INTEGER,
                message TEXT NOT NULL
             );",
        )?;
        // columns added after 1.0.4 – existing databases get them on start
        for (col, def) in [
            ("umbrel_path", "TEXT NOT NULL DEFAULT ''"),
            ("umbrel_app", "TEXT NOT NULL DEFAULT ''"),
            ("stop_app", "INTEGER NOT NULL DEFAULT 0"),
        ] {
            let exists = conn
                .prepare("SELECT 1 FROM pragma_table_info('sources') WHERE name = ?1")?
                .exists([col])?;
            if !exists {
                conn.execute_batch(&format!("ALTER TABLE sources ADD COLUMN {col} {def}"))?;
            }
        }
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    fn c(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ─────────────────────────── kv ───────────────────────────

    pub fn get(&self, key: &str) -> Option<String> {
        self.c()
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set(&self, key: &str, value: &str) {
        let _ = self.c().execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }

    pub fn del(&self, key: &str) {
        let _ = self.c().execute("DELETE FROM kv WHERE key = ?1", [key]);
    }

    // ─────────────────────────── events ───────────────────────────

    pub fn event(&self, level: EventLevel, source_id: Option<i64>, message: &str) {
        let c = self.c();
        let _ = c.execute(
            "INSERT INTO events (ts, level, source_id, message) VALUES (?1, ?2, ?3, ?4)",
            params![now(), enum_to_str(&level), source_id, message],
        );
        // keep the log small
        let _ = c.execute(
            "DELETE FROM events WHERE id <= (SELECT MAX(id) - 5000 FROM events)",
            [],
        );
    }

    pub fn events(&self, limit: u32) -> Vec<Event> {
        let c = self.c();
        let mut st = c
            .prepare(
                "SELECT e.id, e.ts, e.level, e.source_id, s.name, e.message FROM events e
                 LEFT JOIN sources s ON s.id = e.source_id ORDER BY e.id DESC LIMIT ?1",
            )
            .unwrap();
        st.query_map([limit], |r| {
            Ok(Event {
                id: r.get(0)?,
                ts: r.get(1)?,
                level: str_to_enum(&r.get::<_, String>(2)?),
                source_id: r.get(3)?,
                source_name: r.get(4)?,
                message: r.get(5)?,
            })
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    // ─────────────────────────── sessions / tokens ───────────────────────────

    pub fn add_session(&self, hash: &str, expires: i64) {
        let c = self.c();
        let _ = c.execute("DELETE FROM sessions WHERE expires < ?1", [now()]);
        let _ = c.execute(
            "INSERT INTO sessions (token_hash, expires) VALUES (?1, ?2)",
            params![hash, expires],
        );
    }

    pub fn session_valid(&self, hash: &str) -> bool {
        self.c()
            .query_row(
                "SELECT 1 FROM sessions WHERE token_hash = ?1 AND expires > ?2",
                params![hash, now()],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    pub fn delete_session(&self, hash: &str) {
        let _ = self
            .c()
            .execute("DELETE FROM sessions WHERE token_hash = ?1", [hash]);
    }

    pub fn add_token(&self, name: &str, hash: &str, hint: &str) -> TokenInfo {
        let c = self.c();
        let ts = now();
        let _ = c.execute(
            "INSERT INTO tokens (name, token_hash, hint, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, hash, hint, ts],
        );
        TokenInfo {
            id: c.last_insert_rowid(),
            name: name.into(),
            hint: hint.into(),
            created_at: ts,
            last_used: None,
        }
    }

    pub fn tokens(&self) -> Vec<TokenInfo> {
        let c = self.c();
        let mut st = c
            .prepare("SELECT id, name, hint, created_at, last_used FROM tokens ORDER BY id")
            .unwrap();
        st.query_map([], |r| {
            Ok(TokenInfo {
                id: r.get(0)?,
                name: r.get(1)?,
                hint: r.get(2)?,
                created_at: r.get(3)?,
                last_used: r.get(4)?,
            })
        })
        .unwrap()
        .filter_map(Result::ok)
        .collect()
    }

    pub fn delete_token(&self, id: i64) {
        let _ = self.c().execute("DELETE FROM tokens WHERE id = ?1", [id]);
    }

    /// Access token → touches `last_used`.
    pub fn access_token_valid(&self, hash: &str) -> bool {
        self.c()
            .execute(
                "UPDATE tokens SET last_used = ?1 WHERE token_hash = ?2",
                params![now(), hash],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn source_by_token(&self, hash: &str) -> Option<Source> {
        let c = self.c();
        let id: Option<i64> = c
            .query_row(
                "SELECT id FROM sources WHERE token_hash = ?1",
                [hash],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten()
            .or_else(|| {
                c.query_row(
                    "SELECT source_id FROM tickets WHERE token_hash = ?1 AND expires > ?2",
                    params![hash, now()],
                    |r| r.get(0),
                )
                .optional()
                .ok()
                .flatten()
            });
        drop(c);
        self.source(id?)
    }

    pub fn add_ticket(&self, hash: &str, source_id: i64, expires: i64) {
        let c = self.c();
        let _ = c.execute("DELETE FROM tickets WHERE expires < ?1", [now()]);
        let _ = c.execute(
            "INSERT INTO tickets (token_hash, source_id, expires) VALUES (?1, ?2, ?3)",
            params![hash, source_id, expires],
        );
    }

    // ─────────────────────────── sources ───────────────────────────

    pub fn sources(&self) -> Vec<Source> {
        let c = self.c();
        let mut st = c
            .prepare(&format!(
                "SELECT {SOURCE_COLS} FROM sources ORDER BY name COLLATE NOCASE"
            ))
            .unwrap();
        st.query_map([], source_row)
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    pub fn source(&self, id: i64) -> Option<Source> {
        self.c()
            .query_row(
                &format!("SELECT {SOURCE_COLS} FROM sources WHERE id = ?1"),
                [id],
                source_row,
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Finds a source by id, slug or name (case-insensitive).
    pub fn find_source(&self, key: &str) -> Option<Source> {
        if let Ok(id) = key.trim().parse::<i64>()
            && let Some(s) = self.source(id)
        {
            return Some(s);
        }
        let k = key.trim().to_lowercase();
        self.sources()
            .into_iter()
            .find(|s| s.slug == k || s.name.to_lowercase() == k)
    }

    pub fn slug_taken(&self, slug: &str, except: i64) -> bool {
        self.c()
            .query_row(
                "SELECT 1 FROM sources WHERE slug = ?1 AND id != ?2",
                params![slug, except],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    pub fn insert_source(&self, s: &Source, token_hash: &str) -> rusqlite::Result<i64> {
        let c = self.c();
        c.execute(
            "INSERT INTO sources (slug, name, description, token_hash, token_hint, retention, expect_hours, pull_kind,
                pull_url, pull_token, pull_method, pull_tool, pull_args, schedule, next_pull, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                s.slug,
                s.name,
                s.description,
                token_hash,
                s.token_hint,
                serde_json::to_string(&s.retention).unwrap(),
                s.expect_hours,
                enum_to_str(&s.pull_kind),
                s.pull_url,
                s.pull_token,
                s.pull_method,
                s.pull_tool,
                s.pull_args,
                serde_json::to_string(&s.schedule).unwrap(),
                s.next_pull,
                s.created_at
            ],
        )?;
        let id = c.last_insert_rowid();
        Self::write_umbrel(&c, id, s)?;
        Ok(id)
    }

    fn write_umbrel(c: &Connection, id: i64, s: &Source) -> rusqlite::Result<()> {
        c.execute(
            "UPDATE sources SET umbrel_path = ?2, umbrel_app = ?3, stop_app = ?4 WHERE id = ?1",
            params![id, s.umbrel_path, s.umbrel_app, s.stop_app as i64],
        )?;
        Ok(())
    }

    pub fn update_source(&self, s: &Source) -> rusqlite::Result<()> {
        let c = self.c();
        c.execute(
            "UPDATE sources SET slug = ?2, name = ?3, description = ?4, retention = ?5, expect_hours = ?6, pull_kind = ?7,
                pull_url = ?8, pull_token = ?9, pull_method = ?10, pull_tool = ?11, pull_args = ?12, schedule = ?13,
                next_pull = ?14 WHERE id = ?1",
            params![
                s.id,
                s.slug,
                s.name,
                s.description,
                serde_json::to_string(&s.retention).unwrap(),
                s.expect_hours,
                enum_to_str(&s.pull_kind),
                s.pull_url,
                s.pull_token,
                s.pull_method,
                s.pull_tool,
                s.pull_args,
                serde_json::to_string(&s.schedule).unwrap(),
                s.next_pull
            ],
        )?;
        Self::write_umbrel(&c, s.id, s)
    }

    pub fn set_source_token(&self, id: i64, hash: &str, hint: &str) {
        let _ = self.c().execute(
            "UPDATE sources SET token_hash = ?2, token_hint = ?3 WHERE id = ?1",
            params![id, hash, hint],
        );
    }

    pub fn set_pull_result(
        &self,
        id: i64,
        last_pull: i64,
        next_pull: Option<i64>,
        error: Option<&str>,
    ) {
        let _ = self.c().execute(
            "UPDATE sources SET last_pull = ?2, next_pull = ?3, pull_error = ?4 WHERE id = ?1",
            params![id, last_pull, next_pull, error],
        );
    }

    pub fn set_next_pull(&self, id: i64, next_pull: Option<i64>) {
        let _ = self.c().execute(
            "UPDATE sources SET next_pull = ?2 WHERE id = ?1",
            params![id, next_pull],
        );
    }

    /// Deletes the source and its records; returns the spool files that are now orphaned.
    pub fn delete_source(&self, id: i64) -> Vec<String> {
        let c = self.c();
        let mut st = c
            .prepare("SELECT spool FROM backups WHERE source_id = ?1 AND spool IS NOT NULL")
            .unwrap();
        let files: Vec<String> = st
            .query_map([id], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        drop(st);
        let _ = c.execute("DELETE FROM sources WHERE id = ?1", [id]);
        files
    }

    // ─────────────────────────── backups ───────────────────────────

    pub fn insert_backup(&self, b: NewBackup) -> i64 {
        let c = self.c();
        let _ = c.execute(
            "INSERT INTO backups (source_id, filename, stored_name, size, sha256, created_at, status, origin, spool)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7, ?8)",
            params![
                b.source_id,
                b.filename,
                b.stored_name,
                b.size as i64,
                b.sha256,
                now(),
                enum_to_str(&b.origin),
                b.spool
            ],
        );
        c.last_insert_rowid()
    }

    pub fn backup(&self, id: i64) -> Option<Backup> {
        self.c()
            .query_row(
                &format!("SELECT {BACKUP_COLS} FROM backups b LEFT JOIN sources s ON s.id = b.source_id WHERE b.id = ?1"),
                [id],
                backup_row,
            )
            .optional()
            .ok()
            .flatten()
    }

    fn backups_where(&self, cond: &str, p: impl rusqlite::Params) -> Vec<Backup> {
        let c = self.c();
        let mut st = c
            .prepare(&format!(
                "SELECT {BACKUP_COLS} FROM backups b LEFT JOIN sources s ON s.id = b.source_id WHERE {cond}"
            ))
            .unwrap();
        st.query_map(p, backup_row)
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    pub fn backups_of(&self, source_id: i64, limit: u32) -> Vec<Backup> {
        self.backups_where(
            "b.source_id = ?1 ORDER BY b.created_at DESC, b.id DESC LIMIT ?2",
            params![source_id, limit],
        )
    }

    pub fn stored_of(&self, source_id: i64) -> Vec<Backup> {
        self.backups_where(
            "b.source_id = ?1 AND b.status = 'stored' ORDER BY b.created_at DESC",
            [source_id],
        )
    }

    pub fn recent(&self, limit: u32) -> Vec<Backup> {
        self.backups_where(
            "1 = 1 ORDER BY b.created_at DESC, b.id DESC LIMIT ?1",
            [limit],
        )
    }

    /// Next queued backup that is due.
    pub fn next_queued(&self) -> Option<Backup> {
        self.backups_where(
            "b.status = 'queued' AND b.next_try <= ?1 ORDER BY b.next_try, b.id LIMIT 1",
            [now()],
        )
        .into_iter()
        .next()
    }

    pub fn queued_all(&self) -> Vec<Backup> {
        self.backups_where("b.status IN ('queued', 'uploading') ORDER BY b.id", [])
    }

    /// After a restart nothing is uploading anymore.
    pub fn reset_uploading(&self) {
        let _ = self.c().execute(
            "UPDATE backups SET status = 'queued' WHERE status = 'uploading'",
            [],
        );
    }

    pub fn set_uploading(&self, id: i64) {
        let _ = self.c().execute(
            "UPDATE backups SET status = 'uploading', attempts = attempts + 1 WHERE id = ?1",
            [id],
        );
    }

    pub fn set_retry(&self, id: i64, error: &str, next_try: i64) {
        let _ = self.c().execute(
            "UPDATE backups SET status = 'queued', error = ?2, next_try = ?3 WHERE id = ?1",
            params![id, error, next_try],
        );
    }

    pub fn retry_now(&self, id: i64) -> bool {
        self.c()
            .execute(
                "UPDATE backups SET status = 'queued', next_try = 0 WHERE id = ?1 AND status IN ('queued', 'failed') AND spool IS NOT NULL",
                [id],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn set_failed(&self, id: i64, error: &str) {
        let _ = self.c().execute(
            "UPDATE backups SET status = 'failed', error = ?2, spool = NULL WHERE id = ?1",
            params![id, error],
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_stored(
        &self,
        id: i64,
        uuid: &str,
        sealed_key: &str,
        region: &str,
        bucket: &str,
        chunks: u64,
    ) {
        let _ = self.c().execute(
            "UPDATE backups SET status = 'stored', stored_at = ?2, error = NULL, spool = NULL, filen_uuid = ?3,
                filen_key = ?4, filen_region = ?5, filen_bucket = ?6, filen_chunks = ?7 WHERE id = ?1",
            params![id, now(), uuid, sealed_key, region, bucket, chunks as i64],
        );
    }

    pub fn set_deleted(&self, id: i64) {
        let _ = self.c().execute(
            "UPDATE backups SET status = 'deleted', deleted_at = ?2, kept_by = '', spool = NULL WHERE id = ?1",
            params![id, now()],
        );
    }

    pub fn set_kept_by(&self, id: i64, kept: &[&str]) {
        let _ = self.c().execute(
            "UPDATE backups SET kept_by = ?2 WHERE id = ?1",
            params![id, kept.join(",")],
        );
    }

    /// Drop old deleted/failed records (keeps the list readable).
    pub fn cleanup_records(&self) {
        let _ = self.c().execute(
            "DELETE FROM backups WHERE status IN ('deleted', 'failed') AND created_at < ?1",
            [now() - 400 * 86_400],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database of version 1.0.4 (without the umbrelOS columns) opens and gets them.
    #[test]
    fn migrates_old_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE sources (id INTEGER PRIMARY KEY, slug TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
                 description TEXT NOT NULL DEFAULT '', token_hash TEXT NOT NULL UNIQUE, token_hint TEXT NOT NULL,
                 retention TEXT NOT NULL, expect_hours INTEGER NOT NULL DEFAULT 0, pull_kind TEXT NOT NULL DEFAULT 'none',
                 pull_url TEXT NOT NULL DEFAULT '', pull_token TEXT NOT NULL DEFAULT '', pull_method TEXT NOT NULL DEFAULT 'GET',
                 pull_tool TEXT NOT NULL DEFAULT 'create_backup', pull_args TEXT NOT NULL DEFAULT '',
                 schedule TEXT NOT NULL DEFAULT '{}', last_pull INTEGER, next_pull INTEGER, pull_error TEXT,
                 created_at INTEGER NOT NULL);
                 INSERT INTO sources (slug, name, token_hash, token_hint, retention, created_at)
                 VALUES ('alt', 'Alt', 'h', 'nrzbk_…', '{}', 1);",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let s = db.sources();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].umbrel_path, "");
        assert!(!s[0].stop_app);
        drop(db);
        Db::open(&path).unwrap(); // second start: no error
    }
}
