//! Shared building blocks in the style of rust-ui (from nrzBudget): field with label,
//! page header, empty state, loading, callout, section, switch, plus the nrzBackup
//! pieces (traffic light, copy button, code block, stat tile, data loader).

use std::future::Future;

use dioxus::prelude::*;
use nrzbackup_shared::{BackupStatus, Health};
use tw_merge::tw_merge;

use crate::api::Res;
use crate::components::overlay::{toast, toast_error};

pub const INPUT: &str = "flex h-9 w-full min-w-0 rounded-md border border-input bg-transparent px-3 py-1 text-base shadow-xs outline-none transition-[color,box-shadow] placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-input/30";
pub const TEXTAREA: &str = "flex min-h-20 w-full min-w-0 rounded-md border border-input bg-transparent px-3 py-2 font-mono text-sm shadow-xs outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 dark:bg-input/30";
pub const SELECT: &str = "flex h-9 w-full items-center rounded-md border border-input bg-transparent px-3 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:opacity-50 dark:bg-input/30 [&>option]:bg-popover";
pub const BTN: &str = "inline-flex shrink-0 cursor-pointer items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-all outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4";
pub const BTN_PRIMARY: &str =
    "bg-primary text-primary-foreground shadow-xs hover:bg-primary/90 h-9 px-4 py-2";
pub const BTN_OUTLINE: &str = "border bg-background shadow-xs hover:bg-accent hover:text-accent-foreground dark:bg-input/30 dark:border-input dark:hover:bg-input/50 h-9 px-4 py-2";
pub const BTN_GHOST: &str =
    "hover:bg-accent hover:text-accent-foreground dark:hover:bg-accent/50 h-9 px-3";
pub const BTN_SM: &str = "h-8 px-3 text-[13px]";
pub const BTN_ICON: &str = "size-9 p-0";

/// Klassen für einen Knopf, z. B. `btn(BTN_PRIMARY)`.
pub fn btn(variant: &str) -> String {
    format!("{BTN} {variant}")
}

// ------------------------------------------------------------------ Daten laden

/// Data of a page plus the last error; `reload()` fetches again.
pub struct Loader<T: 'static> {
    pub data: Signal<Option<T>>,
    pub error: Signal<Option<String>>,
    tick: Signal<u32>,
}

impl<T> Clone for Loader<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Loader<T> {}

impl<T> Loader<T> {
    pub fn reload(&mut self) {
        let next = *self.tick.peek() + 1;
        self.tick.set(next);
    }
}

/// Loads data once and again on every `reload()`; old data stays visible meanwhile.
pub fn use_loader<T, F, Fut>(f: F) -> Loader<T>
where
    T: 'static,
    F: Fn() -> Fut + 'static,
    Fut: Future<Output = Res<T>> + 'static,
{
    let mut data = use_signal(|| None::<T>);
    let mut error = use_signal(|| None::<String>);
    let tick = use_signal(|| 0u32);
    use_resource(move || {
        let _ = tick();
        let fut = f();
        async move {
            match fut.await {
                Ok(v) => {
                    data.set(Some(v));
                    error.set(None);
                }
                Err(e) => error.set(Some(e)),
            }
        }
    });
    Loader { data, error, tick }
}

/// Reloads the loader every `secs` seconds while the page is shown.
pub fn use_poll<T: 'static>(mut loader: Loader<T>, secs: u64) {
    use_future(move || async move {
        loop {
            gloo_timers::future::sleep(std::time::Duration::from_secs(secs)).await;
            loader.reload();
        }
    });
}

// ------------------------------------------------------------------ Zeit

fn date(ts: i64) -> js_sys::Date {
    js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0))
}

pub fn now() -> i64 {
    (js_sys::Date::now() / 1000.0) as i64
}

/// `12.11.2026, 03:00` in the local time of the browser.
pub fn fmt_time(ts: i64) -> String {
    let d = date(ts);
    format!(
        "{:02}.{:02}.{}, {:02}:{:02}",
        d.get_date(),
        d.get_month() + 1,
        d.get_full_year(),
        d.get_hours(),
        d.get_minutes()
    )
}

