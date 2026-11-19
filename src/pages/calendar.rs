//! Calendar: every backup of a month on its day (stored, waiting, failed), planned backups
//! of the coming days, and the list of the chosen day.

use dioxus::prelude::*;
use nrzbackup_shared::{BackupItem, BackupStatus, SourceSummary, format_bytes};
use wasm_bindgen::JsValue;

use super::source_detail::BackupTable;
use crate::api;
use crate::components::common::*;

const MONTHS: [&str; 12] = [
    "Januar", "Februar", "März", "April", "Mai", "Juni", "Juli", "August", "September", "Oktober",
    "November", "Dezember",
];
const WEEKDAYS: [&str; 7] = ["Mo", "Di", "Mi", "Do", "Fr", "Sa", "So"];

/// Local midnight of `y-m-d` (month 0-based, overflow allowed) as Unix seconds.
fn local_ts(y: i32, m: i32, d: i32) -> i64 {
    let date = js_sys::Date::new_with_year_month_day(y as u32, m, d);
    (date.get_time() / 1000.0) as i64
}

/// `(year, month 0-based, day)` in local time.
fn ymd(ts: i64) -> (i32, i32, i32) {
    let d = js_sys::Date::new(&JsValue::from_f64(ts as f64 * 1000.0));
    (d.get_full_year() as i32, d.get_month() as i32, d.get_date() as i32)
}

#[derive(Clone, PartialEq)]
struct Day {
    ts: i64,
    day: i32,
    in_month: bool,
    today: bool,
    stored: u32,
    waiting: u32,
    failed: u32,
    bytes: u64,
    planned: Vec<String>,
}

