//! Sources: list, form for adding/editing (`SourceInput`) and the one-time token view.

use dioxus::prelude::*;
use nrzbackup_shared::{self as shared, PullKind, Retention, ScheduleMode, format_bytes};

use crate::Route;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;
use crate::pages::umbrel::{UmbrelAppsDialog, UmbrelPullFields};

#[component]
pub fn Sources() -> Element {
    let mut list = use_loader(api::sources);
    let settings = use_loader(api::settings);
    let mut form_open = use_signal(|| false);
    let mut umbrel_open = use_signal(|| false);
    let mut token_open = use_signal(|| false);
    let mut created = use_signal(|| None::<shared::SourceCreated>);
    let nav = navigator();

    let defaults = settings
        .data
        .read()
        .as_ref()
        .map(|s| s.default_retention)
        .unwrap_or_default();

    rsx! {
        PageHeader {
            title: "Quellen",
            description: "Apps, deren Backups nrzBackup verschlüsselt nach Filen sichert.",
            actions: rsx! {
                button { class: "{btn(BTN_OUTLINE)}", onclick: move |_| form_open.set(true),
                    icons::Plus {}
                    "Andere Quelle"
                }
                button { class: "{btn(BTN_PRIMARY)}", onclick: move |_| umbrel_open.set(true),
                    icons::Server {}
                    "umbrelOS sichern"
                }
            },
        }
        if let Some(e) = list.error.read().clone() {
            div { class: "mb-4", Callout { text: e, error: true } }
        }
        match list.data.read().clone() {
            None => rsx! { Loading {} },
            Some(items) if items.is_empty() => rsx! {
                EmptyState {
                    title: "Noch keine Quellen",
                    description: "Wähle die umbrelOS-Apps und Ordner, die gesichert werden sollen – ganz ohne Token. nrzBackup liest sie direkt und lädt sie verschlüsselt nach Filen.",
                    icon: rsx! { icons::DatabaseBackup {} },
                    div { class: "flex flex-wrap justify-center gap-2",
                        button { class: "{btn(BTN_PRIMARY)}", onclick: move |_| umbrel_open.set(true),
                            icons::Server {}
                            "umbrelOS sichern"
                        }
                        button { class: "{btn(BTN_OUTLINE)}", onclick: move |_| form_open.set(true),
                            icons::Plus {}
                            "Andere Quelle"
                        }
                    }
                }
            },
            Some(items) => rsx! {
                SourceList { sources: items, on_change: move |_| list.reload() }
            },
        }
        if form_open() {
            SourceForm {
                open: form_open,
                existing: None,
                defaults,
                on_saved: move |r: Saved| {
                    if let Saved::Created(c) = r {
                        if c.source.pull.kind == PullKind::Umbrel {
                            nav.push(Route::SourceView { id: c.source.id });
                        } else {
                            created.set(Some(*c));
                            token_open.set(true);
                        }
                    }
                    list.reload();
                },
            }
        }
        if umbrel_open() {
            UmbrelAppsDialog { open: umbrel_open, defaults, on_done: move |_| list.reload() }
        }
        if let Some(c) = created() {
            TokenReveal {
                open: token_open,
                title: format!("Quelle „{}“ angelegt", c.source.name),
                description: "Mit diesem Push-Token sendet die App ihre Backups. Er wird nur jetzt angezeigt – kopiere ihn gleich in die App.",
                token: c.token.clone(),
                action: rsx! {
                    IntegrationButton { source_id: c.source.id }
                    button {
                        class: "{btn(BTN_OUTLINE)}",
                        onclick: move |_| {
                            token_open.set(false);
                            nav.push(Route::SourceView { id: c.source.id });
                        },
                        "Zur Quelle"
                        icons::ChevronRight {}
                    }
                },
            }
        }
    }
}

/// "Jetzt anfordern" for a source with a pull configured.
pub fn request_now(id: i64, mut done: impl FnMut() + 'static) {
    spawn(async move {
        match api::request_backup(id).await {
            Ok(r) if r.ok => toast(if r.message.is_empty() {
                "Backup angefordert.".into()
            } else {
                r.message
            }),
            Ok(r) => toast_error(r.message),
            Err(e) => toast_error(e),
        }
        done();
    });
}

