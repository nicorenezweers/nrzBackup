//! Receiving files into the local spool, the upload queue to Filen, retention, the pull
//! scheduler and the quota refresh.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use chrono::{TimeZone, Timelike};
use futures_util::{Stream, StreamExt};
use nrzbackup_shared::*;
use sha2::{Digest, Sha256};

use crate::db::{self, NewBackup, Source, now};
use crate::{AppState, filen, retention};

// ─────────────────────────── Receiving ───────────────────────────

/// Keeps only a safe base name: no folders, no control characters, at most 150 characters.
pub fn clean_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let cleaned: String = base
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| {
            if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let mut out: String = cleaned.chars().take(150).collect();
    if out.is_empty() {
        out = "backup.bin".into();
    }
    out
}

/// Writes an incoming file to the spool, records it as queued and wakes the uploader.
/// Only after this returned Ok the sender may consider the backup delivered.
pub async fn ingest<S, E>(
    state: &Arc<AppState>,
    source: &Source,
    filename: &str,
    origin: BackupOrigin,
    mut body: S,
) -> Result<BackupAccepted, String>
where
    S: Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Display,
{
    let filename = clean_filename(filename);
    let id = uuid::Uuid::new_v4().to_string();
    let part = state.spool_dir.join(format!("{id}.part"));
    let done = state.spool_dir.join(format!("{id}.bin"));
    // master key: encrypted while receiving, nothing unencrypted ever lands on disk
    let Some(identity) = crate::masterkey::identity(state) else {
        return Err("Der Hauptschlüssel ist nicht lesbar – Backup abgelehnt (siehe Protokoll).".into());
    };
    let (writer, tx) = crate::masterkey::encrypting_writer(identity.to_public(), part.clone())
        .map_err(|e| format!("Spool-Datei kann nicht angelegt werden: {e}"))?;
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    let result: Result<(), String> = async {
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| format!("Übertragung abgebrochen: {e}"))?;
            hasher.update(&chunk);
            size += chunk.len() as u64;
            if tx.send(chunk).await.is_err() {
                break; // writer failed, its error follows below
            }
        }
        Ok(())
    }
    .await;
    drop(tx);
    let written = writer
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| format!("Speichern/Verschlüsseln fehlgeschlagen: {e}")));
    if let Err(e) = result.and(written) {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    if size == 0 {
        let _ = tokio::fs::remove_file(&part).await;
        return Err("Die Datei ist leer – leere Backups werden nicht angenommen.".into());
    }
    tokio::fs::rename(&part, &done)
        .await
        .map_err(|e| e.to_string())?;

    let sha256 = hex::encode(hasher.finalize());
    let local = state.tz.timestamp_opt(now(), 0).single().unwrap();
    let stored_name = format!(
        "{}_{}_{}{}",
        source.slug,
        local.format("%Y-%m-%d_%H%M%S"),
        filename,
        crate::masterkey::SUFFIX
    );
    let backup_id = state.db.insert_backup(NewBackup {
        source_id: source.id,
        filename: &filename,
        stored_name: &stored_name,
        size,
        sha256: &sha256,
        origin,
        spool: &done.to_string_lossy(),
    });
    state.db.event(
        EventLevel::Info,
        Some(source.id),
        &format!(
            "{filename} empfangen ({}), wird verschlüsselt hochgeladen",
            format_bytes(size)
        ),
    );
    state.wake.notify_one();
    let connected = state.session.read().await.is_some();
    Ok(BackupAccepted {
        id: backup_id,
        filename,
        size,
        sha256,
        status: BackupStatus::Queued,
        message: if connected {
            "Angenommen – wird verschlüsselt zu Filen hochgeladen.".into()
        } else {
            "Angenommen und lokal gesichert – Filen ist noch nicht verbunden, der Upload folgt danach.".into()
        },
    })
}