#[component]
pub fn Calendar() -> Element {
    let (ty, tm, _) = ymd(now());
    let mut month = use_signal(|| (ty, tm));
    let mut selected = use_signal(|| None::<i64>);
    let mut source_filter = use_signal(|| None::<i64>);

    // grid: Monday before the 1st up to 6 weeks
    let (y, m) = month();
    let first = local_ts(y, m, 1);
    let lead = (js_sys::Date::new(&JsValue::from_f64(first as f64 * 1000.0)).get_day() as i32 + 6) % 7;

    let mut items = use_signal(Vec::<BackupItem>::new);
    let mut error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| true);
    let _ = use_resource(move || {
        let (y, m) = month();
        let first = local_ts(y, m, 1);
        let lead = (js_sys::Date::new(&JsValue::from_f64(first as f64 * 1000.0)).get_day() as i32 + 6) % 7;
        let (from, to) = (local_ts(y, m, 1 - lead), local_ts(y, m, 1 - lead + 42));
        async move {
            loading.set(true);
            match api::calendar(from, to).await {
                Ok(v) => {
                    items.set(v);
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
            loading.set(false);
        }
    });
    let sources = use_loader(api::sources);
    let source_list: Vec<SourceSummary> = sources.data.read().clone().unwrap_or_default();

    let filter = source_filter();
    let all: Vec<BackupItem> = items
        .read()
        .iter()
        .filter(|b| filter.is_none_or(|f| b.source_id == f))
        .cloned()
        .collect();

    let today = local_ts(ty, tm, ymd(now()).2);
    let days: Vec<Day> = (0..42)
        .map(|i| {
            let ts = local_ts(y, m, 1 - lead + i);
            let next = local_ts(y, m, 2 - lead + i);
            let (_, dm, dd) = ymd(ts);
            let mut d = Day {
                ts,
                day: dd,
                in_month: dm == m,
                today: ts == today,
                stored: 0,
                waiting: 0,
                failed: 0,
                bytes: 0,
                planned: vec![],
            };
            for b in all.iter().filter(|b| b.created_at >= ts && b.created_at < next) {
                match b.status {
                    BackupStatus::Failed => d.failed += 1,
                    BackupStatus::Queued | BackupStatus::Uploading => d.waiting += 1,
                    // deleted = was a good backup, thinned out later by the retention
                    _ => d.stored += 1,
                }
                if b.status != BackupStatus::Failed {
                    d.bytes += b.size;
                }
            }
            for s in source_list.iter().filter(|s| filter.is_none_or(|f| s.id == f)) {
                if let Some(n) = s.pull.next_pull
                    && n >= ts
                    && n < next
                {
                    d.planned.push(s.name.clone());
                }
            }
            d
        })
        .collect();

    let month_items: Vec<&BackupItem> = all
        .iter()
        .filter(|b| b.created_at >= first && b.created_at < local_ts(y, m + 1, 1))
        .collect();
    let month_ok = month_items.iter().filter(|b| b.status != BackupStatus::Failed).count();
    let month_failed = month_items.len() - month_ok;
    let month_bytes: u64 = month_items
        .iter()
        .filter(|b| b.status != BackupStatus::Failed)
        .map(|b| b.size)
        .sum();
    let days_with = days.iter().filter(|d| d.in_month && d.stored + d.waiting > 0).count();
    let max_count = days.iter().map(|d| d.stored + d.waiting).max().unwrap_or(0).max(1);

    let sel = selected();
    let day_items: Vec<BackupItem> = match sel {
        Some(ts) => {
            let (sy, sm, sd) = ymd(ts);
            let end = local_ts(sy, sm, sd + 1);
            let mut v: Vec<BackupItem> = all
                .iter()
                .filter(|b| b.created_at >= ts && b.created_at < end)
                .cloned()
                .collect();
            v.reverse();
            v
        }
        None => vec![],
    };
    let sel_day = sel.and_then(|ts| days.iter().find(|d| d.ts == ts).cloned());

    rsx! {
        PageHeader { title: "Kalender" }
        div { class: "grid gap-6",
            div { class: "grid grid-cols-2 gap-3 lg:grid-cols-4",
                Stat { label: "Backups im {MONTHS[m as usize]}", value: rsx! { "{month_ok}" }, hint: format!("an {days_with} Tagen") }
                Stat { label: "Gesichert im Monat", value: rsx! { {format_bytes(month_bytes)} } }
                Stat {
                    label: "Fehlgeschlagen",
                    value: rsx! { span { class: if month_failed > 0 { "text-destructive" } else { "" }, "{month_failed}" } },
                }
                Stat {
                    label: "Geplant",
                    value: rsx! { "{days.iter().map(|d| d.planned.len()).sum::<usize>()}" },
                    hint: "nächste Anforderungen",
                }
            }

            section { class: "rounded-xl border bg-card p-4 shadow-sm sm:p-5",
                div { class: "mb-4 flex flex-wrap items-center gap-2",
                    button {
                        class: "{btn(BTN_OUTLINE)} {BTN_ICON}",
                        title: "Vorheriger Monat",
                        onclick: move |_| {
                            let (y, m) = month();
                            month.set(if m == 0 { (y - 1, 11) } else { (y, m - 1) });
                            selected.set(None);
                        },
                        icons::ChevronLeft {}
                    }
                    h2 { class: "min-w-40 text-center text-lg font-semibold tabular-nums", "{MONTHS[m as usize]} {y}" }
                    button {
                        class: "{btn(BTN_OUTLINE)} {BTN_ICON}",
                        title: "Nächster Monat",
                        onclick: move |_| {
                            let (y, m) = month();
                            month.set(if m == 11 { (y + 1, 0) } else { (y, m + 1) });
                            selected.set(None);
                        },
                        icons::ChevronRight {}
                    }
                    button {
                        class: "{btn(BTN_GHOST)} {BTN_SM}",
                        onclick: move |_| {
                            month.set((ty, tm));
                            selected.set(Some(today));
                        },
                        "Heute"
                    }
                    if loading() {
                        icons::LoaderCircle { class: "size-4 animate-spin text-muted-foreground" }
                    }
                    div { class: "flex-1" }
                    select {
                        class: "{SELECT} w-auto max-w-56",
                        onchange: move |e| source_filter.set(e.value().parse().ok()),
                        option { value: "", "Alle Quellen" }
                        for s in source_list.iter() {
                            option { key: "{s.id}", value: "{s.id}", selected: filter == Some(s.id), "{s.name}" }
                        }
                    }
                }
                if let Some(e) = error() {
                    div { class: "mb-3", Callout { text: e, error: true } }
                }
                div { class: "grid grid-cols-7 gap-1 sm:gap-1.5",
                    for w in WEEKDAYS {
                        div { class: "pb-1 text-center text-xs font-medium text-muted-foreground", "{w}" }
                    }
                    for d in days.iter().cloned() {
                        DayCell {
                            key: "{d.ts}",
                            day: d.clone(),
                            max: max_count,
                            selected: sel == Some(d.ts),
                            onclick: move |_| selected.set(if sel == Some(d.ts) { None } else { Some(d.ts) }),
                        }
                    }
                }
                div { class: "mt-4 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground",
                    span { class: "flex items-center gap-1.5", span { class: "size-2.5 rounded-full bg-success" } "gesichert" }
                    span { class: "flex items-center gap-1.5", span { class: "size-2.5 rounded-full bg-warning" } "wartet auf Upload" }
                    span { class: "flex items-center gap-1.5", span { class: "size-2.5 rounded-full bg-destructive" } "fehlgeschlagen" }
                    span { class: "flex items-center gap-1.5", span { class: "size-2.5 rounded-full border border-primary" } "geplant" }
                }
            }

            if let Some(d) = sel_day {
                Section {
                    title: format!("{:02}.{:02}. – {} Backup(s)", d.day, ymd(d.ts).1 + 1, day_items.len()),
                    description: if d.bytes > 0 { Some(format!("{} gesichert", format_bytes(d.bytes))) } else { None },
                    flush: true,
                    if !d.planned.is_empty() {
                        div { class: "flex flex-wrap items-center gap-2 border-b px-5 py-3 text-sm",
                            icons::CalendarClock { class: "size-4 text-muted-foreground" }
                            span { class: "text-muted-foreground", "Geplant:" }
                            for n in d.planned.iter() {
                                Tag { text: n.clone(), tone: Tone::Neutral }
                            }
                        }
                    }
                    if day_items.is_empty() {
                        p { class: "px-5 py-6 text-center text-sm text-muted-foreground", "An diesem Tag gab es kein Backup." }
                    } else {
                        BackupTable { backups: day_items.clone(), show_source: true, actions: false, on_change: move |_| {} }
                    }
                }
            }
        }
    }
}

#[component]
fn DayCell(day: Day, max: u32, selected: bool, onclick: EventHandler<MouseEvent>) -> Element {
    let count = day.stored + day.waiting;
    // background intensity like a heatmap
    let heat = if count == 0 {
        ""
    } else {
        match (count * 4).div_ceil(max) {
            1 => "bg-success/10",
            2 => "bg-success/20",
            3 => "bg-success/30",
            _ => "bg-success/40",
        }
    };
    let ring = if selected {
        "ring-2 ring-primary"
    } else if day.today {
        "ring-1 ring-primary/60"
    } else {
        ""
    };
    let muted = if day.in_month { "" } else { "opacity-40" };
    let failed_bg = if day.failed > 0 && count == 0 { "bg-destructive/15" } else { "" };
    rsx! {
        button {
            class: "flex aspect-square min-h-12 cursor-pointer flex-col rounded-lg border p-1 text-left transition-colors hover:border-primary/60 sm:aspect-[4/3] sm:p-2 {heat} {failed_bg} {ring} {muted}",
            onclick: move |e| onclick.call(e),
            div { class: "flex items-center justify-between",
                span { class: if day.today { "text-xs font-semibold text-primary sm:text-sm" } else { "text-xs font-medium sm:text-sm" }, "{day.day}" }
                if !day.planned.is_empty() {
                    span { class: "size-2 rounded-full border border-primary", title: "Geplant: {day.planned.join(\", \")}" }
                }
            }
            div { class: "flex-1" }
            div { class: "flex flex-wrap items-center gap-1",
                if day.stored > 0 {
                    span { class: "flex items-center gap-0.5 text-[10px] font-medium tabular-nums text-success sm:text-xs",
                        span { class: "size-1.5 rounded-full bg-success sm:size-2" }
                        "{day.stored}"
                    }
                }
                if day.waiting > 0 {
                    span { class: "flex items-center gap-0.5 text-[10px] font-medium tabular-nums text-warning sm:text-xs",
                        span { class: "size-1.5 rounded-full bg-warning sm:size-2" }
                        "{day.waiting}"
                    }
                }
                if day.failed > 0 {
                    span { class: "flex items-center gap-0.5 text-[10px] font-medium tabular-nums text-destructive sm:text-xs",
                        span { class: "size-1.5 rounded-full bg-destructive sm:size-2" }
                        "{day.failed}"
                    }
                }
            }
            if day.bytes > 0 {
                span { class: "hidden truncate text-[11px] text-muted-foreground tabular-nums sm:block", {format_bytes(day.bytes)} }
            }
        }
    }
}
