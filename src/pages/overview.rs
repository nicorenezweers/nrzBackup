//! Dashboard: traffic light, Filen storage, key figures, sources and recent backups.

use dioxus::prelude::*;
use nrzbackup_shared::{CalcInput, Health, Stats, calculate, format_bytes};

use super::source_detail::BackupTable;
use super::sources::SourceList;
use crate::Route;
use crate::api;
use crate::components::common::*;
use crate::components::masterkey::MasterKeyDialog;
use crate::components::overlay::*;

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

            StatsPanel {}

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

/// Reminder until the master key is confirmed as stored safely. The whole banner is a
/// button that opens the key dialog right here.
#[component]
fn MasterKeyBanner() -> Element {
    let mut settings = use_loader(api::settings);
    let mut open = use_signal(|| false);
    let need = settings
        .data
        .read()
        .as_ref()
        .is_some_and(|s| s.master_key.exists && !s.master_key.confirmed);
    rsx! {
        MasterKeyDialog { open, confirmed: false, on_change: move |_| settings.reload() }
        if need {
            button {
                class: "mb-4 flex w-full cursor-pointer items-center gap-3 rounded-lg border border-warning/40 bg-warning/10 px-4 py-3 text-left text-sm transition-colors hover:bg-warning/15 focus-visible:outline-none focus-visible:ring-[3px] focus-visible:ring-warning/40",
                onclick: move |_| open.set(true),
                icons::TriangleAlert { class: "size-5 shrink-0 text-warning" }
                div { class: "min-w-0 flex-1",
                    p { class: "font-medium", "Dein Hauptschlüssel ist noch nicht gesichert" }
                    p { class: "text-muted-foreground", "Ohne ihn sind die Backups bei einem Defekt dieses Geräts verloren. Tippe hier, um ihn anzusehen, auszudrucken oder in Google / iCloud zu sichern." }
                }
                span { class: "{btn(BTN_OUTLINE)} {BTN_SM} pointer-events-none hidden sm:inline-flex",
                    icons::KeyRound {}
                    "Jetzt sichern"
                }
                icons::ChevronRight { class: "size-4 shrink-0 text-muted-foreground sm:hidden" }
            }
        }
    }
}

