//! Dashboard: traffic light, Filen storage, key figures, sources and recent backups.

use dioxus::prelude::*;
use nrzbackup_shared::{Health, format_bytes};

use super::source_detail::BackupTable;
use super::sources::SourceList;
use crate::Route;
use crate::api;
use crate::components::common::*;

#[component]
pub fn Overview() -> Element {
    let mut data = use_loader(api::overview);
    use_poll(data, 15);

    let error = data.error.read().clone();
    let Some(o) = data.data.read().clone() else {
        return rsx! {
            if let Some(e) = error {
                Callout { text: e, error: true }
            } else {
                Loading {}
            }
        };
    };
    let ring = match o.health {
        Health::Ok => "ring-success/30",
        Health::Warning => "ring-warning/30",
        Health::Error => "ring-destructive/30",
        Health::Unknown => "ring-muted-foreground/20",
    };
    let q = &o.queue;
    let queue_hint = if q.retrying > 0 {
        format!("{} mit Fehlversuch · {}", q.retrying, format_bytes(q.bytes))
    } else if q.uploading > 0 {
        format!("{} wird hochgeladen", q.uploading)
    } else if q.queued > 0 {
        format_bytes(q.bytes)
    } else {
        "nichts offen".into()
    };
    let f = o.filen.clone();

    rsx! {
        PageHeader { title: "Übersicht" }
        MasterKeyBanner {}
        if let Some(e) = error {
            div { class: "mb-4", Callout { text: format!("Aktualisieren fehlgeschlagen: {e}"), warning: true } }
        }
        div { class: "grid gap-6",
            div { class: "grid gap-4 lg:grid-cols-[1.4fr_1fr]",
                // ── Ampel
                div { class: "flex items-center gap-5 rounded-xl border bg-card p-6 shadow-sm",
                    div { class: "grid size-16 shrink-0 place-items-center rounded-full bg-muted ring-8 {ring}",
                        span { class: "size-7 rounded-full {health_bg(o.health)}" }
                    }
                    div { class: "min-w-0",
                        p { class: "text-2xl font-semibold tracking-tight {health_text(o.health)}", "{o.health.label()}" }
                        p { class: "mt-1 text-sm text-muted-foreground", "{o.message}" }
                    }
                }
                // ── Filen
                div { class: "grid gap-3 rounded-xl border bg-card p-5 shadow-sm",
                    div { class: "flex items-center gap-2",
                        if f.connected {
                            icons::Cloud { class: "size-4 text-muted-foreground" }
                        } else {
                            icons::CloudOff { class: "size-4 text-destructive" }
                        }
                        p { class: "flex-1 font-medium", "Filen.io" }
                        if f.connected {
                            Tag { text: "verbunden", tone: Tone::Success }
                        } else {
                            Tag { text: "nicht verbunden", tone: Tone::Danger }
                        }
                    }
                    if f.connected {
                        p { class: "truncate text-sm text-muted-foreground", "{f.email}" }
                        UsageBar { used: f.storage_used, max: f.storage_max }
                    }
                    if let Some(e) = f.error.clone() {
                        p { class: "break-words text-sm text-destructive", "{e}" }
                    }
                    if !f.connected || f.error.is_some() {
                        Link { to: Route::Settings {}, class: "{btn(BTN_OUTLINE)} {BTN_SM} w-fit",
                            icons::Settings {}
                            if f.connected { "Zu den Einstellungen" } else { "Filen-Konto verbinden" }
                        }
                    }
                }
            }

            // ── Kennzahlen
            div { class: "grid grid-cols-2 gap-3 lg:grid-cols-4",
                Stat { label: "Kopien in Filen", value: rsx! { "{o.copies}" }, hint: format!("{} Quellen", o.sources.len()) }
                Stat { label: "Speicher in Filen", value: rsx! { {format_bytes(o.stored_bytes)} } }
                Stat {
                    label: "Letzte Sicherung",
                    value: rsx! { TimeAgo { ts: o.last_backup, empty: "noch nie" } },
                    hint: o.last_backup.map(fmt_time).unwrap_or_default(),
                }
                Stat {
                    label: "Warteschlange",
                    value: rsx! {
                        span { class: if q.retrying > 0 { "text-warning" } else { "" }, "{q.queued + q.uploading}" }
                    },
                    hint: queue_hint,
                }
            }

            // ── Quellen
            div { class: "grid gap-3",
                div { class: "flex items-center justify-between",
                    h2 { class: "font-semibold", "Quellen" }
                    Link { to: Route::Sources {}, class: "text-sm text-muted-foreground hover:text-foreground", "Alle verwalten" }
                }
                if o.sources.is_empty() {
                    EmptyState {
                        title: "Noch keine Quellen",
                        description: "Lege eine Quelle an, damit Apps ihre Backups senden können.",
                        icon: rsx! { icons::DatabaseBackup {} },
                        Link { to: Route::Sources {}, class: "{btn(BTN_PRIMARY)}",
                            icons::Plus {}
                            "Quelle hinzufügen"
                        }
                    }
                } else {
                    SourceList { sources: o.sources.clone(), on_change: move |_| data.reload() }
                }
            }

            // ── Letzte Backups
            if !o.recent.is_empty() {
                Section { title: "Letzte Backups", flush: true,
                    BackupTable { backups: o.recent.clone(), show_source: true, actions: false, on_change: move |_| data.reload() }
                }
            }
        }
    }
}

/// Reminder until the master key is confirmed as stored safely.
#[component]
fn MasterKeyBanner() -> Element {
    let settings = use_loader(api::settings);
    let need = settings
        .data
        .read()
        .as_ref()
        .is_some_and(|s| s.master_key.exists && !s.master_key.confirmed);
    rsx! {
        if need {
            div { class: "mb-4",
                Callout { text: "Dein Hauptschlüssel ist noch nicht gesichert. Ohne ihn sind die Backups bei einem Defekt dieses Geräts verloren.", warning: true,
                    div { class: "mt-2",
                        Link { to: crate::Route::Settings {}, class: "{btn(BTN_OUTLINE)} {BTN_SM}",
                            icons::KeyRound {}
                            "Jetzt sichern"
                        }
                    }
                }
            }
        }
    }
}