/// Removes leftovers of interrupted transfers and spool files nobody references.
pub fn cleanup_spool(state: &AppState) {
    let referenced: std::collections::HashSet<String> = state
        .db
        .queued_all()
        .into_iter()
        .filter_map(|b| b.spool)
        .collect();
    if let Ok(dir) = std::fs::read_dir(&state.spool_dir) {
        for e in dir.flatten() {
            let p = e.path();
            if !referenced.contains(p.to_string_lossy().as_ref()) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

// ─────────────────────────── Upload queue ───────────────────────────

const BACKOFF: [i64; 6] = [60, 300, 900, 1800, 3600, 3 * 3600];

pub async fn upload_loop(state: Arc<AppState>) {
    loop {
        let has_session = state.session.read().await.is_some();
        let next = if has_session {
            state.db.next_queued()
        } else {
            None
        };
        match next {
            Some(b) => upload_one(&state, b).await,
            None => {
                let _ = tokio::time::timeout(Duration::from_secs(30), state.wake.notified()).await;
            }
        }
    }
}

async fn upload_one(state: &Arc<AppState>, b: db::Backup) {
    let id = b.item.id;
    let Some(source) = state.db.source(b.item.source_id) else {
        state.db.set_failed(id, "Quelle wurde gelöscht");
        return;
    };
    let Some(spool) = b.spool.clone().map(PathBuf::from) else {
        state.db.set_failed(id, "Lokale Datei fehlt");
        return;
    };
    if !spool.exists() {
        state.db.set_failed(id, "Lokale Datei fehlt");
        state.db.event(
            EventLevel::Error,
            Some(source.id),
            &format!("{}: lokale Datei fehlt, Backup verloren", b.item.filename),
        );
        return;
    }
    state.db.set_uploading(id);
    let attempt = b.item.attempts + 1;

    match upload_to_filen(state, &source, &b, &spool).await {
        Ok(up) => {
            state.db.set_stored(
                id,
                &up.uuid,
                &state.secrets.seal(&up.key),
                &up.region,
                &up.bucket,
                up.chunks,
            );
            let _ = tokio::fs::remove_file(&spool).await;
            state.db.event(
                EventLevel::Info,
                Some(source.id),
                &format!(
                    "{} in Filen gesichert ({})",
                    b.item.filename,
                    format_bytes(up.size)
                ),
            );
            if let Err(e) = prune(state, source.id).await {
                state.db.event(
                    EventLevel::Warn,
                    Some(source.id),
                    &format!("Aufräumen alter Kopien: {e}"),
                );
            }
            refresh_quota(state).await;
        }
        Err(e) => {
            let wait = BACKOFF[(attempt as usize - 1).min(BACKOFF.len() - 1)];
            state.db.set_retry(id, &e, now() + wait);
            if attempt == 1 || attempt == 3 || attempt.is_multiple_of(10) {
                state.db.event(
                    EventLevel::Warn,
                    Some(source.id),
                    &format!("Upload von {} fehlgeschlagen (Versuch {attempt}), nächster Versuch in {} min: {e}", b.item.filename, wait / 60),
                );
            }
            if looks_like_auth_error(&e) {
                relogin(state).await;
            }
        }
    }
}

async fn upload_to_filen(
    state: &Arc<AppState>,
    source: &Source,
    b: &db::Backup,
    spool: &Path,
) -> Result<filen::Uploaded, String> {
    let session = state
        .filen_session()
        .await
        .ok_or("Filen ist nicht verbunden")?;
    let root = state.root_folder();
    let parent = match state
        .filen
        .ensure_path(&session, &[&root, &source.slug])
        .await
    {
        Ok(p) => p,
        Err(e) => {
            // The folder may have been deleted in Filen in the meantime.
            state.filen.forget_dirs();
            return Err(e);
        }
    };
    let mime = mime_guess::from_path(&b.item.filename)
        .first_or_octet_stream()
        .essence_str()
        .to_string();
    let result = state
        .filen
        .upload_file(
            &session,
            spool,
            &parent,
            &b.item.stored_name,
            &mime,
            b.item.created_at * 1000,
        )
        .await;
    if result.is_err() {
        state.filen.forget_dirs();
    }
    result
}

fn looks_like_auth_error(e: &str) -> bool {
    let l = e.to_lowercase();
    l.contains("api key")
        || l.contains("api_key")
        || l.contains("unauthorized")
        || l.contains("http 401")
}

/// Logs in again with the stored password (works when two-factor authentication is off).
pub async fn relogin(state: &Arc<AppState>) {
    let Some(pw) = state
        .db
        .get("filen_password")
        .and_then(|s| state.secrets.open(&s))
    else {
        return;
    };
    let Some(email) = state.filen_session().await.map(|s| s.email) else {
        return;
    };
    match state.filen.login(&email, &pw, "").await {
        Ok(s) => {
            store_session(state, &s).await;
            state
                .db
                .event(EventLevel::Info, None, "Bei Filen neu angemeldet");
        }
        Err(e) => {
            state.db.event(
                EventLevel::Error,
                None,
                &format!("Neue Anmeldung bei Filen fehlgeschlagen: {e}"),
            );
            let mut st = state.filen_status.write().await;
            st.error = Some(e);
            st.checked_at = Some(now());
        }
    }
}

pub async fn store_session(state: &Arc<AppState>, s: &filen::Session) {
    state.db.set(
        "filen_session",
        &state.secrets.seal(&serde_json::to_string(s).unwrap()),
    );
    *state.session.write().await = Some(s.clone());
    state.filen.forget_dirs();
    state.wake.notify_one();
}

// ─────────────────────────── Retention ───────────────────────────

/// Applies the retention of a source: marks what stays, deletes the rest in Filen.
/// Returns the number of deleted copies.
pub async fn prune(state: &Arc<AppState>, source_id: i64) -> Result<u32, String> {
    let source = state.db.source(source_id).ok_or("Quelle nicht gefunden")?;
    let stored = state.db.stored_of(source_id);
    let copies: Vec<retention::Copy> = stored
        .iter()
        .map(|b| retention::Copy {
            id: b.item.id,
            ts: b.item.created_at,
        })
        .collect();
    let keep = retention::plan(&copies, &source.retention, state.tz);
    if keep.is_empty() && !stored.is_empty() {
        return Err("Aufbewahrung würde alles löschen – abgebrochen".into());
    }
    let session = state.filen_session().await;
    let permanently = state.delete_permanently();
    let mut deleted = 0;
    let mut errors = vec![];
    for b in &stored {
        if let Some(kept) = keep.get(&b.item.id) {
            state.db.set_kept_by(b.item.id, kept);
            continue;
        }
        let Some(session) = &session else {
            return Err("Filen ist nicht verbunden".into());
        };
        let Some(uuid) = &b.filen_uuid else {
            state.db.set_deleted(b.item.id);
            continue;
        };
        match state.filen.delete_file(session, uuid, permanently).await {
            Ok(()) => {
                state.db.set_deleted(b.item.id);
                deleted += 1;
            }
            Err(e) => errors.push(format!("{}: {e}", b.item.stored_name)),
        }
    }
    if deleted > 0 {
        state.db.event(
            EventLevel::Info,
            Some(source_id),
            &format!(
                "{deleted} alte Kopie(n) {} (Aufbewahrung)",
                if permanently {
                    "gelöscht"
                } else {
                    "in den Filen-Papierkorb verschoben"
                }
            ),
        );
    }
    if errors.is_empty() {
        Ok(deleted)
    } else {
        Err(errors.join("; "))
    }
}

/// Deletes one copy by hand.
pub async fn delete_backup(state: &Arc<AppState>, b: &db::Backup) -> Result<(), String> {
    match b.item.status {
        BackupStatus::Stored => {
            if let Some(uuid) = &b.filen_uuid {
                let session = state
                    .filen_session()
                    .await
                    .ok_or("Filen ist nicht verbunden")?;
                state
                    .filen
                    .delete_file(&session, uuid, state.delete_permanently())
                    .await?;
            }
        }
        BackupStatus::Uploading => {
            return Err("Wird gerade hochgeladen – bitte gleich nochmal versuchen".into());
        }
        _ => {
            if let Some(spool) = &b.spool {
                let _ = tokio::fs::remove_file(spool).await;
            }
        }
    }
    state.db.set_deleted(b.item.id);
    state.db.event(
        EventLevel::Info,
        Some(b.item.source_id),
        &format!("{} von Hand gelöscht", b.item.stored_name),
    );
    Ok(())
}

// ─────────────────────────── Scheduler ───────────────────────────

/// Next run of a schedule after `from` (Unix seconds).
pub fn next_run(schedule: &Schedule, from: i64, tz: chrono_tz::Tz) -> Option<i64> {
    match schedule.mode {
        ScheduleMode::Off => None,
        ScheduleMode::Interval => Some(from + i64::from(schedule.every_hours.max(1)) * 3600),
        ScheduleMode::Daily => {
            let (h, m) = parse_hhmm(&schedule.time).unwrap_or((3, 0));
            let local = tz.timestamp_opt(from, 0).single()?;
            let mut day = local.date_naive();
            for _ in 0..3 {
                if let Some(t) = day.and_hms_opt(h, m, 0)
                    && let Some(at) = tz.from_local_datetime(&t).earliest()
                    && at.timestamp() > from
                {
                    return Some(at.timestamp());
                }
                day = day.succ_opt()?;
            }
            None
        }
    }
}

pub fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some((h, m))
}

pub async fn scheduler_loop(state: Arc<AppState>) {
    let mut ticks: u64 = 0;
    loop {
        let t = now();
        for s in state.db.sources() {
            if s.pull_kind == PullKind::None || s.schedule.mode == ScheduleMode::Off {
                continue;
            }
            match s.next_pull {
                None => state
                    .db
                    .set_next_pull(s.id, next_run(&s.schedule, t, state.tz)),
                Some(at) if at <= t => {
                    // set the next run first, so a slow pull is not started twice
                    state
                        .db
                        .set_next_pull(s.id, next_run(&s.schedule, t, state.tz));
                    let st = state.clone();
                    tokio::spawn(async move {
                        let _ = crate::pull::run(&st, s.id).await;
                    });
                }
                _ => {}
            }
        }
        ticks += 1;
        if ticks.is_multiple_of(2880) {
            state.db.cleanup_records();
        }
        // align roughly to full minutes so "03:00" runs at 03:00
        let secs = chrono::Utc::now().second() as u64;
        tokio::time::sleep(Duration::from_secs(30 - secs % 30 + 1)).await;
    }
}

// ─────────────────────────── Quota ───────────────────────────

pub async fn refresh_quota(state: &Arc<AppState>) {
    let Some(session) = state.filen_session().await else {
        return;
    };
    let result = state.filen.quota(&session).await;
    let mut st = state.filen_status.write().await;
    st.connected = true;
    st.email = session.email.clone();
    st.auth_version = session.auth_version;
    st.checked_at = Some(now());
    match result {
        Ok((used, max)) => {
            st.storage_used = used;
            st.storage_max = max;
            st.error = None;
        }
        Err(e) => {
            if looks_like_auth_error(&e) {
                drop(st);
                relogin(state).await;
                return;
            }
            st.error = Some(e);
        }
    }
    state
        .db
        .set("filen_status", &serde_json::to_string(&*st).unwrap());
}

pub async fn quota_loop(state: Arc<AppState>) {
    loop {
        refresh_quota(&state).await;
        tokio::time::sleep(Duration::from_secs(600)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames() {
        assert_eq!(clean_filename("../../etc/passwd"), "passwd");
        assert_eq!(clean_filename("C:\\x\\db.sqlite"), "db.sqlite");
        assert_eq!(clean_filename("  "), "backup.bin");
        assert_eq!(clean_filename("a:b?.tar.gz"), "a_b_.tar.gz");
    }

    #[test]
    fn schedule() {
        let tz = chrono_tz::Europe::Zurich;
        let from = tz
            .with_ymd_and_hms(2026, 11, 12, 4, 0, 0)
            .unwrap()
            .timestamp();
        let s = Schedule {
            mode: ScheduleMode::Daily,
            time: "03:30".into(),
            every_hours: 24,
        };
        let next = next_run(&s, from, tz).unwrap();
        assert_eq!(
            next,
            tz.with_ymd_and_hms(2026, 11, 13, 3, 30, 0)
                .unwrap()
                .timestamp()
        );
        let s = Schedule {
            mode: ScheduleMode::Interval,
            time: String::new(),
            every_hours: 6,
        };
        assert_eq!(next_run(&s, from, tz), Some(from + 6 * 3600));
        assert_eq!(parse_hhmm("24:00"), None);
    }
}