/// Storage, app coverage and "back up everything now".
#[component]
fn StatsPanel() -> Element {
    let mut stats = use_loader(api::stats);
    use_poll(stats, 60);
    let mut busy = use_signal(|| false);
    let mut show_apps = use_signal(|| false);
    let all = move |_| {
        spawn(async move {
            busy.set(true);
            match api::backup_all().await {
                Ok(r) if r.ok => toast(r.message),
                Ok(r) => toast_error(r.message),
                Err(e) => toast_error(e),
            }
            busy.set(false);
            stats.reload();
        });
    };
    let Some(st) = stats.data.read().clone() else {
        return rsx! {
            if let Some(e) = stats.error.read().clone() {
                Callout { text: format!("Statistik nicht verfügbar: {e}"), warning: true }
            }
        };
    };
    let c = st.coverage.clone();
    let pct = c.percent.clamp(0.0, 100.0);
    let bar = if pct >= 99.5 { "bg-success" } else if pct >= 50.0 { "bg-warning" } else { "bg-destructive" };
    rsx! {
        div { class: "grid gap-4 lg:grid-cols-2",
            // ── Apps gesichert
            div { class: "grid content-start gap-3 rounded-xl border bg-card p-5 shadow-sm",
                div { class: "flex items-center gap-2",
                    icons::LayoutGrid { class: "size-4 text-muted-foreground" }
                    p { class: "flex-1 font-medium", "Apps gesichert" }
                    if c.available {
                        span { class: "text-2xl font-semibold tabular-nums", "{pct:.0} %" }
                    }
                }
                if c.available {
                    div { class: "h-2.5 overflow-hidden rounded-full bg-muted",
                        div { class: "h-full rounded-full {bar}", style: "width: {pct:.1}%" }
                    }
                    p { class: "text-sm text-muted-foreground",
                        "{c.backed_up} von {c.total} umbrelOS-Apps haben ein Backup"
                        if c.configured > c.backed_up { " · {c.configured - c.backed_up} eingerichtet, warten noch" }
                    }
                    if !c.apps.is_empty() {
                        button { class: "w-fit text-sm text-muted-foreground underline-offset-4 hover:text-foreground hover:underline", onclick: move |_| show_apps.toggle(),
                            if show_apps() { "Liste ausblenden" } else { "Alle Apps zeigen" }
                        }
                    }
                    if show_apps() {
                        div { class: "grid max-h-72 gap-1 overflow-y-auto pr-1",
                            for a in c.apps.iter().cloned() {
                                div { key: "{a.id}", class: "flex items-center gap-2 rounded-md px-2 py-1 text-sm hover:bg-muted/50",
                                    if a.backed_up {
                                        icons::CircleCheck { class: "size-4 shrink-0 text-success" }
                                    } else if a.source_id.is_some() {
                                        icons::Clock { class: "size-4 shrink-0 text-warning" }
                                    } else {
                                        icons::CircleDashed { class: "size-4 shrink-0 text-muted-foreground" }
                                    }
                                    span { class: "min-w-0 flex-1 truncate", "{a.name}" }
                                    span { class: "shrink-0 text-xs text-muted-foreground",
                                        if let Some(ts) = a.last_backup { {fmt_relative(ts)} } else if a.source_id.is_some() { "noch kein Backup" } else { "nicht gesichert" }
                                    }
                                }
                            }
                        }
                    }
                    if c.total > c.configured {
                        Link { to: crate::Route::Sources {}, class: "{btn(BTN_OUTLINE)} {BTN_SM} w-fit",
                            icons::Plus {}
                            "Fehlende Apps sichern"
                        }
                    }
                } else {
                    p { class: "text-sm text-muted-foreground",
                        {c.error.clone().unwrap_or_else(|| "umbrelOS ist nicht verbunden.".into())}
                    }
                }
            }
            // ── Speicher
            div { class: "grid content-start gap-3 rounded-xl border bg-card p-5 shadow-sm",
                div { class: "flex items-center gap-2",
                    icons::HardDrive { class: "size-4 text-muted-foreground" }
                    p { class: "flex-1 font-medium", "Speicher" }
                    button { class: "{btn(BTN_PRIMARY)} {BTN_SM}", disabled: busy(), onclick: all,
                        if busy() { icons::LoaderCircle { class: "animate-spin" } } else { icons::Play {} }
                        "Alles jetzt sichern"
                    }
                }
                div { class: "grid grid-cols-3 gap-3",
                    div {
                        p { class: "text-xs text-muted-foreground", "Gesichert" }
                        p { class: "text-lg font-semibold tabular-nums", {format_bytes(st.stored_bytes)} }
                        p { class: "text-xs text-muted-foreground", "{st.copies} Kopien" }
                    }
                    div {
                        p { class: "text-xs text-muted-foreground", "Filen frei" }
                        p { class: "text-lg font-semibold tabular-nums", if st.filen_connected { {format_bytes(st.storage_free)} } else { "–" } }
                        p { class: "text-xs text-muted-foreground", "von " {format_bytes(st.storage_max)} }
                    }
                    div {
                        p { class: "text-xs text-muted-foreground", "Filen belegt" }
                        p { class: "text-lg font-semibold tabular-nums", {format_bytes(st.storage_used)} }
                        p { class: "text-xs text-muted-foreground", "davon Backups " {format_bytes(st.stored_bytes)} }
                    }
                }
                if st.filen_connected {
                    UsageBar { used: st.storage_used, max: st.storage_max }
                }
            }
        }
        BackupCalculator { stats: st.clone() }
    }
}

