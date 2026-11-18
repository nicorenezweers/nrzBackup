//! umbrelOS: pull fields in the source form (app or folder), folder browser and the
//! dialog that creates one source per installed app.

use dioxus::prelude::*;
use nrzbackup_shared::{self as shared, Retention, Schedule, ScheduleMode};

use crate::Route;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;

fn num(v: &str) -> u32 {
    v.trim().parse().unwrap_or(0)
}

const SEG_ON: &str = "rounded-md bg-background px-3 py-1.5 text-sm font-medium shadow-xs";
const SEG_OFF: &str = "rounded-md px-3 py-1.5 text-sm text-muted-foreground hover:text-foreground";

fn state_tone(state: &str) -> Tone {
    match state {
        "ready" | "running" => Tone::Success,
        "stopped" => Tone::Neutral,
        _ => Tone::Warning,
    }
}

/// Callout shown when the apps call fails (connection missing or unreachable).
#[component]
fn NotConnected(#[props(into)] error: String) -> Element {
    rsx! {
        Callout {
            text: format!("nrzBackup kann die umbrelOS-Apps gerade nicht lesen ({error})."),
            warning: true,
            div { class: "mt-2",
                Link { to: Route::Settings {}, class: "{btn(BTN_OUTLINE)} {BTN_SM}",
                    icons::Settings {}
                    "Zu den Einstellungen"
                }
            }
        }
    }
}

/// True when the optional umbrelOS token works (needed to stop apps during a backup).
fn use_can_stop() -> Memo<bool> {
    let settings = use_loader(api::settings);
    use_memo(move || {
        settings
            .data
            .read()
            .as_ref()
            .is_some_and(|s| s.umbrel.connected)
    })
}

/// Sensible "expected every … hours" for a schedule.
fn expect_for(s: &Schedule) -> u32 {
    match s.mode {
        ScheduleMode::Off => 0,
        ScheduleMode::Daily => 26,
        ScheduleMode::Interval => s.every_hours.max(1) * 2 + 1,
    }
}

// ------------------------------------------------------------------ Formularteil

