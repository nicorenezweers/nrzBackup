//! Settings: Filen account, general settings, access tokens and the MCP address.

use dioxus::prelude::*;
use nrzbackup_shared::{self as shared, ACCESS_TOKEN_PREFIX, format_bytes};

use super::sources::TokenReveal;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;

#[component]
pub fn Settings() -> Element {
    let mut settings = use_loader(api::settings);

    let Some(s) = settings.data.read().clone() else {
        return rsx! {
            PageHeader { title: "Einstellungen" }
            if let Some(e) = settings.error.read().clone() {
                Callout { text: e, error: true }
            } else {
                Loading {}
            }
        };
    };

    rsx! {
        PageHeader { title: "Einstellungen" }
        div { class: "grid gap-6",
            MasterKeySection { status: s.master_key.clone(), on_change: move |_| settings.reload() }
            FilenSection { status: s.filen.clone(), on_change: move |_| settings.reload() }
            UmbrelSection { key: "{s.umbrel.url}", status: s.umbrel.clone(), on_change: move |_| settings.reload() }
            GeneralSection { key: "{s.root_folder}{s.public_url}", settings: s.clone(), on_change: move |_| settings.reload() }
            TokensSection { settings: s.clone(), on_change: move |_| settings.reload() }
            if !s.version.is_empty() {
                p { class: "text-center text-xs text-muted-foreground", "nrzBackup {s.version}" }
            }
        }
    }
}

// ------------------------------------------------------------------ Hauptschlüssel