/// `vor 3 Std.`, `in 20 Min.`, `gerade eben`.
pub fn fmt_relative(ts: i64) -> String {
    let diff = now() - ts;
    let future = diff < 0;
    let s = diff.unsigned_abs();
    let text = if s < 60 {
        return if future {
            "gleich".into()
        } else {
            "gerade eben".into()
        };
    } else if s < 3600 {
        format!("{} Min.", s / 60)
    } else if s < 86_400 {
        format!("{} Std.", s / 3600)
    } else if s < 86_400 * 60 {
        let d = s / 86_400;
        format!("{d} {}", if d == 1 { "Tag" } else { "Tagen" })
    } else {
        return fmt_time(ts);
    };
    if future {
        format!("in {text}")
    } else {
        format!("vor {text}")
    }
}

/// `vor 3 Std.` with the exact time as tooltip, or a dash.
#[component]
pub fn TimeAgo(ts: Option<i64>, #[props(into, optional)] empty: Option<String>) -> Element {
    match ts {
        Some(t) => rsx! {
            span { title: fmt_time(t), {fmt_relative(t)} }
        },
        None => rsx! {
            span { class: "text-muted-foreground", {empty.unwrap_or_else(|| "–".into())} }
        },
    }
}

// ------------------------------------------------------------------ Ampel

pub fn health_bg(h: Health) -> &'static str {
    match h {
        Health::Ok => "bg-success",
        Health::Warning => "bg-warning",
        Health::Error => "bg-destructive",
        Health::Unknown => "bg-muted-foreground/50",
    }
}

pub fn health_text(h: Health) -> &'static str {
    match h {
        Health::Ok => "text-success",
        Health::Warning => "text-warning",
        Health::Error => "text-destructive",
        Health::Unknown => "text-muted-foreground",
    }
}

#[component]
pub fn HealthDot(health: Health, #[props(into, optional)] class: Option<String>) -> Element {
    let c = tw_merge!(
        "inline-block size-2.5 shrink-0 rounded-full",
        health_bg(health),
        class.as_deref().unwrap_or("")
    );
    rsx! { span { class: "{c}", title: health.label() } }
}