/// Sources with traffic light, last backup, copies, size and the request button.
#[component]
pub fn SourceList(sources: Vec<shared::SourceSummary>, on_change: EventHandler<()>) -> Element {
    let mut busy = use_signal(|| None::<i64>);
    rsx! {
        div { class: "divide-y overflow-hidden rounded-xl border bg-card shadow-sm",
            for s in sources {
                div { key: "{s.id}", class: "flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3 sm:px-5",
                    Link { to: Route::SourceView { id: s.id }, class: "group flex min-w-0 flex-1 basis-60 items-center gap-3",
                        HealthDot { health: s.health }
                        div { class: "min-w-0",
                            p { class: "truncate font-medium group-hover:underline underline-offset-4", "{s.name}" }
                            p { class: "truncate text-xs text-muted-foreground",
                                if s.health_message.is_empty() { "{s.health.label()}" } else { "{s.health_message}" }
                            }
                        }
                    }
                    div { class: "grid grid-cols-3 gap-4 text-sm sm:w-80",
                        div { class: "min-w-0",
                            p { class: "text-[11px] text-muted-foreground", "Letzte Sicherung" }
                            p { class: "truncate", TimeAgo { ts: s.last_backup, empty: "noch nie" } }
                        }
                        div {
                            p { class: "text-[11px] text-muted-foreground", "Kopien" }
                            p { class: "tabular-nums",
                                "{s.copies}"
                                if s.queued > 0 {
                                    span { class: "ml-1 text-xs text-warning", "+{s.queued}" }
                                }
                            }
                        }
                        div {
                            p { class: "text-[11px] text-muted-foreground", "Größe" }
                            p { class: "tabular-nums", {format_bytes(s.stored_bytes)} }
                        }
                    }
                    div { class: "flex w-36 justify-end max-sm:w-full",
                        if s.pull.kind != PullKind::None {
                            button {
                                class: "{btn(BTN_OUTLINE)} {BTN_SM} max-sm:w-full",
                                disabled: busy() == Some(s.id),
                                onclick: move |_| {
                                    busy.set(Some(s.id));
                                    request_now(s.id, move || {
                                        busy.set(None);
                                        on_change.call(());
                                    });
                                },
                                if busy() == Some(s.id) {
                                    icons::LoaderCircle { class: "animate-spin" }
                                } else {
                                    icons::Download {}
                                }
                                "Jetzt anfordern"
                            }
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Token einmal zeigen

#[component]
pub fn TokenReveal(
    open: Signal<bool>,
    #[props(into)] title: String,
    #[props(into)] description: String,
    #[props(into)] token: String,
    #[props(optional)] action: Option<Element>,
) -> Element {
    rsx! {
        Modal { open, title, description, dismissible: false,
            div { class: "grid gap-4",
                CodeBlock { text: token.clone(), large: true }
                Callout { text: "Der Token wird nicht noch einmal angezeigt. Geht er verloren, erzeugst du einfach einen neuen.", warning: true }
                div { class: "flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
                    if let Some(a) = action {
                        {a}
                    }
                    button { class: "{btn(BTN_OUTLINE)}", onclick: move |_| copy_text(token.clone()),
                        icons::Copy {}
                        "Kopieren"
                    }
                    button { class: "{btn(BTN_PRIMARY)}", onclick: move |_| open.set(false), "Fertig" }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ KI-Anleitung

/// Button + dialog with the prompt for a coding agent (no token inside).
#[component]
pub fn IntegrationButton(source_id: i64, #[props(default)] small: bool) -> Element {
    let mut open = use_signal(|| false);
    let size = if small { BTN_SM } else { "" };
    rsx! {
        button {
            r#type: "button",
            class: "{btn(BTN_OUTLINE)} {size}",
            onclick: move |_| open.set(true),
            icons::Sparkles {}
            "KI-Anleitung"
        }
        if open() {
            IntegrationDialog { open, source_id }
        }
    }
}

#[component]
fn IntegrationDialog(open: Signal<bool>, source_id: i64) -> Element {
    let prompt = use_resource(move || api::integration(source_id));
    rsx! {
        Modal {
            open,
            title: "KI-Anleitung für die Integration",
            description: "Diesen Text einem Coding-Agenten (Claude Code, Codex, …) im Projekt der App geben. Er enthält alle Adressen, Endpunkte und Regeln dieser Quelle – aber keinen Token. Den Push-Token trägst du danach selbst in der App ein.",
            class: "sm:max-w-3xl",
            match &*prompt.read_unchecked() {
                None => rsx! { Loading {} },
                Some(Err(e)) => rsx! { Callout { text: e.clone(), error: true } },
                Some(Ok(p)) => {
                    let text = p.text.clone();
                    rsx! {
                        div { class: "grid gap-4",
                            pre { class: "max-h-[55vh] overflow-auto rounded-lg border bg-muted/40 p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words",
                                "{text}"
                            }
                            div { class: "flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
                                button { class: "{btn(BTN_OUTLINE)}", onclick: move |_| open.set(false), "Schließen" }
                                button { class: "{btn(BTN_PRIMARY)}", onclick: move |_| copy_text(text.clone()),
                                    icons::Copy {}
                                    "Anleitung kopieren"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Formular

pub enum Saved {
    Created(Box<shared::SourceCreated>),
    Updated,
}

fn input_from(s: &shared::SourceSummary) -> shared::SourceInput {
    shared::SourceInput {
        name: s.name.clone(),
        description: s.description.clone(),
        retention: s.retention,
        expect_hours: s.expect_hours,
        pull: shared::PullInput {
            kind: s.pull.kind,
            url: s.pull.url.clone(),
            token: String::new(),
            clear_token: false,
            method: s.pull.method.clone(),
            tool: s.pull.tool.clone(),
            arguments: s.pull.arguments.clone(),
            schedule: s.pull.schedule.clone(),
            umbrel_path: s.pull.umbrel_path.clone(),
            umbrel_app: s.pull.umbrel_app.clone(),
            stop_app: s.pull.stop_app,
        },
    }
}

fn num(v: &str) -> u32 {
    v.trim().parse().unwrap_or(0)
}

/// Add or edit a source in a dialog.
#[component]
pub fn SourceForm(
    open: Signal<bool>,
    existing: Option<shared::SourceSummary>,
    defaults: Retention,
    on_saved: EventHandler<Saved>,
) -> Element {
    let edit_id = existing.as_ref().map(|s| s.id);
    let has_token = existing.as_ref().is_some_and(|s| s.pull.has_token);
    let mut form = use_signal(|| match &existing {
        Some(s) => input_from(s),
        None => shared::SourceInput {
            retention: defaults,
            expect_hours: 24,
            pull: shared::PullInput {
                method: "GET".into(),
                tool: "create_backup".into(),
                ..Default::default()
            },
            ..Default::default()
        },
    });
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let submit = move |e: FormEvent| {
        e.prevent_default();
        let mut input = form();
        input.name = input.name.trim().to_string();
        if input.name.is_empty() {
            error.set(Some("Bitte einen Namen eingeben.".into()));
            return;
        }
        if input.pull.kind == PullKind::Umbrel {
            input.pull.umbrel_path = input.pull.umbrel_path.trim().to_string();
            if !input.pull.umbrel_path.starts_with('/') {
                error.set(Some(
                    "Bitte eine App oder einen Ordner wählen (Pfad beginnt mit /).".into(),
                ));
                return;
            }
        } else if input.pull.kind != PullKind::None && input.pull.url.trim().is_empty() {
            error.set(Some("Bitte die URL zum Anfordern eingeben.".into()));
            return;
        }
        if input.pull.kind == PullKind::Mcp {
            if input.pull.tool.trim().is_empty() {
                input.pull.tool = "create_backup".into();
            }
            let args = input.pull.arguments.trim();
            if !args.is_empty()
                && !serde_json::from_str::<serde_json::Value>(args).is_ok_and(|v| v.is_object())
            {
                error.set(Some(
                    "Die Argumente müssen ein JSON-Objekt sein, z. B. {\"full\": true}.".into(),
                ));
                return;
            }
        }
        if input.pull.method.is_empty() {
            input.pull.method = "GET".into();
        }
        busy.set(true);
        error.set(None);
        spawn(async move {
            let res = match edit_id {
                Some(id) => api::update_source(id, &input).await.map(|_| Saved::Updated),
                None => api::create_source(&input)
                    .await
                    .map(|c| Saved::Created(Box::new(c))),
            };
            busy.set(false);
            match res {
                Ok(r) => {
                    if matches!(r, Saved::Updated) {
                        toast("Quelle gespeichert.");
                    }
                    open.set(false);
                    on_saved.call(r);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let f = form.read().clone();
    let kind = f.pull.kind;
    let mode = f.pull.schedule.mode;
    let r = f.retention;
    let title = if edit_id.is_some() {
        "Quelle bearbeiten"
    } else {
        "Quelle hinzufügen"
    };

    rsx! {
        Modal { open, title, class: "sm:max-w-2xl",
            form { class: "grid gap-6", onsubmit: submit,
                if let Some(e) = error() {
                    Callout { text: e, error: true }
                }
                // ── Allgemein
                div { class: "grid gap-4",
                    Field { label: "Name", hint: "Wird auch als Ordnername in Filen verwendet.",
                        input { class: INPUT, value: "{f.name}", placeholder: "z. B. nrzBudget", oninput: move |e| form.write().name = e.value() }
                    }
                    Field { label: "Beschreibung",
                        input { class: INPUT, value: "{f.description}", placeholder: "optional", oninput: move |e| form.write().description = e.value() }
                    }
                    Field { label: "Backup erwartet alle … Stunden", hint: "Kommt länger keins, wird die Ampel gelb (doppelt so lange: rot). 0 = keine Erwartung.",
                        input { class: "{INPUT} sm:w-40", r#type: "number", min: "0", value: "{f.expect_hours}", oninput: move |e| form.write().expect_hours = num(&e.value()) }
                    }
                }
                // ── Aufbewahrung
                fieldset { class: "grid gap-3",
                    legend { class: "mb-1 text-sm font-semibold", "Aufbewahrung" }
                    p { class: "text-xs text-muted-foreground",
                        "Eine Kopie bleibt, wenn irgendeine Regel sie behalten will; alle anderen werden nach einem erfolgreichen Upload in Filen gelöscht. Die neueste Kopie bleibt immer."
                    }
                    div { class: "grid grid-cols-2 gap-3 sm:grid-cols-5",
                        Field { label: "Neueste", hint: "letzte N Kopien",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.keep_last}", oninput: move |e| form.write().retention.keep_last = num(&e.value()) }
                        }
                        Field { label: "Täglich", hint: "je Tag, N Tage",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.daily}", oninput: move |e| form.write().retention.daily = num(&e.value()) }
                        }
                        Field { label: "Wöchentlich", hint: "je Woche, N Wochen",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.weekly}", oninput: move |e| form.write().retention.weekly = num(&e.value()) }
                        }
                        Field { label: "Monatlich", hint: "je Monat, N Monate",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.monthly}", oninput: move |e| form.write().retention.monthly = num(&e.value()) }
                        }
                        Field { label: "Jährlich", hint: "je Jahr, N Jahre",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.yearly}", oninput: move |e| form.write().retention.yearly = num(&e.value()) }
                        }
                    }
                }
                // ── Anfordern
                fieldset { class: "grid gap-4",
                    legend { class: "mb-1 text-sm font-semibold", "Anfordern" }
                    p { class: "-mt-2 text-xs text-muted-foreground",
                        "Optional: nrzBackup fragt die App selbst nach einem Backup – per Knopf oder nach Zeitplan."
                    }
                    div { class: "flex w-fit max-w-full flex-wrap gap-1 rounded-lg bg-muted p-1",
                        for (k, label) in [(PullKind::None, "Keine"), (PullKind::Http, "HTTP"), (PullKind::Mcp, "MCP"), (PullKind::Umbrel, "umbrelOS (App oder Ordner)")] {
                            button {
                                r#type: "button",
                                class: if kind == k { "rounded-md bg-background px-3 py-1.5 text-sm font-medium shadow-xs" } else { "rounded-md px-3 py-1.5 text-sm text-muted-foreground hover:text-foreground" },
                                onclick: move |_| {
                                    let mut w = form.write();
                                    w.pull.kind = k;
                                    // umbrelOS has no other trigger than the schedule
                                    if k == PullKind::Umbrel && w.pull.schedule.mode == ScheduleMode::Off {
                                        w.pull.schedule.mode = ScheduleMode::Daily;
                                    }
                                },
                                "{label}"
                            }
                        }
                    }
                    if kind == PullKind::Umbrel {
                        UmbrelPullFields { form }
                    }
                    if kind == PullKind::Http || kind == PullKind::Mcp {
                        div { class: "grid gap-4 sm:grid-cols-[1fr_8rem]",
                            Field {
                                label: if kind == PullKind::Http { "URL" } else { "MCP-URL der App" },
                                hint: if kind == PullKind::Http { "Die Antwort auf diese Anfrage ist die Backup-Datei." } else { "z. B. http://nrz-budget_app_1:8080/mcp" },
                                input { class: INPUT, value: "{f.pull.url}", placeholder: "http://…", oninput: move |e| form.write().pull.url = e.value() }
                            }
                            if kind == PullKind::Http {
                                Field { label: "Methode",
                                    select { class: SELECT, value: "{f.pull.method}", onchange: move |e| form.write().pull.method = e.value(),
                                        option { value: "GET", selected: f.pull.method != "POST", "GET" }
                                        option { value: "POST", selected: f.pull.method == "POST", "POST" }
                                    }
                                }
                            }
                        }
                        Field {
                            label: "Token der App",
                            hint: if edit_id.is_some() && has_token { "Leer lassen = gespeicherten Token behalten." } else { "Wird als „Authorization: Bearer …“ mitgeschickt (optional)." },
                            input {
                                class: INPUT,
                                r#type: "password",
                                autocomplete: "new-password",
                                disabled: f.pull.clear_token,
                                placeholder: if has_token { "•••••••• (gespeichert)" } else { "" },
                                value: "{f.pull.token}",
                                oninput: move |e| form.write().pull.token = e.value(),
                            }
                        }
                        if has_token {
                            Toggle {
                                checked: f.pull.clear_token,
                                label: "Gespeicherten Token entfernen",
                                onchange: move |v: bool| {
                                    let mut w = form.write();
                                    w.pull.clear_token = v;
                                    if v {
                                        w.pull.token.clear();
                                    }
                                },
                            }
                        }
                        if kind == PullKind::Mcp {
                            Field { label: "Tool", hint: "Name des MCP-Tools, das ein Backup liefert.",
                                input { class: INPUT, value: "{f.pull.tool}", placeholder: "create_backup", oninput: move |e| form.write().pull.tool = e.value() }
                            }
                            Field { label: "Argumente (JSON)", hint: "Zusätzliche Argumente für das Tool, leer = keine.",
                                textarea { class: TEXTAREA, rows: "3", value: "{f.pull.arguments}", placeholder: "{{}}", oninput: move |e| form.write().pull.arguments = e.value() }
                            }
                        }
                    }
                    if kind != PullKind::None {
                        div { class: "grid gap-3 sm:grid-cols-[12rem_1fr]",
                            Field { label: "Zeitplan",
                                select {
                                    class: SELECT,
                                    onchange: move |e| {
                                        let mut w = form.write();
                                        let sch = &mut w.pull.schedule;
                                        match e.value().as_str() {
                                            "daily" => sch.mode = ScheduleMode::Daily,
                                            "hourly" => {
                                                sch.mode = ScheduleMode::Interval;
                                                sch.every_hours = 1;
                                            }
                                            "interval" => {
                                                sch.mode = ScheduleMode::Interval;
                                                if sch.every_hours <= 1 {
                                                    sch.every_hours = 6;
                                                }
                                            }
                                            _ => sch.mode = ScheduleMode::Off,
                                        }
                                    },
                                    option { value: "off", selected: mode == ScheduleMode::Off, "Aus (nur per Knopf)" }
                                    option { value: "hourly", selected: mode == ScheduleMode::Interval && f.pull.schedule.every_hours == 1, "Stündlich" }
                                    option { value: "daily", selected: mode == ScheduleMode::Daily, "Täglich um …" }
                                    option { value: "interval", selected: mode == ScheduleMode::Interval && f.pull.schedule.every_hours != 1, "Alle … Stunden" }
                                }
                            }
                            if mode == ScheduleMode::Daily {
                                Field { label: "Uhrzeit", hint: "Ortszeit des Servers.",
                                    input { class: "{INPUT} sm:w-36", r#type: "time", value: "{f.pull.schedule.time}", oninput: move |e| form.write().pull.schedule.time = e.value() }
                                }
                            }
                            if mode == ScheduleMode::Interval && f.pull.schedule.every_hours != 1 {
                                Field { label: "Alle … Stunden",
                                    input {
                                        class: "{INPUT} sm:w-36",
                                        r#type: "number",
                                        min: "1",
                                        value: "{f.pull.schedule.every_hours}",
                                        oninput: move |e| form.write().pull.schedule.every_hours = num(&e.value()).max(1),
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
                    button { r#type: "button", class: "{btn(BTN_OUTLINE)}", onclick: move |_| open.set(false), "Abbrechen" }
                    button { r#type: "submit", class: "{btn(BTN_PRIMARY)}", disabled: busy(),
                        if busy() {
                            icons::LoaderCircle { class: "animate-spin" }
                        }
                        if edit_id.is_some() { "Speichern" } else { "Quelle anlegen" }
                    }
                }
            }
        }
    }
}