/// Fields of the source form for [`shared::PullKind::Umbrel`].
#[component]
pub fn UmbrelPullFields(form: Signal<shared::SourceInput>) -> Element {
    let apps = use_loader(api::umbrel_apps);
    let can_stop = use_can_stop();
    let mut app_mode = use_signal(|| {
        let p = &form.peek().pull;
        !p.umbrel_app.is_empty() || p.umbrel_path.is_empty()
    });
    let mut browse_open = use_signal(|| false);

    let pull = form.read().pull.clone();
    let list = apps.data.read().clone();
    let selected = list
        .as_ref()
        .and_then(|l| l.iter().find(|a| a.id == pull.umbrel_app).cloned());
    let whole = selected
        .as_ref()
        .is_some_and(|a| a.app_path != a.data_path && pull.umbrel_path == a.app_path);
    let known = selected.is_some();

    rsx! {
        div { class: "grid gap-4",
            if let (Some(e), None) = (apps.error.read().clone(), list.as_ref()) {
                NotConnected { error: e }
            }
            Field { label: "Was sichern?",
                div { class: "grid w-fit grid-cols-2 gap-1 rounded-lg bg-muted p-1",
                    button {
                        r#type: "button",
                        class: if app_mode() { SEG_ON } else { SEG_OFF },
                        onclick: move |_| {
                            if !app_mode() {
                                app_mode.set(true);
                                let mut w = form.write();
                                w.pull.umbrel_path.clear();
                                w.pull.umbrel_app.clear();
                            }
                        },
                        "App"
                    }
                    button {
                        r#type: "button",
                        class: if app_mode() { SEG_OFF } else { SEG_ON },
                        onclick: move |_| {
                            app_mode.set(false);
                            let mut w = form.write();
                            w.pull.umbrel_app.clear();
                            w.pull.stop_app = false;
                        },
                        "Ordner"
                    }
                }
            }
            if app_mode() {
                Field { label: "App",
                    select {
                        class: SELECT,
                        onchange: move |e| {
                            let id = e.value();
                            let app = apps.data.peek().as_ref().and_then(|l| l.iter().find(|a| a.id == id).cloned());
                            let mut w = form.write();
                            match app {
                                Some(a) => {
                                    w.pull.umbrel_app = a.id.clone();
                                    w.pull.umbrel_path = a.data_path.clone();
                                    if w.name.trim().is_empty() {
                                        w.name = a.name.clone();
                                    }
                                }
                                None => {
                                    w.pull.umbrel_app.clear();
                                    w.pull.umbrel_path.clear();
                                }
                            }
                        },
                        option { value: "", selected: pull.umbrel_app.is_empty(), disabled: true,
                            if list.is_none() && apps.error.read().is_none() { "Apps werden geladen …" } else { "App wählen …" }
                        }
                        if !pull.umbrel_app.is_empty() && !known {
                            option { value: "{pull.umbrel_app}", selected: true, "{pull.umbrel_app}" }
                        }
                        for a in list.clone().unwrap_or_default() {
                            option { key: "{a.id}", value: "{a.id}", selected: a.id == pull.umbrel_app,
                                if a.state.is_empty() { "{a.name} ({a.id})" } else { "{a.name} ({a.id}) · {a.state}" }
                            }
                        }
                    }
                }
                if !pull.umbrel_path.is_empty() {
                    p { class: "-mt-2 text-xs text-muted-foreground",
                        "Gesichert wird "
                        code { class: "font-mono", "{pull.umbrel_path}" }
                        " direkt aus umbrelOS (als .tar.gz), ohne Token."
                    }
                }
                Toggle {
                    checked: whole,
                    label: "Ganzen App-Ordner sichern (statt nur data/)",
                    onchange: move |v: bool| {
                        if let Some(a) = selected.clone() {
                            form.write().pull.umbrel_path = if v { a.app_path } else { a.data_path };
                        }
                    },
                }
                Toggle {
                    checked: pull.stop_app,
                    label: "App beim Backup anhalten",
                    hint: if can_stop() { "Wird vor dem Kopieren gestoppt, gesichert und sofort wieder gestartet – empfohlen für Apps mit Datenbank." } else { "Wird vor dem Kopieren gestoppt und danach wieder gestartet. Braucht den umbrelOS-Token (Einstellungen → umbrelOS)." },
                    onchange: move |v| form.write().pull.stop_app = v,
                }
            } else {
                Field { label: "Ordner", hint: "Pfad in umbrelOS, z. B. /Home/Documents. /External und /Network gehen nur mit umbrelOS-Token.",
                    div { class: "flex gap-2",
                        input {
                            class: "{INPUT} font-mono",
                            value: "{pull.umbrel_path}",
                            placeholder: "/Home/Documents",
                            oninput: move |e| form.write().pull.umbrel_path = e.value(),
                        }
                        button {
                            r#type: "button",
                            class: "{btn(BTN_OUTLINE)}",
                            onclick: move |e| {
                                e.prevent_default();
                                browse_open.set(true);
                            },
                            icons::FolderOpen {}
                            "Durchsuchen"
                        }
                    }
                }
            }
        }
        if browse_open() {
            FolderBrowser {
                open: browse_open,
                start: if pull.umbrel_path.starts_with('/') { pull.umbrel_path.clone() } else { "/".to_string() },
                on_pick: move |p: String| form.write().pull.umbrel_path = p,
            }
        }
    }
}

// ------------------------------------------------------------------ Ordner-Browser