/// Small tinted label, color only where it means something.
#[component]
pub fn Tag(
    #[props(into)] text: String,
    #[props(default)] tone: Tone,
    #[props(into, optional)] title: Option<String>,
) -> Element {
    let c = match tone {
        Tone::Neutral => "bg-muted text-muted-foreground",
        Tone::Success => "bg-success/15 text-success",
        Tone::Warning => "bg-warning/15 text-warning",
        Tone::Danger => "bg-destructive/15 text-destructive",
        Tone::Outline => "border text-muted-foreground",
    };
    rsx! {
        span {
            class: "inline-flex w-fit items-center whitespace-nowrap rounded-md px-1.5 py-0.5 text-[11px] font-medium {c}",
            title: title.unwrap_or_default(),
            "{text}"
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Tone {
    #[default]
    Neutral,
    Success,
    Warning,
    Danger,
    Outline,
}

pub fn status_tone(s: BackupStatus) -> Tone {
    match s {
        BackupStatus::Stored => Tone::Success,
        BackupStatus::Queued | BackupStatus::Uploading => Tone::Warning,
        BackupStatus::Failed => Tone::Danger,
        BackupStatus::Deleted => Tone::Outline,
    }
}

// ------------------------------------------------------------------ Kopieren

pub fn copy_text(text: String) {
    spawn(async move {
        let ok = match web_sys::window() {
            Some(w) => {
                let p = w.navigator().clipboard().write_text(&text);
                wasm_bindgen_futures::JsFuture::from(p).await.is_ok()
            }
            None => false,
        };
        if ok {
            toast("In die Zwischenablage kopiert.");
        } else {
            toast_error("Kopieren nicht möglich – bitte von Hand markieren.");
        }
    });
}

#[component]
pub fn CopyButton(#[props(into)] text: String, #[props(default)] label: bool) -> Element {
    rsx! {
        button {
            r#type: "button",
            class: if label { format!("{} {BTN_SM}", btn(BTN_OUTLINE)) } else { format!("{} size-8 p-0 text-muted-foreground", btn(BTN_GHOST)) },
            title: "Kopieren",
            onclick: move |_| copy_text(text.clone()),
            icons::Copy { class: "size-4" }
            if label { "Kopieren" }
        }
    }
}

/// Monospace text with a copy button (examples, URLs, tokens).
#[component]
pub fn CodeBlock(#[props(into)] text: String, #[props(default)] large: bool) -> Element {
    let size = if large {
        "text-base font-semibold"
    } else {
        "text-xs"
    };
    rsx! {
        div { class: "flex items-start gap-2 rounded-lg border bg-muted/40 py-1.5 pr-1.5 pl-3",
            code { class: "min-w-0 flex-1 self-center break-all py-1 font-mono whitespace-pre-wrap {size}", "{text}" }
            CopyButton { text: text.clone() }
        }
    }
}

// ------------------------------------------------------------------ Bausteine

#[component]
pub fn Field(
    #[props(into)] label: String,
    #[props(into, optional)] hint: Option<String>,
    #[props(into, optional)] class: Option<String>,
    children: Element,
) -> Element {
    let c = tw_merge!("grid content-start gap-1.5", class.as_deref().unwrap_or(""));
    rsx! {
        label { class: "{c}",
            span { class: "text-sm font-medium leading-none", "{label}" }
            {children}
            if let Some(h) = hint {
                span { class: "text-xs text-muted-foreground", "{h}" }
            }
        }
    }
}

#[component]
pub fn PageHeader(
    #[props(into)] title: String,
    #[props(into, optional)] description: Option<String>,
    #[props(optional)] actions: Option<Element>,
    #[props(optional)] eyebrow: Option<Element>,
) -> Element {
    rsx! {
        div { class: "mb-6 flex flex-wrap items-end justify-between gap-4",
            div { class: "min-w-0",
                if let Some(e) = eyebrow {
                    div { class: "mb-1 text-xs font-medium text-muted-foreground", {e} }
                }
                h1 { class: "truncate text-2xl font-semibold tracking-tight", "{title}" }
                if let Some(d) = description {
                    p { class: "mt-1 text-sm text-muted-foreground", "{d}" }
                }
            }
            if let Some(a) = actions {
                div { class: "flex flex-wrap items-center gap-2", {a} }
            }
        }
    }
}

#[component]
pub fn EmptyState(
    #[props(into)] title: String,
    #[props(into, optional)] description: Option<String>,
    #[props(optional)] icon: Option<Element>,
    #[props(optional)] children: Option<Element>,
) -> Element {
    rsx! {
        div { class: "flex min-h-[12rem] flex-col items-center justify-center gap-3 rounded-xl border border-dashed p-8 text-center",
            if let Some(i) = icon {
                div { class: "grid size-12 place-items-center rounded-xl bg-muted text-muted-foreground [&_svg]:size-6", {i} }
            }
            div {
                p { class: "font-medium", "{title}" }
                if let Some(d) = description {
                    p { class: "mt-1 max-w-sm text-sm text-muted-foreground", "{d}" }
                }
            }
            if let Some(c) = children {
                {c}
            }
        }
    }
}

#[component]
pub fn Loading(#[props(into, optional)] text: Option<String>) -> Element {
    let text = text.unwrap_or_else(|| "Lädt …".into());
    rsx! {
        div { class: "flex items-center justify-center gap-2 py-10 text-sm text-muted-foreground",
            icons::LoaderCircle { class: "size-4 animate-spin" }
            "{text}"
        }
    }
}

#[component]
pub fn Callout(
    #[props(into)] text: String,
    #[props(default)] error: bool,
    #[props(default)] warning: bool,
    #[props(optional)] children: Option<Element>,
) -> Element {
    let c = if error {
        "border-destructive/40 bg-destructive/10"
    } else if warning {
        "border-warning/40 bg-warning/10"
    } else {
        "bg-muted/50"
    };
    rsx! {
        div { class: "flex items-start gap-2 rounded-lg border px-3 py-2.5 text-sm {c}",
            if error {
                icons::CircleAlert { class: "mt-0.5 size-4 shrink-0 text-destructive" }
            } else if warning {
                icons::TriangleAlert { class: "mt-0.5 size-4 shrink-0 text-warning" }
            } else {
                icons::Info { class: "mt-0.5 size-4 shrink-0 text-muted-foreground" }
            }
            div { class: "min-w-0 flex-1",
                span { class: "break-words", "{text}" }
                if let Some(c) = children {
                    {c}
                }
            }
        }
    }
}

/// Card with a title bar (rust-ui Card).
#[component]
pub fn Section(
    #[props(into)] title: String,
    #[props(into, optional)] description: Option<String>,
    #[props(optional)] actions: Option<Element>,
    #[props(default)] flush: bool,
    children: Element,
) -> Element {
    let body = if flush {
        ""
    } else {
        "px-5 py-4 sm:px-6 sm:py-5"
    };
    rsx! {
        section { class: "min-w-0 rounded-xl border bg-card text-card-foreground shadow-sm",
            div { class: "flex flex-wrap items-start justify-between gap-3 border-b px-5 py-4 sm:px-6",
                div { class: "min-w-0",
                    h2 { class: "font-semibold leading-none", "{title}" }
                    if let Some(d) = description {
                        p { class: "mt-1.5 text-sm text-muted-foreground", "{d}" }
                    }
                }
                if let Some(a) = actions {
                    div { class: "flex flex-wrap items-center gap-2", {a} }
                }
            }
            div { class: "{body}", {children} }
        }
    }
}

/// Key figure tile.
#[component]
pub fn Stat(
    #[props(into)] label: String,
    value: Element,
    #[props(into, optional)] hint: Option<String>,
) -> Element {
    rsx! {
        div { class: "min-w-0 rounded-xl border bg-card p-4 shadow-sm",
            p { class: "text-xs font-medium text-muted-foreground", "{label}" }
            div { class: "mt-1.5 truncate text-xl font-semibold tabular-nums tracking-tight", {value} }
            if let Some(h) = hint {
                p { class: "mt-0.5 truncate text-xs text-muted-foreground", "{h}" }
            }
        }
    }
}

/// Used/max bar (Filen storage).
#[component]
pub fn UsageBar(used: u64, max: u64) -> Element {
    let pct = if max > 0 {
        (used as f64 / max as f64 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let color = if pct >= 95.0 {
        "bg-destructive"
    } else if pct >= 85.0 {
        "bg-warning"
    } else {
        "bg-primary/80"
    };
    rsx! {
        div { class: "grid gap-1.5",
            div { class: "h-2 overflow-hidden rounded-full bg-muted",
                div { class: "h-full rounded-full {color}", style: "width: {pct:.1}%" }
            }
            div { class: "flex justify-between text-xs text-muted-foreground tabular-nums",
                span { {nrzbackup_shared::format_bytes(used)} " belegt" }
                if max > 0 {
                    span { "{pct:.0} % von " {nrzbackup_shared::format_bytes(max)} }
                }
            }
        }
    }
}

/// rust-ui Switch als kontrollierte Komponente.
#[component]
pub fn Toggle(
    checked: bool,
    onchange: EventHandler<bool>,
    #[props(into, optional)] label: Option<String>,
    #[props(into, optional)] hint: Option<String>,
) -> Element {
    let state = if checked {
        "bg-primary"
    } else {
        "bg-input dark:bg-input/80"
    };
    let thumb = if checked {
        "translate-x-[calc(100%-2px)] dark:bg-primary-foreground"
    } else {
        "translate-x-0 dark:bg-foreground"
    };
    rsx! {
        label { class: "inline-flex cursor-pointer items-start gap-3 text-sm",
            button {
                r#type: "button",
                role: "switch",
                "aria-checked": "{checked}",
                class: "peer mt-0.5 inline-flex h-[1.15rem] w-8 shrink-0 items-center rounded-full border border-transparent shadow-xs transition-all outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 {state}",
                onclick: move |_| onchange.call(!checked),
                span { class: "pointer-events-none block size-4 rounded-full bg-background ring-0 transition-transform {thumb}" }
            }
            span { class: "grid gap-0.5",
                if let Some(l) = label {
                    span { "{l}" }
                }
                if let Some(h) = hint {
                    span { class: "text-xs text-muted-foreground", "{h}" }
                }
            }
        }
    }
}