/// What-if calculator: size, copies, growth → space needed and when Filen is full.
#[component]
fn BackupCalculator(stats: Stats) -> Element {
    let base = stats.forecast_input;
    let mut size_gb = use_signal(move || format!("{:.2}", base.backup_size as f64 / 1e9));
    let mut copies = use_signal(move || base.copies.to_string());
    let mut growth = use_signal(move || format!("{}", base.growth_percent_month));
    let mut max_gb = use_signal(move || format!("{:.0}", base.storage_max as f64 / 1e9));
    let num = |s: &str| s.trim().replace(',', ".").parse::<f64>().ok();
    let input = CalcInput {
        backup_size: (num(&size_gb()).unwrap_or(0.0).max(0.0) * 1e9) as u64,
        copies: num(&copies()).unwrap_or(1.0).max(1.0) as u32,
        growth_percent_month: num(&growth()).unwrap_or(0.0),
        storage_max: (num(&max_gb()).unwrap_or(0.0).max(0.0) * 1e9) as u64,
        ..base
    };
    let r = calculate(&input);
    let reset = move |_| {
        size_gb.set(format!("{:.2}", base.backup_size as f64 / 1e9));
        copies.set(base.copies.to_string());
        growth.set(format!("{}", base.growth_percent_month));
        max_gb.set(format!("{:.0}", base.storage_max as f64 / 1e9));
    };
    let full = match r.months_until_full {
        _ if input.storage_max == 0 => "Filen-Speichergröße unbekannt".to_string(),
        Some(m) if m <= 0.0 => "passt jetzt schon nicht".into(),
        Some(m) if m < 24.0 => format!("in ca. {m:.0} Monaten"),
        Some(m) => format!("in ca. {:.1} Jahren", m / 12.0),
        None => "nie (ohne Wachstum)".into(),
    };
    let signed = |v: i64| if v < 0 { format!("−{}", format_bytes(v.unsigned_abs())) } else { format_bytes(v as u64) };
    rsx! {
        Section {
            title: "Backup-Rechner",
            description: "Wie viel Platz brauchen deine Backups, wenn die Aufbewahrung voll ist? Vorbelegt mit der aktuellen Größe, deiner Aufbewahrung und dem gemessenen Wachstum – zum Durchspielen einfach ändern.",
            actions: rsx! {
                button { class: "{btn(BTN_GHOST)} {BTN_SM}", onclick: reset, icons::RotateCcw {} "Aktuelle Werte" }
            },
            div { class: "grid gap-5 lg:grid-cols-[1fr_1.2fr]",
                div { class: "grid grid-cols-2 gap-3",
                    Field { label: "Größe pro Backup (GB)", hint: "alle Quellen zusammen",
                        input { class: "{INPUT}", inputmode: "decimal", value: "{size_gb}", oninput: move |e| size_gb.set(e.value()) }
                    }
                    Field { label: "Aufbewahrte Kopien", hint: "aus der Aufbewahrung",
                        input { class: "{INPUT}", inputmode: "numeric", value: "{copies}", oninput: move |e| copies.set(e.value()) }
                    }
                    Field { label: "Wachstum % / Monat", hint: "gemessen: {base.growth_percent_month} %",
                        input { class: "{INPUT}", inputmode: "decimal", value: "{growth}", oninput: move |e| growth.set(e.value()) }
                    }
                    Field { label: "Filen-Speicher (GB)", hint: "dein Tarif",
                        input { class: "{INPUT}", inputmode: "decimal", value: "{max_gb}", oninput: move |e| max_gb.set(e.value()) }
                    }
                }
                div { class: "grid content-start gap-3",
                    div { class: "grid grid-cols-2 gap-3",
                        Stat { label: "Platzbedarf", value: rsx! { {format_bytes(r.projected_bytes)} }, hint: format!("jetzt {}", format_bytes(base.current_bytes)) }
                        Stat { label: "Kommt noch dazu", value: rsx! { {signed(r.extra_bytes)} } }
                        Stat { label: "In 12 Monaten", value: rsx! { {format_bytes(r.in_12_months)} } }
                        Stat {
                            label: "Danach frei in Filen",
                            value: rsx! { span { class: if r.fits { "" } else { "text-destructive" }, {signed(r.free_after)} } },
                        }
                    }
                    if input.storage_max > 0 {
                        Callout {
                            text: if r.fits { format!("Passt. Filen ist voll: {full}.") } else { format!("Passt nicht in deinen Filen-Speicher – es fehlen {}. Weniger Kopien aufbewahren oder einen größeren Tarif wählen.", format_bytes(r.free_after.unsigned_abs())) },
                            warning: !r.fits || r.months_until_full.is_some_and(|m| m < 6.0),
                        }
                    }
                    if !stats.sources.is_empty() {
                        div { class: "overflow-x-auto",
                            table { class: "w-full text-sm",
                                thead {
                                    tr { class: "text-left text-xs text-muted-foreground",
                                        th { class: "py-1 pr-3 font-medium", "Quelle" }
                                        th { class: "py-1 pr-3 text-right font-medium", "Backup" }
                                        th { class: "py-1 pr-3 text-right font-medium", "Kopien" }
                                        th { class: "py-1 text-right font-medium", "Bedarf" }
                                    }
                                }
                                tbody {
                                    for s in stats.sources.iter().cloned() {
                                        tr { key: "{s.source_id}", class: "border-t tabular-nums",
                                            td { class: "max-w-40 truncate py-1.5 pr-3", "{s.name}" }
                                            td { class: "py-1.5 pr-3 text-right", {format_bytes(s.latest_size)} }
                                            td { class: "py-1.5 pr-3 text-right", "{s.copies} → {s.steady_copies}" }
                                            td { class: "py-1.5 text-right", {format_bytes(s.projected_bytes)} }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