#[component]
fn MasterKeySection(status: shared::MasterKeyStatus, on_change: EventHandler<()>) -> Element {
    let mut shown = use_signal(|| None::<shared::MasterKeySecret>);
    let mut busy = use_signal(|| false);

    let show = move |_| {
        spawn(async move {
            let ask = Ask::new(
                "Hauptschlüssel anzeigen?",
                "Achte darauf, dass niemand mitliest. Wer diesen Schlüssel und Zugang zu deinem Filen-Konto hat, kann alle Backups lesen.",
            )
            .confirm("Anzeigen");
            if !confirm(ask).await {
                return;
            }
            busy.set(true);
            match api::masterkey().await {
                Ok(k) => shown.set(Some(k)),
                Err(e) => toast_error(e),
            }
            busy.set(false);
        });
    };
    let done = move |_| {
        spawn(async move {
            match api::masterkey_confirm().await {
                Ok(()) => {
                    toast("Danke – der Hauptschlüssel ist als gesichert vermerkt.");
                    shown.set(None);
                    on_change.call(());
                }
                Err(e) => toast_error(e),
            }
        });
    };

    rsx! {
        Section {
            title: "Hauptschlüssel",
            description: "Jedes Backup wird vor dem Upload mit diesem einen Schlüssel verschlüsselt (age, zusätzlich zur Verschlüsselung von Filen). Ohne ihn kann niemand die Backups lesen – auch nicht, wer in dein Filen-Konto kommt.",
            div { class: "grid gap-4",
                div { class: "flex flex-wrap items-center gap-2",
                    if !status.exists {
                        Tag { text: "nicht lesbar", tone: Tone::Danger }
                    } else if status.confirmed {
                        Tag { text: "aktiv · gesichert", tone: Tone::Success }
                    } else {
                        Tag { text: "aktiv · noch nicht gesichert", tone: Tone::Warning }
                    }
                    if let Some(ts) = status.created_at {
                        span { class: "text-xs text-muted-foreground", "erstellt " {fmt_time(ts)} }
                    }
                }
                if status.exists && !status.confirmed {
                    Callout {
                        text: "Bitte jetzt sichern: Schlüssel anzeigen, im Passwort-Manager UND zusätzlich offline (z. B. ausgedruckt) ablegen. Geht dieses Gerät kaputt, kommst du nur mit diesem Schlüssel an deine Backups.",
                        warning: true,
                    }
                }
                if let Some(k) = shown() {
                    div { class: "grid gap-3 rounded-lg border border-warning/40 bg-warning/5 p-4",
                        p { class: "text-sm font-medium", "Dein Hauptschlüssel" }
                        CodeBlock { text: k.secret_key.clone(), large: true }
                        div { class: "flex flex-wrap gap-2",
                            CopyButton { text: k.secret_key.clone(), label: true }
                        }
                        div { class: "grid gap-1 text-xs text-muted-foreground",
                            p { "Wiederherstellen ohne nrzBackup: Schlüssel in eine Datei schreiben (z. B. hauptschluessel.txt), das Backup aus Filen laden und mit dem kostenlosen Programm age (age-encryption.org) entschlüsseln:" }
                            code { class: "block rounded-md bg-background px-2 py-1.5 font-mono text-[11px] text-foreground", "age -d -i hauptschluessel.txt backup.tar.gz.age > backup.tar.gz" }
                            p { "In nrzBackup selbst entschlüsselt „Herunterladen“ automatisch." }
                        }
                        div { class: "flex flex-wrap gap-2",
                            button { class: "{btn(BTN_PRIMARY)}", onclick: done,
                                icons::Check {}
                                "Ich habe ihn sicher abgelegt"
                            }
                            button { class: "{btn(BTN_GHOST)}", onclick: move |_| shown.set(None), "Ausblenden" }
                        }
                    }
                } else if status.exists {
                    div {
                        button { class: "{btn(BTN_OUTLINE)}", disabled: busy(), onclick: show,
                            if busy() { icons::LoaderCircle { class: "animate-spin" } } else { icons::KeyRound {} }
                            "Hauptschlüssel anzeigen"
                        }
                    }
                }
                if !status.public_key.is_empty() {
                    p { class: "text-xs text-muted-foreground break-all",
                        "Öffentlicher Teil (nur zum Verschlüsseln): "
                        code { class: "font-mono", "{status.public_key}" }
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Filen

#[component]
fn FilenSection(status: shared::FilenStatus, on_change: EventHandler<()>) -> Element {
    let mut email = use_signal(|| status.email.clone());
    let mut password = use_signal(String::new);
    let mut code = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut show_form = use_signal(|| false);

    let connect = move |e: FormEvent| {
        e.prevent_default();
        if email().trim().is_empty() || password().is_empty() {
            error.set(Some("Bitte E-Mail und Passwort eingeben.".into()));
            return;
        }
        busy.set(true);
        error.set(None);
        spawn(async move {
            let body = shared::FilenConnect {
                email: email().trim().to_string(),
                password: password(),
                two_factor_code: code().trim().to_string(),
            };
            match api::filen_connect(&body).await {
                Ok(st) => {
                    password.set(String::new());
                    code.set(String::new());
                    if let Some(e) = st.error {
                        error.set(Some(e));
                    } else if st.connected {
                        toast("Filen-Konto verbunden.");
                        show_form.set(false);
                    }
                    on_change.call(());
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };
    let refresh = move |_| {
        busy.set(true);
        spawn(async move {
            match api::filen_refresh().await {
                Ok(st) => match st.error {
                    Some(e) => toast_error(e),
                    None => toast("Filen-Status aktualisiert."),
                },
                Err(e) => toast_error(e),
            }
            busy.set(false);
            on_change.call(());
        });
    };
    let disconnect = move |_| {
        spawn(async move {
            let ask = Ask::new(
                "Filen-Konto trennen?",
                "Neue Backups warten dann in der Warteschlange, bis wieder ein Konto verbunden ist. Die Kopien in Filen bleiben erhalten.",
            )
            .confirm("Trennen")
            .destructive();
            if !confirm(ask).await {
                return;
            }
            match api::filen_disconnect().await {
                Ok(_) => toast("Filen-Konto getrennt."),
                Err(e) => toast_error(e),
            }
            on_change.call(());
        });
    };

    let form_visible = !status.connected || show_form();
    rsx! {
        Section {
            title: "Filen-Konto",
            description: "Hierhin werden alle Backups hochgeladen.",
            actions: rsx! {
                if status.connected {
                    button { class: "{btn(BTN_OUTLINE)} {BTN_SM}", disabled: busy(), onclick: refresh,
                        icons::RefreshCw {}
                        "Aktualisieren"
                    }
                    button { class: "{btn(BTN_GHOST)} {BTN_SM} text-destructive hover:text-destructive", onclick: disconnect,
                        icons::Unplug {}
                        "Trennen"
                    }
                }
            },
            div { class: "grid gap-5",
                div { class: "grid gap-4 sm:grid-cols-2",
                    div { class: "grid content-start gap-1",
                        div { class: "flex items-center gap-2",
                            if status.connected {
                                Tag { text: "verbunden", tone: Tone::Success }
                                span { class: "truncate text-sm", "{status.email}" }
                            } else {
                                Tag { text: "nicht verbunden", tone: Tone::Danger }
                            }
                        }
                        if let Some(t) = status.checked_at {
                            p { class: "text-xs text-muted-foreground", "Geprüft " {fmt_relative(t)} }
                        }
                        if !status.root_folder.is_empty() {
                            p { class: "text-xs text-muted-foreground", "Stammordner: /{status.root_folder}" }
                        }
                    }
                    if status.connected {
                        div { class: "grid content-start gap-1.5",
                            p { class: "text-xs text-muted-foreground",
                                "Speicher: " {format_bytes(status.storage_used)}
                                if status.storage_max > 0 { " von " {format_bytes(status.storage_max)} }
                            }
                            UsageBar { used: status.storage_used, max: status.storage_max }
                        }
                    }
                }
                if let Some(e) = status.error.clone() {
                    Callout { text: e, error: true }
                }
                Callout { text: "Dateien werden vor dem Hochladen auf diesem Gerät mit AES-256-GCM verschlüsselt (Ende-zu-Ende, wie in den Filen-Apps)." }
                if form_visible {
                    form { class: "grid gap-4", onsubmit: connect,
                        if let Some(e) = error() {
                            Callout { text: e, error: true }
                        }
                        div { class: "grid gap-4 sm:grid-cols-2",
                            Field { label: "E-Mail",
                                input { class: INPUT, r#type: "email", autocomplete: "username", value: "{email}", oninput: move |e| email.set(e.value()) }
                            }
                            Field { label: "Passwort",
                                input { class: INPUT, r#type: "password", autocomplete: "current-password", value: "{password}", oninput: move |e| password.set(e.value()) }
                            }
                            Field { label: "2FA-Code", hint: "Nur nötig, wenn die Zwei-Faktor-Anmeldung aktiv ist.",
                                input {
                                    class: "{INPUT} sm:w-40",
                                    inputmode: "numeric",
                                    autocomplete: "one-time-code",
                                    maxlength: "6",
                                    value: "{code}",
                                    oninput: move |e| code.set(e.value()),
                                }
                            }
                        }
                        div { class: "flex flex-wrap gap-2",
                            button { r#type: "submit", class: "{btn(BTN_PRIMARY)}", disabled: busy(),
                                if busy() { icons::LoaderCircle { class: "animate-spin" } } else { icons::Plug {} }
                                if status.connected { "Neu verbinden" } else { "Verbinden" }
                            }
                            if status.connected {
                                button { r#type: "button", class: "{btn(BTN_GHOST)}", onclick: move |_| show_form.set(false), "Abbrechen" }
                            }
                        }
                    }
                } else {
                    button { class: "{btn(BTN_OUTLINE)} {BTN_SM} w-fit", onclick: move |_| show_form.set(true),
                        icons::KeyRound {}
                        "Anderes Konto verbinden"
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ umbrelOS

#[component]
fn UmbrelSection(status: shared::UmbrelStatus, on_change: EventHandler<()>) -> Element {
    let mut url = use_signal(|| {
        if status.url.is_empty() {
            "http://host.docker.internal".to_string()
        } else {
            status.url.clone()
        }
    });
    let mut token = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let save = move |e: FormEvent| {
        e.prevent_default();
        let u = url().trim().trim_end_matches('/').to_string();
        if u.is_empty() {
            error.set(Some("Bitte die Adresse von umbrelOS eingeben.".into()));
            return;
        }
        let t: String = token().chars().filter(|c| !c.is_whitespace()).collect();
        if !status.has_token && t.is_empty() {
            error.set(Some("Bitte einen MCP-Token eingeben.".into()));
            return;
        }
        busy.set(true);
        error.set(None);
        spawn(async move {
            let body = shared::UmbrelConnect {
                url: u,
                token: t,
                clear: false,
            };
            match api::umbrel_connect(&body).await {
                Ok(st) => {
                    token.set(String::new());
                    match st.error {
                        Some(e) => error.set(Some(e)),
                        None if st.connected => toast("umbrelOS verbunden."),
                        None => toast("Gespeichert."),
                    }
                    on_change.call(());
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };
    let remove = move |_| {
        spawn(async move {
            let ask = Ask::new(
                "Token entfernen?",
                "/Home und /Apps sichert nrzBackup weiter direkt. Ohne Token gehen nur /External, /Network und das Anhalten von Apps nicht mehr.",
            )
            .confirm("Entfernen")
            .destructive();
            if !confirm(ask).await {
                return;
            }
            let body = shared::UmbrelConnect {
                url: url().trim().trim_end_matches('/').to_string(),
                token: String::new(),
                clear: true,
            };
            match api::umbrel_connect(&body).await {
                Ok(_) => toast("Token entfernt."),
                Err(e) => toast_error(e),
            }
            on_change.call(());
        });
    };

    rsx! {
        Section {
            title: "umbrelOS",
            description: "nrzBackup liest /Home und die Daten aller Apps direkt (nur lesend) – ganz ohne Token. Was gesichert wird, wählst du unter Quellen → „umbrelOS sichern“.",
            div { class: "grid gap-5",
                div { class: "flex flex-wrap items-center gap-2",
                    if !status.local.is_empty() {
                        Tag { text: "direkt verbunden", tone: Tone::Success }
                        span { class: "text-sm text-muted-foreground",
                            "Lesbar: "
                            code { class: "font-mono", {status.local.join(", ")} }
                        }
                    } else {
                        Tag { text: "Ordner nicht eingebunden", tone: Tone::Warning }
                        span { class: "text-sm text-muted-foreground", "Bitte nrzBackup im nrzStore aktualisieren." }
                    }
                }
                details { class: "group rounded-lg border p-4", open: status.has_token,
                    summary { class: "cursor-pointer text-sm font-medium",
                        "Optional: umbrelOS-Token "
                        span { class: "font-normal text-muted-foreground", "(für /External, /Network und zum Anhalten von Apps während des Backups)" }
                    }
                    div { class: "mt-4 grid gap-4",
                div { class: "grid content-start gap-1",
                    div { class: "flex flex-wrap items-center gap-2",
                        if status.connected {
                            Tag { text: "Token verbunden", tone: Tone::Success }
                        } else if status.has_token {
                            Tag { text: "nicht erreichbar", tone: Tone::Danger }
                        } else {
                            Tag { text: "kein Token", tone: Tone::Neutral }
                        }
                        if !status.device.is_empty() {
                            span { class: "truncate text-sm", "{status.device}" }
                        }
                    }
                    if status.has_token && !status.token_hint.is_empty() {
                        p { class: "text-xs text-muted-foreground",
                            "Token: "
                            code { class: "font-mono", "{status.token_hint}" }
                        }
                    }
                }
                if let Some(e) = status.error.clone() {
                    Callout { text: e, error: true }
                }
                form { class: "grid gap-4", onsubmit: save,
                    if let Some(e) = error() {
                        Callout { text: e, error: true }
                    }
                    div { class: "grid gap-4 sm:grid-cols-2",
                        Field { label: "Adresse von umbrelOS", hint: "Aus dem App-Container: http://host.docker.internal",
                            input { class: INPUT, value: "{url}", placeholder: "http://host.docker.internal", oninput: move |e| url.set(e.value()) }
                        }
                        Field {
                            label: "MCP-Token",
                            hint: if status.has_token { "Leer lassen = gespeicherten Token behalten. In umbrelOS unter Einstellungen → AI agents (MCP) erstellen. Er braucht Zugriff auf Apps und Dateien." } else { "In umbrelOS unter Einstellungen → AI agents (MCP) erstellen. Er braucht Zugriff auf Apps und Dateien." },
                            input {
                                class: INPUT,
                                r#type: "password",
                                autocomplete: "new-password",
                                placeholder: if status.has_token { "•••••••• (gespeichert)" } else { "umbrelmcp_…" },
                                value: "{token}",
                                oninput: move |e| token.set(e.value()),
                            }
                        }
                    }
                    div { class: "flex flex-wrap gap-2",
                        button { r#type: "submit", class: "{btn(BTN_PRIMARY)}", disabled: busy(),
                            if busy() { icons::LoaderCircle { class: "animate-spin" } } else { icons::Server {} }
                            "Speichern & testen"
                        }
                        if status.has_token {
                            button { r#type: "button", class: "{btn(BTN_GHOST)} text-destructive hover:text-destructive", onclick: remove,
                                icons::Trash2 {}
                                "Token entfernen"
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

// ------------------------------------------------------------------ Allgemein

fn num(v: &str) -> u32 {
    v.trim().parse().unwrap_or(0)
}

#[component]
fn GeneralSection(settings: shared::Settings, on_change: EventHandler<()>) -> Element {
    let mut form = use_signal(|| shared::SettingsInput {
        root_folder: settings.root_folder.clone(),
        default_retention: settings.default_retention,
        public_url: settings.public_url.clone(),
        delete_permanently: settings.delete_permanently,
    });
    let mut busy = use_signal(|| false);

    let save = move |e: FormEvent| {
        e.prevent_default();
        busy.set(true);
        spawn(async move {
            let mut input = form();
            input.root_folder = input.root_folder.trim().trim_matches('/').to_string();
            input.public_url = input.public_url.trim().trim_end_matches('/').to_string();
            match api::save_settings(&input).await {
                Ok(_) => {
                    toast("Einstellungen gespeichert.");
                    on_change.call(());
                }
                Err(e) => toast_error(e),
            }
            busy.set(false);
        });
    };

    let f = form.read().clone();
    let r = f.default_retention;
    rsx! {
        Section { title: "Allgemein",
            form { class: "grid gap-5", onsubmit: save,
                div { class: "grid gap-4 sm:grid-cols-2",
                    Field { label: "Stammordner in Filen", hint: "Jede Quelle bekommt darin einen eigenen Unterordner.",
                        input { class: INPUT, value: "{f.root_folder}", placeholder: "nrzBackup", oninput: move |e| form.write().root_folder = e.value() }
                    }
                    Field { label: "Adresse für andere Apps", hint: "Unter dieser Adresse erreichen andere Apps nrzBackup (Push-URL, MCP).",
                        input { class: INPUT, value: "{f.public_url}", placeholder: "http://nrz-backup_app_1:8080", oninput: move |e| form.write().public_url = e.value() }
                    }
                }
                fieldset { class: "grid gap-3",
                    legend { class: "mb-1 text-sm font-medium", "Standard-Aufbewahrung für neue Quellen" }
                    div { class: "grid grid-cols-2 gap-3 sm:grid-cols-5",
                        Field { label: "Neueste", hint: "letzte N Kopien",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.keep_last}", oninput: move |e| form.write().default_retention.keep_last = num(&e.value()) }
                        }
                        Field { label: "Täglich", hint: "N Tage",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.daily}", oninput: move |e| form.write().default_retention.daily = num(&e.value()) }
                        }
                        Field { label: "Wöchentlich", hint: "N Wochen",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.weekly}", oninput: move |e| form.write().default_retention.weekly = num(&e.value()) }
                        }
                        Field { label: "Monatlich", hint: "N Monate",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.monthly}", oninput: move |e| form.write().default_retention.monthly = num(&e.value()) }
                        }
                        Field { label: "Jährlich", hint: "N Jahre",
                            input { class: INPUT, r#type: "number", min: "0", value: "{r.yearly}", oninput: move |e| form.write().default_retention.yearly = num(&e.value()) }
                        }
                    }
                }
                Toggle {
                    checked: f.delete_permanently,
                    label: "Alte Kopien endgültig löschen",
                    hint: "Aus: alte Kopien landen im Papierkorb von Filen und belegen dort weiter Speicher, bis er geleert wird.",
                    onchange: move |v| form.write().delete_permanently = v,
                }
                div {
                    button { r#type: "submit", class: "{btn(BTN_PRIMARY)}", disabled: busy(),
                        if busy() { icons::LoaderCircle { class: "animate-spin" } }
                        "Speichern"
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Zugangstokens

#[component]
fn TokensSection(settings: shared::Settings, on_change: EventHandler<()>) -> Element {
    let mut name = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut created = use_signal(String::new);
    let mut reveal = use_signal(|| false);

    let create = move |e: FormEvent| {
        e.prevent_default();
        let n = name().trim().to_string();
        if n.is_empty() {
            toast_error("Bitte einen Namen für den Token eingeben.");
            return;
        }
        busy.set(true);
        spawn(async move {
            match api::create_token(&n).await {
                Ok(t) => {
                    name.set(String::new());
                    created.set(t.token);
                    reveal.set(true);
                    on_change.call(());
                }
                Err(e) => toast_error(e),
            }
            busy.set(false);
        });
    };
    let revoke = move |t: shared::TokenInfo| {
        spawn(async move {
            let ask = Ask::new(
                format!("Token „{}“ widerrufen?", t.name),
                "Apps, die diesen Token benutzen, haben danach keinen Zugriff mehr.",
            )
            .confirm("Widerrufen")
            .destructive();
            if !confirm(ask).await {
                return;
            }
            match api::delete_token(t.id).await {
                Ok(()) => toast("Token widerrufen."),
                Err(e) => toast_error(e),
            }
            on_change.call(());
        });
    };

    rsx! {
        Section {
            title: "Zugangstokens für nrzDashboard und MCP",
            description: format!("Mit einem Zugangstoken ({ACCESS_TOKEN_PREFIX}…) können andere Apps den Status abfragen und Backups steuern."),
            div { class: "grid gap-5",
                div { class: "grid gap-2",
                    p { class: "text-sm font-medium", "MCP-Adresse" }
                    CodeBlock { text: settings.mcp_url.clone() }
                    ol { class: "ml-4 list-decimal space-y-0.5 text-xs text-muted-foreground",
                        li { "Unten einen Zugangstoken anlegen, z. B. „nrzDashboard“." }
                        li { "In nrzDashboard einen MCP-Server hinzufügen und diese Adresse eintragen." }
                        li { "Als Bearer-Token den neuen Zugangstoken einfügen." }
                    }
                }
                if settings.tokens.is_empty() {
                    p { class: "text-sm text-muted-foreground", "Noch keine Zugangstokens." }
                } else {
                    div { class: "divide-y rounded-lg border",
                        for t in settings.tokens.clone() {
                            div { key: "{t.id}", class: "flex items-center gap-3 px-3 py-2.5",
                                icons::Key { class: "size-4 shrink-0 text-muted-foreground" }
                                div { class: "min-w-0 flex-1",
                                    p { class: "truncate text-sm font-medium", "{t.name}" }
                                    p { class: "truncate text-xs text-muted-foreground",
                                        code { class: "font-mono", "{t.hint}" }
                                        " · angelegt {fmt_time(t.created_at)} · "
                                        match t.last_used {
                                            Some(u) => rsx! { "zuletzt benutzt " {fmt_relative(u)} },
                                            None => rsx! { "noch nie benutzt" },
                                        }
                                    }
                                }
                                button {
                                    class: "{btn(BTN_GHOST)} {BTN_SM} text-muted-foreground hover:text-destructive",
                                    onclick: {
                                        let t = t.clone();
                                        move |_| revoke(t.clone())
                                    },
                                    icons::Trash2 {}
                                    span { class: "max-sm:hidden", "Widerrufen" }
                                }
                            }
                        }
                    }
                }
                form { class: "flex flex-col gap-2 sm:flex-row", onsubmit: create,
                    input { class: "{INPUT} sm:max-w-xs", placeholder: "Name, z. B. nrzDashboard", value: "{name}", oninput: move |e| name.set(e.value()) }
                    button { r#type: "submit", class: "{btn(BTN_OUTLINE)}", disabled: busy(),
                        icons::Plus {}
                        "Token anlegen"
                    }
                }
            }
        }
        TokenReveal {
            open: reveal,
            title: "Neuer Zugangstoken",
            description: "Kopiere den Token jetzt in nrzDashboard bzw. deinen MCP-Client.",
            token: created(),
        }
    }
}