#[component]
fn FolderBrowser(open: Signal<bool>, start: String, on_pick: EventHandler<String>) -> Element {
    let mut path = use_signal(|| start.clone());
    let dir = use_resource(move || {
        let p = path();
        async move { api::umbrel_browse(&p).await }
    });

    let current = path();
    rsx! {
        // not dismissible: Esc / click outside would also close the source form behind it
        Modal {
            open,
            title: "Ordner wählen",
            description: "Klick auf einen Ordner öffnet ihn. Gesichert wird der angezeigte Ordner samt Inhalt.",
            class: "sm:max-w-xl",
            dismissible: false,
            div { class: "grid gap-3",
                div { class: "flex items-center gap-2",
                    button {
                        r#type: "button",
                        class: "{btn(BTN_OUTLINE)} {BTN_SM}",
                        disabled: current == "/",
                        onclick: move |_| {
                            let parent = match &*dir.peek() {
                                Some(Ok(d)) => d.parent.clone(),
                                _ => None,
                            };
                            let p = parent.unwrap_or_else(|| {
                                let cur = path.peek().trim_end_matches('/').to_string();
                                match cur.rfind('/') {
                                    Some(0) | None => "/".to_string(),
                                    Some(i) => cur[..i].to_string(),
                                }
                            });
                            path.set(p);
                        },
                        icons::ArrowUp {}
                        "Nach oben"
                    }
                    code { class: "min-w-0 flex-1 truncate rounded-md bg-muted px-2 py-1 font-mono text-xs", "{current}" }
                }
                div { class: "max-h-[50vh] min-h-40 overflow-y-auto rounded-lg border",
                    match &*dir.read_unchecked() {
                        None => rsx! { div { class: "p-4", Loading {} } },
                        Some(Err(e)) => rsx! {
                            div { class: "grid gap-2 p-3",
                                Callout { text: e.clone(), error: true }
                                if current != "/" {
                                    button { r#type: "button", class: "{btn(BTN_OUTLINE)} {BTN_SM} w-fit", onclick: move |_| path.set("/".into()),
                                        "Zum Anfang"
                                    }
                                }
                            }
                        },
                        Some(Ok(d)) if d.entries.is_empty() => rsx! {
                            p { class: "p-4 text-sm text-muted-foreground", "Dieser Ordner ist leer." }
                        },
                        Some(Ok(d)) => rsx! {
                            div { class: "divide-y",
                                for en in d.entries.clone() {
                                    if en.is_dir {
                                        button {
                                            key: "{en.path}",
                                            r#type: "button",
                                            class: "flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-accent",
                                            onclick: {
                                                let p = en.path.clone();
                                                move |_| path.set(p.clone())
                                            },
                                            icons::Folder { class: "size-4 shrink-0 text-primary" }
                                            span { class: "min-w-0 flex-1 truncate", "{en.name}" }
                                            icons::ChevronRight { class: "size-4 shrink-0 text-muted-foreground" }
                                        }
                                    } else {
                                        div { key: "{en.path}", class: "flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground/70",
                                            icons::File { class: "size-4 shrink-0" }
                                            span { class: "min-w-0 flex-1 truncate", "{en.name}" }
                                            if let Some(sz) = en.size {
                                                span { class: "text-xs tabular-nums", {shared::format_bytes(sz)} }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }
                }
                div { class: "flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
                    button { r#type: "button", class: "{btn(BTN_OUTLINE)}", onclick: move |_| open.set(false), "Abbrechen" }
                    button {
                        r#type: "button",
                        class: "{btn(BTN_PRIMARY)}",
                        disabled: current == "/",
                        onclick: move |_| {
                            on_pick.call(path());
                            open.set(false);
                        },
                        icons::Check {}
                        "Diesen Ordner wählen"
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Apps auf einmal

#[component]
fn ScheduleFields(schedule: Schedule, onchange: EventHandler<Schedule>) -> Element {
    let mode = schedule.mode;
    let hourly = mode == ScheduleMode::Interval && schedule.every_hours == 1;
    let s1 = schedule.clone();
    let s2 = schedule.clone();
    let s3 = schedule.clone();
    rsx! {
        div { class: "grid gap-3 sm:grid-cols-[12rem_1fr]",
            Field { label: "Zeitplan",
                select {
                    class: SELECT,
                    onchange: move |e| {
                        let mut s = s1.clone();
                        match e.value().as_str() {
                            "daily" => s.mode = ScheduleMode::Daily,
                            "hourly" => {
                                s.mode = ScheduleMode::Interval;
                                s.every_hours = 1;
                            }
                            "interval" => {
                                s.mode = ScheduleMode::Interval;
                                if s.every_hours <= 1 {
                                    s.every_hours = 6;
                                }
                            }
                            _ => s.mode = ScheduleMode::Off,
                        }
                        onchange.call(s);
                    },
                    option { value: "off", selected: mode == ScheduleMode::Off, "Aus (nur per Knopf)" }
                    option { value: "hourly", selected: hourly, "Stündlich" }
                    option { value: "daily", selected: mode == ScheduleMode::Daily, "Täglich um …" }
                    option { value: "interval", selected: mode == ScheduleMode::Interval && !hourly, "Alle … Stunden" }
                }
            }
            if mode == ScheduleMode::Daily {
                Field { label: "Uhrzeit", hint: "Ortszeit des Servers.",
                    input {
                        class: "{INPUT} sm:w-36",
                        r#type: "time",
                        value: "{schedule.time}",
                        oninput: move |e| {
                            let mut s = s2.clone();
                            s.time = e.value();
                            onchange.call(s);
                        },
                    }
                }
            }
            if mode == ScheduleMode::Interval && !hourly {
                Field { label: "Alle … Stunden",
                    input {
                        class: "{INPUT} sm:w-36",
                        r#type: "number",
                        min: "1",
                        value: "{schedule.every_hours}",
                        oninput: move |e| {
                            let mut s = s3.clone();
                            s.every_hours = num(&e.value()).max(1);
                            onchange.call(s);
                        },
                    }
                }
            }
        }
    }
}

/// Default selection: everything not backed up yet, except nrzBackup itself.
fn default_selection(apps: &[shared::UmbrelApp]) -> Vec<String> {
    apps.iter()
        .filter(|a| a.source_id.is_none() && a.id != "nrz-backup")
        .map(|a| a.id.clone())
        .collect()
}

/// Dialog: one source per selected umbrelOS app (`POST /api/umbrel/sources`).
#[component]
pub fn UmbrelAppsDialog(
    open: Signal<bool>,
    defaults: Retention,
    on_done: EventHandler<()>,
) -> Element {
    let apps = use_loader(api::umbrel_apps);
    let can_stop = use_can_stop();
    let mut picked = use_signal(|| None::<Vec<String>>);
    let mut folders = use_signal(Vec::<String>::new);
    let mut browse_open = use_signal(|| false);
    let mut whole = use_signal(|| false);
    let mut stop_ids = use_signal(Vec::<String>::new);
    let mut schedule = use_signal(|| Schedule {
        mode: ScheduleMode::Daily,
        time: "03:00".into(),
        every_hours: 24,
    });
    let mut expect = use_signal(|| 26u32);
    let mut retention = use_signal(|| defaults);
    let mut busy = use_signal(|| false);

    let list = apps.data.read().clone();
    let selection = move || -> Vec<String> {
        picked().unwrap_or_else(|| {
            apps.data
                .read()
                .as_deref()
                .map(default_selection)
                .unwrap_or_default()
        })
    };
    let chosen = selection();
    let n = chosen.len() + folders.read().len();

    let submit = move |_| {
        let ids = selection();
        if ids.is_empty() && folders.read().is_empty() {
            toast_error("Bitte mindestens eine App oder einen Ordner auswählen.");
            return;
        }
        busy.set(true);
        spawn(async move {
            let body = shared::UmbrelBulk {
                apps: ids,
                folders: folders(),
                whole_folder: whole(),
                stop_app: false,
                stop_apps: stop_ids(),
                schedule: schedule(),
                expect_hours: expect(),
                retention: retention(),
            };
            match api::umbrel_sources(&body).await {
                Ok(v) => {
                    toast(if v.len() == 1 {
                        "1 Quelle angelegt.".to_string()
                    } else {
                        format!("{} Quellen angelegt.", v.len())
                    });
                    open.set(false);
                    on_done.call(());
                }
                Err(e) => toast_error(e),
            }
            busy.set(false);
        });
    };

    let r = retention();
    rsx! {
        Modal {
            open,
            title: "umbrelOS sichern",
            description: "Apps und Ordner anhaken – nrzBackup liest sie direkt aus umbrelOS (ohne Token), packt sie als .tar.gz und lädt sie verschlüsselt nach Filen. Pro App bzw. Ordner entsteht eine Quelle.",
            class: "sm:max-w-2xl",
            div { class: "grid gap-6",
                if browse_open() {
                    FolderBrowser {
                        open: browse_open,
                        start: "/Home".to_string(),
                        on_pick: move |p: String| {
                            if !folders.peek().contains(&p) {
                                folders.write().push(p);
                            }
                        },
                    }
                }
                match (list.clone(), apps.error.read().clone()) {
                    (None, Some(e)) => rsx! { NotConnected { error: e } },
                    (None, None) => rsx! { Loading { text: "Apps werden geladen …" } },
                    (Some(l), _) if l.is_empty() => rsx! {
                        p { class: "text-sm text-muted-foreground", "Keine Apps gefunden." }
                    },
                    (Some(l), _) => rsx! {
                        div { class: "grid gap-2",
                            div { class: "flex items-center justify-between gap-2",
                                p { class: "text-sm font-medium", "Apps" }
                                div { class: "flex gap-1",
                                    button {
                                        r#type: "button",
                                        class: "{btn(BTN_GHOST)} {BTN_SM}",
                                        onclick: {
                                            let l = l.clone();
                                            move |_| picked.set(Some(l.iter().filter(|a| a.source_id.is_none()).map(|a| a.id.clone()).collect()))
                                        },
                                        "Alle auswählen"
                                    }
                                    button { r#type: "button", class: "{btn(BTN_GHOST)} {BTN_SM}", onclick: move |_| picked.set(Some(Vec::new())), "Keine" }
                                }
                            }
                            div { class: "max-h-[40vh] divide-y overflow-y-auto rounded-lg border",
                                for a in l.clone() {
                                    {
                                        let done = a.source_id.is_some();
                                        let on = !done && chosen.contains(&a.id);
                                        let id = a.id.clone();
                                        rsx! {
                                            div {
                                                key: "{a.id}",
                                                class: if done { "flex items-center gap-3 px-3 py-2 opacity-60" } else { "flex cursor-pointer items-center gap-3 px-3 py-2 hover:bg-accent/50" },
                                                onclick: move |_| {
                                                    if done {
                                                        return;
                                                    }
                                                    let mut cur = selection();
                                                    let was = cur.contains(&id);
                                                    cur.retain(|x| x != &id);
                                                    if !was {
                                                        cur.push(id.clone());
                                                    }
                                                    picked.set(Some(cur));
                                                },
                                                input {
                                                    r#type: "checkbox",
                                                    class: "pointer-events-none size-4 shrink-0 accent-primary",
                                                    checked: on,
                                                    disabled: done,
                                                    tabindex: "-1",
                                                }
                                                div { class: "min-w-0 flex-1",
                                                    p { class: "truncate text-sm font-medium", "{a.name}" }
                                                    p { class: "truncate font-mono text-xs text-muted-foreground", "{a.id}" }
                                                }
                                                if on && a.id != "nrz-backup" {
                                                    {
                                                        let sid = a.id.clone();
                                                        let stopped = stop_ids.read().contains(&a.id);
                                                        rsx! {
                                                            label {
                                                                class: "flex shrink-0 cursor-pointer items-center gap-1.5 rounded-md border px-2 py-1 text-xs text-muted-foreground hover:text-foreground",
                                                                title: "Die App wird vor dem Kopieren gestoppt und danach wieder gestartet",
                                                                onclick: move |e| e.stop_propagation(),
                                                                input {
                                                                    r#type: "checkbox",
                                                                    class: "size-3.5 accent-primary",
                                                                    checked: stopped,
                                                                    onchange: move |e| {
                                                                        let mut v = stop_ids.write();
                                                                        v.retain(|x| x != &sid);
                                                                        if e.checked() {
                                                                            v.push(sid.clone());
                                                                        }
                                                                    },
                                                                }
                                                                "anhalten"
                                                            }
                                                        }
                                                    }
                                                }
                                                if done {
                                                    Tag { text: "gesichert", tone: Tone::Success }
                                                }
                                                if !a.state.is_empty() {
                                                    Tag { text: a.state.clone(), tone: state_tone(&a.state) }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
                div { class: "grid gap-2",
                    div { class: "flex items-center justify-between gap-2",
                        p { class: "text-sm font-medium", "Ordner" }
                        button { r#type: "button", class: "{btn(BTN_OUTLINE)} {BTN_SM}", onclick: move |_| browse_open.set(true),
                            icons::FolderOpen {}
                            "Ordner hinzufügen"
                        }
                    }
                    if folders.read().is_empty() {
                        p { class: "text-xs text-muted-foreground", "Optional: z. B. /Home/Documents oder /Home/Photos." }
                    } else {
                        div { class: "divide-y rounded-lg border",
                            for f in folders() {
                                div { key: "{f}", class: "flex items-center gap-2 px-3 py-2 text-sm",
                                    icons::Folder { class: "size-4 shrink-0 text-primary" }
                                    code { class: "min-w-0 flex-1 truncate font-mono text-xs", "{f}" }
                                    button {
                                        r#type: "button",
                                        class: "{btn(BTN_GHOST)} {BTN_ICON} size-7",
                                        title: "Entfernen",
                                        onclick: {
                                            let f = f.clone();
                                            move |_| folders.write().retain(|x| x != &f)
                                        },
                                        icons::X { class: "size-4" }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "grid gap-3",
                    Toggle {
                        checked: whole(),
                        label: "Ganzen App-Ordner sichern",
                        hint: "Aus: nur den data/-Ordner der App (dort liegen die Daten).",
                        onchange: move |v| whole.set(v),
                    }
                    p { class: "text-xs text-muted-foreground",
                        "„anhalten“ bei einer App: Sie wird vor dem Kopieren gestoppt, gesichert und danach sofort wieder gestartet – empfohlen für Apps mit Datenbank. Später änderbar in der Quelle."
                    }
                    if !can_stop() && !stop_ids.read().is_empty() {
                        Callout { text: "Zum Anhalten braucht nrzBackup den umbrelOS-Token (Einstellungen → umbrelOS). Ohne ihn wird im laufenden Betrieb gesichert.", warning: true }
                    }
                }
                ScheduleFields {
                    schedule: schedule(),
                    onchange: move |s: Schedule| {
                        expect.set(expect_for(&s));
                        schedule.set(s);
                    },
                }
                Field { label: "Backup erwartet alle … Stunden", hint: "Kommt länger keins, wird die Ampel gelb (doppelt so lange: rot). 0 = keine Erwartung.",
                    input { class: "{INPUT} sm:w-40", r#type: "number", min: "0", value: "{expect}", oninput: move |e| expect.set(num(&e.value())) }
                }
                fieldset { class: "grid gap-3",
                    legend { class: "mb-1 text-sm font-semibold", "Aufbewahrung" }
                    div { class: "grid grid-cols-2 gap-3 sm:grid-cols-5",
                        Field { label: "Neueste", hint: "letzte N Kopien",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.keep_last}", oninput: move |e| retention.write().keep_last = num(&e.value()) }
                        }
                        Field { label: "Täglich", hint: "N Tage",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.daily}", oninput: move |e| retention.write().daily = num(&e.value()) }
                        }
                        Field { label: "Wöchentlich", hint: "N Wochen",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.weekly}", oninput: move |e| retention.write().weekly = num(&e.value()) }
                        }
                        Field { label: "Monatlich", hint: "N Monate",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.monthly}", oninput: move |e| retention.write().monthly = num(&e.value()) }
                        }
                        Field { label: "Jährlich", hint: "N Jahre",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.yearly}", oninput: move |e| retention.write().yearly = num(&e.value()) }
                        }
                    }
                }
                div { class: "flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
                    button { r#type: "button", class: "{btn(BTN_OUTLINE)}", onclick: move |_| open.set(false), "Abbrechen" }
                    button {
                        r#type: "button",
                        class: "{btn(BTN_PRIMARY)}",
                        disabled: busy() || n == 0,
                        onclick: submit,
                        if busy() { icons::LoaderCircle { class: "animate-spin" } } else { icons::Plus {} }
                        if n == 1 { "1 Quelle anlegen" } else { "{n} Quellen anlegen" }
                    }
                }
            }
        }
    }
}
