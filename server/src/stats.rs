//! Statistics for the dashboard and MCP: storage, how many umbrelOS apps are backed up,
//! the backup forecast and "back up everything now".

use std::sync::Arc;

use nrzbackup_shared::*;

use crate::AppState;
use crate::db::{Backup, Source, now};

const DAY: f64 = 86_400.0;

/// Days between two backups: measured over the last 30 days, else from the schedule.
fn interval_days(s: &Source, backups: &[Backup]) -> f64 {
    let since = now() - 30 * 86_400;
    let mut ts: Vec<i64> = backups
        .iter()
        .filter(|b| b.item.status != BackupStatus::Failed && b.item.created_at >= since)
        .map(|b| b.item.created_at)
        .collect();
    ts.sort_unstable();
    if ts.len() >= 3 {
        let span = (ts[ts.len() - 1] - ts[0]) as f64 / DAY;
        if span > 0.0 {
            return (span / (ts.len() - 1) as f64).max(1.0 / 24.0);
        }
    }
    match s.schedule.mode {
        ScheduleMode::Daily => 1.0,
        ScheduleMode::Interval => f64::from(s.schedule.every_hours.max(1)) / 24.0,
        ScheduleMode::Off if s.expect_hours > 0 => f64::from(s.expect_hours) / 24.0,
        ScheduleMode::Off => 1.0,
    }
}

/// Growth of the backup size in percent per month (newest vs. the oldest of the last 90 days).
fn growth(backups: &[Backup]) -> f64 {
    let since = now() - 90 * 86_400;
    let ok: Vec<&Backup> = backups
        .iter()
        .filter(|b| b.item.status != BackupStatus::Failed && b.item.created_at >= since && b.item.size > 0)
        .collect();
    let (Some(new), Some(old)) = (ok.first(), ok.last()) else {
        return 0.0;
    };
    let months = (new.item.created_at - old.item.created_at) as f64 / (30.44 * DAY);
    if months < 7.0 / 30.44 || old.item.size == 0 {
        return 0.0;
    }
    let ratio = new.item.size as f64 / old.item.size as f64;
    ((ratio.powf(1.0 / months) - 1.0) * 100.0).clamp(-50.0, 500.0)
}

pub fn source_forecast(s: &Source, backups: &[Backup]) -> SourceForecast {
    let stored: Vec<&Backup> = backups
        .iter()
        .filter(|b| b.item.status == BackupStatus::Stored)
        .collect();
    let latest_size = backups
        .iter()
        .find(|b| matches!(b.item.status, BackupStatus::Stored | BackupStatus::Queued | BackupStatus::Uploading))
        .map(|b| b.item.size)
        .unwrap_or(0);
    let interval = interval_days(s, backups);
    let steady = steady_copies(&s.retention, interval);
    SourceForecast {
        source_id: s.id,
        name: s.name.clone(),
        latest_size,
        stored_bytes: stored.iter().map(|b| b.item.size).sum(),
        copies: stored.len() as u32,
        interval_days: interval,
        steady_copies: steady,
        projected_bytes: latest_size.saturating_mul(u64::from(steady)),
        growth_percent_month: growth(backups),
    }
}

pub async fn coverage(state: &Arc<AppState>) -> AppCoverage {
    let apps = match crate::umbrel::apps(state).await {
        Ok(a) => a,
        Err(e) => return AppCoverage { error: Some(e), ..Default::default() },
    };
    let mut items: Vec<AppCoverageItem> = apps
        .iter()
        .map(|a| {
            let last = a.source_id.and_then(|id| {
                state
                    .db
                    .backups_of(id, 50)
                    .into_iter()
                    .filter(|b| matches!(b.item.status, BackupStatus::Stored | BackupStatus::Queued | BackupStatus::Uploading))
                    .map(|b| b.item.created_at)
                    .max()
            });
            AppCoverageItem {
                id: a.id.clone(),
                name: a.name.clone(),
                source_id: a.source_id,
                last_backup: last,
                backed_up: last.is_some(),
            }
        })
        .collect();
    items.sort_by(|a, b| a.backed_up.cmp(&b.backed_up).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    let total = items.len() as u32;
    let backed_up = items.iter().filter(|a| a.backed_up).count() as u32;
    AppCoverage {
        available: true,
        error: None,
        total,
        configured: items.iter().filter(|a| a.source_id.is_some()).count() as u32,
        backed_up,
        percent: if total > 0 { f64::from(backed_up) * 100.0 / f64::from(total) } else { 0.0 },
        apps: items,
    }
}

pub async fn stats(state: &Arc<AppState>) -> Stats {
    let filen = crate::status::filen_status(state).await;
    let sources: Vec<SourceForecast> = state
        .db
        .sources()
        .iter()
        .map(|s| source_forecast(s, &state.db.backups_of(s.id, 1000)))
        .collect();
    let stored_bytes: u64 = sources.iter().map(|s| s.stored_bytes).sum();
    let backup_size: u64 = sources.iter().map(|s| s.latest_size).sum();
    let projected: u64 = sources.iter().map(|s| s.projected_bytes).sum();
    // size-weighted growth and the copies that give the same projection
    let growth = if backup_size > 0 {
        sources.iter().map(|s| s.growth_percent_month * s.latest_size as f64).sum::<f64>() / backup_size as f64
    } else {
        0.0
    };
    let copies = if backup_size > 0 { ((projected as f64 / backup_size as f64).round() as u32).max(1) } else { 1 };
    let input = CalcInput {
        backup_size,
        copies,
        growth_percent_month: (growth * 10.0).round() / 10.0,
        storage_used: filen.storage_used,
        storage_max: filen.storage_max,
        current_bytes: stored_bytes,
    };
    Stats {
        stored_bytes,
        copies: sources.iter().map(|s| s.copies).sum(),
        filen_connected: filen.connected,
        storage_used: filen.storage_used,
        storage_max: filen.storage_max,
        storage_free: filen.storage_max.saturating_sub(filen.storage_used),
        coverage: coverage(state).await,
        forecast: calculate(&input),
        forecast_input: input,
        sources,
    }
}

/// Requests a backup from every source that can be asked (HTTP, MCP, umbrelOS).
pub fn backup_all(state: &Arc<AppState>) -> ActionResult {
    let sources = state.db.sources();
    let (mut started, mut skipped) = (vec![], 0);
    for s in &sources {
        if s.pull_kind == PullKind::None {
            skipped += 1;
            continue;
        }
        if crate::api::start_pull(state, s).ok {
            started.push(s.name.clone());
        }
    }
    if started.is_empty() {
        return ActionResult {
            ok: false,
            message: if sources.is_empty() {
                "Noch keine Quelle eingerichtet.".into()
            } else {
                "Keine Quelle konnte angefordert werden (kein Anfordern eingerichtet oder läuft schon).".into()
            },
        };
    }
    let mut message = format!("Backup angefordert bei {}: {}", started.len(), started.join(", "));
    if skipped > 0 {
        message.push_str(&format!(" – {skipped} Quelle(n) senden selbst"));
    }
    ActionResult { ok: true, message }
}
