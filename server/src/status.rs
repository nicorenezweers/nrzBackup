//! Traffic lights: per source and overall.

use std::sync::Arc;

use nrzbackup_shared::*;

use crate::AppState;
use crate::db::{Backup, Source, now};

pub fn ago(secs: i64) -> String {
    let s = secs.max(0);
    if s < 90 {
        "gerade eben".into()
    } else if s < 90 * 60 {
        format!("vor {} min", s / 60)
    } else if s < 36 * 3600 {
        format!("vor {} Std.", s / 3600)
    } else {
        format!("vor {} Tagen", s / 86_400)
    }
}

pub fn summary(state: &AppState, s: &Source) -> SourceSummary {
    let backups = state.db.backups_of(s.id, 500);
    summary_with(state, s, &backups)
}

pub fn summary_with(state: &AppState, s: &Source, backups: &[Backup]) -> SourceSummary {
    let t = now();
    let stored: Vec<&Backup> = backups
        .iter()
        .filter(|b| b.item.status == BackupStatus::Stored)
        .collect();
    let queued: Vec<&Backup> = backups
        .iter()
        .filter(|b| {
            matches!(
                b.item.status,
                BackupStatus::Queued | BackupStatus::Uploading
            )
        })
        .collect();
    // Received counts as backed up: it is safe in the spool and will reach Filen.
    let last_backup = backups
        .iter()
        .filter(|b| {
            matches!(
                b.item.status,
                BackupStatus::Stored | BackupStatus::Queued | BackupStatus::Uploading
            )
        })
        .map(|b| b.item.created_at)
        .max();
    let last_error = queued
        .iter()
        .filter_map(|b| b.item.error.clone())
        .next()
        .or_else(|| s.pull_error.clone());

    let mut health = Health::Ok;
    let mut msgs: Vec<String> = vec![];
    fn worse(h: Health, m: String, health: &mut Health, msgs: &mut Vec<String>) {
        if h > *health {
            *health = h;
        }
        msgs.push(m);
    }

    if s.expect_hours > 0 {
        let expect = i64::from(s.expect_hours) * 3600;
        match last_backup {
            None if t - s.created_at > expect => worse(
                Health::Error,
                format!("Seit dem Anlegen ({}) kein Backup", ago(t - s.created_at)),
                &mut health,
                &mut msgs,
            ),
            None => worse(
                Health::Warning,
                "Noch kein Backup".into(),
                &mut health,
                &mut msgs,
            ),
            Some(last) if t - last > 2 * expect => worse(
                Health::Error,
                format!(
                    "Letztes Backup {} – erwartet alle {} Std.",
                    ago(t - last),
                    s.expect_hours
                ),
                &mut health,
                &mut msgs,
            ),
            Some(last) if t - last > expect => worse(
                Health::Warning,
                format!("Überfällig: letztes Backup {}", ago(t - last)),
                &mut health,
                &mut msgs,
            ),
            _ => {}
        }
    } else if last_backup.is_none() {
        health = Health::Unknown;
        msgs.push("Noch kein Backup".into());
    }
    if let Some(b) = queued
        .iter()
        .find(|b| b.item.attempts > 0 && b.item.error.is_some())
    {
        worse(
            Health::Warning,
            format!(
                "Upload wird wiederholt: {}",
                b.item.error.clone().unwrap_or_default()
            ),
            &mut health,
            &mut msgs,
        );
    }
    if let Some(e) = &s.pull_error {
        worse(
            Health::Warning,
            format!("Anfordern fehlgeschlagen: {e}"),
            &mut health,
            &mut msgs,
        );
    }
    if msgs.is_empty() {
        msgs.push(match last_backup {
            Some(last) => format!("Gesichert {}", ago(t - last)),
            None => "Bereit".into(),
        });
    }

    SourceSummary {
        id: s.id,
        slug: s.slug.clone(),
        name: s.name.clone(),
        description: s.description.clone(),
        health,
        health_message: msgs.join(" · "),
        copies: stored.len() as u32,
        stored_bytes: stored.iter().map(|b| b.item.size).sum(),
        queued: queued.len() as u32,
        last_backup,
        last_error,
        retention: s.retention,
        expect_hours: s.expect_hours,
        pull: PullView {
            kind: s.pull_kind,
            url: s.pull_url.clone(),
            has_token: !s.pull_token.is_empty(),
            method: s.pull_method.clone(),
            tool: s.pull_tool.clone(),
            arguments: s.pull_args.clone(),
            schedule: s.schedule.clone(),
            last_pull: s.last_pull,
            next_pull: if s.pull_kind == PullKind::None {
                None
            } else {
                s.next_pull
            },
            last_error: s.pull_error.clone(),
            umbrel_path: s.umbrel_path.clone(),
            umbrel_app: s.umbrel_app.clone(),
            stop_app: s.stop_app,
        },
        token_hint: s.token_hint.clone(),
        filen_path: format!("/{}/{}", state.root_folder(), s.slug),
        created_at: s.created_at,
    }
}

pub async fn filen_status(state: &Arc<AppState>) -> FilenStatus {
    let mut st = state.filen_status.read().await.clone();
    let session = state.filen_session().await;
    st.connected = session.is_some();
    if let Some(s) = session {
        st.email = s.email;
        st.auth_version = s.auth_version;
    }
    st.root_folder = state.root_folder();
    st
}

pub async fn overview(state: &Arc<AppState>) -> Overview {
    let filen = filen_status(state).await;
    let sources: Vec<SourceSummary> = state
        .db
        .sources()
        .iter()
        .map(|s| summary(state, s))
        .collect();
    let queued = state.db.queued_all();
    let queue = QueueInfo {
        queued: queued
            .iter()
            .filter(|b| b.item.status == BackupStatus::Queued)
            .count() as u32,
        uploading: queued
            .iter()
            .filter(|b| b.item.status == BackupStatus::Uploading)
            .count() as u32,
        retrying: queued
            .iter()
            .filter(|b| b.item.attempts > 0 && b.item.error.is_some())
            .count() as u32,
        bytes: queued.iter().map(|b| b.item.size).sum(),
    };

    let (health, message) = if !filen.connected {
        (
            Health::Error,
            if queue.queued > 0 {
                format!(
                    "Filen ist nicht verbunden – {} Backup(s) warten lokal",
                    queue.queued
                )
            } else {
                "Filen ist nicht verbunden".to_string()
            },
        )
    } else if let Some(e) = &filen.error {
        (Health::Error, format!("Filen meldet einen Fehler: {e}"))
    } else if sources.is_empty() {
        (Health::Unknown, "Noch keine Quelle eingerichtet".into())
    } else {
        let bad: Vec<&SourceSummary> = sources
            .iter()
            .filter(|s| matches!(s.health, Health::Warning | Health::Error))
            .collect();
        let worst = sources
            .iter()
            .map(|s| s.health)
            .filter(|h| *h != Health::Unknown)
            .max()
            .unwrap_or(Health::Unknown);
        if bad.is_empty() {
            let ok = sources.iter().filter(|s| s.health == Health::Ok).count();
            if ok == 0 {
                (Health::Unknown, "Noch keine Backups eingegangen".into())
            } else {
                (
                    worst,
                    format!(
                        "Alles gesichert – {ok} von {} Quelle(n) aktuell",
                        sources.len()
                    ),
                )
            }
        } else {
            let names: Vec<&str> = bad.iter().map(|s| s.name.as_str()).collect();
            (
                worst,
                format!(
                    "{} von {} Quelle(n) brauchen Aufmerksamkeit: {}",
                    bad.len(),
                    sources.len(),
                    names.join(", ")
                ),
            )
        }
    };

    Overview {
        health,
        message,
        copies: sources.iter().map(|s| s.copies).sum(),
        stored_bytes: sources.iter().map(|s| s.stored_bytes).sum(),
        last_backup: sources.iter().filter_map(|s| s.last_backup).max(),
        recent: state.db.recent(20).into_iter().map(|b| b.item).collect(),
        sources,
        queue,
        filen,
        version: crate::VERSION.into(),
    }
}
