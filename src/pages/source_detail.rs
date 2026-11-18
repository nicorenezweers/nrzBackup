//! One source: status, how apps send backups, actions and the list of backups.

use dioxus::prelude::*;
use nrzbackup_shared::{
    self as shared, BackupOrigin, BackupStatus, PullKind, ScheduleMode, format_bytes,
};
use wasm_bindgen::JsCast;

use super::sources::{IntegrationButton, SourceForm, TokenReveal, request_now};
use crate::Route;
use crate::api;
use crate::components::common::*;
use crate::components::overlay::*;

#[component]
pub fn SourceView(id: i64) -> Element {
    // Remount when the id changes, so all state starts fresh.
    rsx! { SourcePage { key: "{id}", id } }
}

fn retention_text(r: &shared::Retention) -> String {
    let mut parts = Vec::new();
    if r.keep_last > 0 {
        parts.push(format!("{} neueste", r.keep_last));
    }
    for (n, one, many) in [
        (r.daily, "Tag", "Tage"),
        (r.weekly, "Woche", "Wochen"),
        (r.monthly, "Monat", "Monate"),
        (r.yearly, "Jahr", "Jahre"),
    ] {
        if n > 0 {
            parts.push(format!("{n} {}", if n == 1 { one } else { many }));
        }
    }
    if parts.is_empty() {
        "nur die neueste Kopie".into()
    } else {
        parts.join(" · ")
    }
}

fn schedule_text(p: &shared::PullView) -> String {
    match p.schedule.mode {
        ScheduleMode::Off => "nur per Knopf".into(),
        ScheduleMode::Daily => format!("täglich um {} Uhr", p.schedule.time),
        ScheduleMode::Interval if p.schedule.every_hours == 1 => "jede Stunde".into(),
        ScheduleMode::Interval => format!("alle {} Stunden", p.schedule.every_hours),
    }
}

#[component]
fn SourcePage(id: i64) -> Element {
    let mut detail = use_loader(move || api::source(id));
    use_poll(detail, 15);
    let settings = use_loader(api::settings);
    let mut edit_open = use_signal(|| false);
    let mut token_open = use_signal(|| false);
    let mut new_token = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let nav = navigator();

    let Some(d) = detail.data.read().clone() else {
        return rsx! {
            if let Some(e) = detail.error.read().clone() {
                Callout { text: e, error: true }
            } else {
                Loading {}
            }
        };
    };
    let s = d.source.clone();
    let defaults = settings
        .data
        .read()
        .as_ref()
        .map(|x| x.default_retention)
        .unwrap_or_default();

    let upload_id = format!("upload-{id}");
    let upload = {
        let upload_id = upload_id.clone();
        move |_| {
            let Some(input) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|doc| doc.get_element_by_id(&upload_id))
                .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            else {
                return;
            };
            let Some(file) = input.files().and_then(|f| f.get(0)) else {
                return;
            };
            busy.set(true);
            spawn(async move {
                match api::upload(id, &file).await {
                    Ok(r) => toast(if r.message.is_empty() {
                        format!("{} hochgeladen ({}).", r.filename, format_bytes(r.size))
                    } else {
                        r.message
                    }),
                    Err(e) => toast_error(e),
                }
                input.set_value("");
                busy.set(false);
                detail.reload();
            });
        }
    };

    let prune = move |_| {
        busy.set(true);
        spawn(async move {
            match api::prune(id).await {
                Ok(r) if r.ok => toast(if r.message.is_empty() {
                    "Aufbewahrung angewendet.".into()
                } else {
                    r.message
                }),
                Ok(r) => toast_error(r.message),
                Err(e) => toast_error(e),
            }
            busy.set(false);
            detail.reload();
        });
    };

    let rotate = move |_| {
        spawn(async move {
            let ask = Ask::new(
                "Neuen Push-Token erzeugen?",
                "Der bisherige Token wird sofort ungültig. Die App braucht danach den neuen Token, um Backups zu senden.",
            )
            .confirm("Neuen Token erzeugen")
            .destructive();
            if !confirm(ask).await {
                return;
            }
            match api::new_source_token(id).await {
                Ok(t) => {
                    new_token.set(t.token);
                    token_open.set(true);
                    detail.reload();
                }
                Err(e) => toast_error(e),
            }
        });
    };

    let name = s.name.clone();
    let remove = move |_| {
        let name = name.clone();
        spawn(async move {
            let ask = Ask::new(
                format!("Quelle „{name}“ löschen?"),
                "Die Quelle und ihr Push-Token werden entfernt. Die Kopien in Filen bleiben erhalten und können dort von Hand gelöscht werden.",
            )
            .confirm("Quelle löschen")
            .destructive();
            if !confirm(ask).await {
                return;
            }
            match api::delete_source(id).await {
                Ok(()) => {
                    toast("Quelle gelöscht.");
                    nav.push(Route::Sources {});
                }
                Err(e) => toast_error(e),
            }
        });
    };

    let token = format!("<TOKEN>  (beginnt mit {})", s.token_hint);
    let curl = format!(
        "curl -X POST -H \"Authorization: Bearer <TOKEN>\" --data-binary @backup.tar.gz \"{}?name=backup.tar.gz\"",
        d.push_url
    );
    let pull = s.pull.clone();
    let can_stop = settings
        .data
        .read()
        .as_ref()
        .is_some_and(|x| x.umbrel.connected);
    let is_umbrel = pull.kind == PullKind::Umbrel;

    rsx! {
        PageHeader {
            title: s.name.clone(),
            description: if s.description.is_empty() { None } else { Some(s.description.clone()) },
            eyebrow: rsx! {
                Link { to: Route::Sources {}, class: "inline-flex items-center gap-1 hover:text-foreground",
                    icons::ArrowLeft { class: "size-3.5" }
                    "Quellen"
                }
            },
            actions: rsx! {
                button { class: "{btn(BTN_OUTLINE)}", onclick: move |_| edit_open.set(true),
                    icons::Pencil {}
                    "Bearbeiten"
                }
                if pull.kind != PullKind::None {
                    button {
                        class: "{btn(BTN_PRIMARY)}",
                        disabled: busy(),
                        onclick: move |_| {
                            busy.set(true);
                            request_now(id, move || {
                                busy.set(false);
                                detail.reload();
                            });
                        },
                        icons::Download {}
                        "Jetzt anfordern"
                    }
                }
            },
        }

        div { class: "grid gap-6",
            // ── Status
            div { class: "grid gap-4 rounded-xl border bg-card p-5 shadow-sm sm:p-6 lg:grid-cols-[1.2fr_1fr]",
                div { class: "flex items-start gap-4",
                    div { class: "mt-1 grid size-10 shrink-0 place-items-center rounded-full bg-muted",
                        HealthDot { health: s.health, class: "size-4" }
                    }
                    div { class: "min-w-0",
                        p { class: "font-semibold {health_text(s.health)}", "{s.health.label()}" }
                        if !s.health_message.is_empty() {
                            p { class: "text-sm text-muted-foreground", "{s.health_message}" }
                        }
                        if let Some(e) = s.last_error.clone() {
                            p { class: "mt-2 break-words text-sm text-destructive", "{e}" }
                        }
                    }
                }
                dl { class: "grid grid-cols-2 gap-x-4 gap-y-3 text-sm",
                    div {
                        dt { class: "text-xs text-muted-foreground", "Letzte Sicherung" }
                        dd { TimeAgo { ts: s.last_backup, empty: "noch nie" } }
                    }
                    div {
                        dt { class: "text-xs text-muted-foreground", "Kopien in Filen" }
                        dd { class: "tabular-nums",
                            "{s.copies} · " {format_bytes(s.stored_bytes)}
                            if s.queued > 0 {
                                span { class: "text-warning", " · {s.queued} wartend" }
                            }
                        }
                    }
                    div { class: "col-span-2",
                        dt { class: "text-xs text-muted-foreground", "Pfad in Filen" }
                        dd { class: "flex items-center gap-1",
                            code { class: "truncate font-mono text-xs", "{s.filen_path}" }
                            CopyButton { text: s.filen_path.clone() }
                        }
                    }
                    div { class: "col-span-2",
                        dt { class: "text-xs text-muted-foreground", "Aufbewahrung" }
                        dd { {retention_text(&s.retention)} }
                    }
                    if s.expect_hours > 0 {
                        div {
                            dt { class: "text-xs text-muted-foreground", "Erwartet" }
                            dd { "alle {s.expect_hours} Std." }
                        }
                    }
                    if pull.kind == PullKind::Umbrel {
                        div { class: "col-span-2",
                            dt { class: "text-xs text-muted-foreground", "Anfordern" }
                            dd { class: "break-words",
                                "umbrelOS: "
                                code { class: "font-mono text-xs", "{pull.umbrel_path}" }
                                if pull.stop_app { " · App wird angehalten" }
                                " · " {schedule_text(&pull)}
                            }
                        }
                    } else if pull.kind != PullKind::None {
                        div {
                            dt { class: "text-xs text-muted-foreground", "Anfordern" }
                            dd {
                                if pull.kind == PullKind::Http { "HTTP" } else { "MCP" }
                                ", " {schedule_text(&pull)}
                            }
                        }
                    }
                    if pull.kind != PullKind::None {
                        if let Some(n) = pull.next_pull {
                            div {
                                dt { class: "text-xs text-muted-foreground", "Nächste Anforderung" }
                                dd { TimeAgo { ts: Some(n) } }
                            }
                        }
                        if let Some(l) = pull.last_pull {
                            div {
                                dt { class: "text-xs text-muted-foreground", "Zuletzt angefordert" }
                                dd { TimeAgo { ts: Some(l) } }
                            }
                        }
                    }
                }
                if let Some(e) = pull.last_error.clone() {
                    div { class: "lg:col-span-2",
                        Callout { text: format!("Letzte Anforderung fehlgeschlagen: {e}"), warning: true }
                    }
                }
                if pull.kind == PullKind::Umbrel && !pull.umbrel_app.is_empty() {
                    div { class: "grid gap-2 rounded-lg border p-3 lg:col-span-2",
                        Toggle {
                            checked: pull.stop_app,
                            label: "App beim Backup anhalten",
                            hint: format!("{} wird vor dem Kopieren gestoppt, gesichert und danach sofort wieder gestartet – empfohlen für Apps mit Datenbank. Die App ist dabei kurz nicht erreichbar.", pull.umbrel_app),
                            onchange: move |v: bool| {
                                spawn(async move {
                                    match api::set_stop_app(id, v).await {
                                        Ok(_) => {
                                            toast(if v { "Die App wird bei Backups angehalten." } else { "Die App läuft bei Backups weiter." });
                                            detail.reload();
                                        }
                                        Err(e) => toast_error(e),
                                    }
                                });
                            },
                        }
                        if pull.stop_app && !can_stop {
                            Callout {
                                text: "Zum Anhalten braucht nrzBackup den umbrelOS-Token (Einstellungen → umbrelOS → Optional: umbrelOS-Token). Ohne ihn wird im laufenden Betrieb gesichert.",
                                warning: true,
                            }
                        }
                    }
                }
            }

            // ── Aktionen
            div { class: "flex flex-wrap gap-2",
                input { id: "{upload_id}", class: "hidden", r#type: "file", onchange: upload }
                label { r#for: "{upload_id}", class: "{btn(BTN_OUTLINE)} {BTN_SM}",
                    if busy() { icons::LoaderCircle { class: "animate-spin" } } else { icons::Upload {} }
                    "Datei hochladen"
                }
                button { class: "{btn(BTN_OUTLINE)} {BTN_SM}", disabled: busy(), onclick: prune,
                    icons::Scissors {}
                    "Aufbewahrung jetzt anwenden"
                }
                if !is_umbrel {
                    button { class: "{btn(BTN_OUTLINE)} {BTN_SM}", onclick: rotate,
                        icons::KeyRound {}
                        "Neuer Push-Token"
                    }
                }
                div { class: "flex-1" }
                button { class: "{btn(BTN_GHOST)} {BTN_SM} text-destructive hover:text-destructive", onclick: remove,
                    icons::Trash2 {}
                    "Quelle löschen"
                }
            }

            // ── So sendet eine App Backups (umbrelOS-Quellen brauchen das nicht)
            if !is_umbrel {
                Section {
                    title: "So sendet eine App Backups",
                    description: "Jede Datei wird auf diesem Gerät verschlüsselt und nach Filen hochgeladen. Hier bleibt nichts liegen.",
                    div { class: "grid gap-5",
                        div { class: "flex flex-col gap-3 rounded-lg border bg-muted/30 p-4 sm:flex-row sm:items-center",
                            div { class: "flex-1",
                                p { class: "text-sm font-medium", "Integration mit KI bauen" }
                                p { class: "mt-0.5 text-xs text-muted-foreground",
                                    "Fertiger Prompt für einen Coding-Agenten: Senden, Anfordern (MCP-Tool), Wiederherstellen, Einstellungen und Tests – mit allen Daten dieser Quelle, ohne Token."
                                }
                            }
                            IntegrationButton { source_id: id, small: true }
                        }
                        div { class: "grid gap-2",
                            p { class: "text-sm font-medium", "Push-Token" }
                            CodeBlock { text: token }
                            p { class: "text-xs text-muted-foreground", "Der vollständige Token wurde beim Anlegen einmal angezeigt. Vergessen? Erzeuge einen neuen." }
                        }
                        div { class: "grid gap-2",
                            p { class: "text-sm font-medium", "HTTP (curl)" }
                            CodeBlock { text: curl }
                        }
                        div { class: "grid gap-2",
                            p { class: "text-sm font-medium", "MCP" }
                            CodeBlock { text: d.mcp_url.clone() }
                            div { class: "grid gap-2 text-sm text-muted-foreground sm:grid-cols-2",
                                div { class: "rounded-lg border p-3",
                                    p { code { class: "font-mono text-foreground", "backup_file" } }
                                    p { class: "mt-1 text-xs",
                                        "Argumente: " code { class: "font-mono", "filename" } ", " code { class: "font-mono", "content_base64" }
                                    }
                                }
                                div { class: "rounded-lg border p-3",
                                    p { code { class: "font-mono text-foreground", "backup_from_url" } }
                                    p { class: "mt-1 text-xs",
                                        "Argumente: " code { class: "font-mono", "url" } ", " code { class: "font-mono", "filename" } ", optional " code { class: "font-mono", "headers" }
                                    }
                                }
                            }
                            p { class: "text-xs text-muted-foreground", "Als Bearer-Token dient der Push-Token dieser Quelle." }
                        }
                    }
                }
            }

            // ── Backups
            Section { title: "Backups", description: "Neueste zuerst, inklusive wartender, fehlgeschlagener und gelöschter Kopien.", flush: true,
                if d.backups.is_empty() {
                    div { class: "p-5",
                        EmptyState { title: "Noch keine Backups", description: "Sobald die App etwas sendet, erscheint es hier.", icon: rsx! { icons::Inbox {} } }
                    }
                } else {
                    BackupTable { backups: d.backups.clone(), show_source: false, actions: true, on_change: move |_| detail.reload() }
                }
            }
        }

        if edit_open() {
            SourceForm {
                open: edit_open,
                existing: Some(s.clone()),
                defaults,
                on_saved: move |_| detail.reload(),
            }
        }
        TokenReveal {
            open: token_open,
            title: "Neuer Push-Token",
            description: "Trage diesen Token in der App ein. Der alte Token funktioniert nicht mehr.",
            token: new_token(),
        }
    }
}

fn origin_label(o: BackupOrigin) -> &'static str {
    match o {
        BackupOrigin::Push => "Push",
        BackupOrigin::Pull => "Angefordert",
        BackupOrigin::Mcp => "MCP",
        BackupOrigin::Upload => "Hochgeladen",
    }
}

fn kept_label(k: &str) -> &str {
    match k {
        "last" => "Neueste",
        "daily" => "Tag",
        "weekly" => "Woche",
        "monthly" => "Monat",
        "yearly" => "Jahr",
        other => other,
    }
}

/// Backups as a table (wide screens) or cards (phones).
#[component]
pub fn BackupTable(
    backups: Vec<shared::BackupItem>,
    show_source: bool,
    actions: bool,
    on_change: EventHandler<()>,
) -> Element {
    let mut busy = use_signal(|| None::<i64>);

    let mut retry = move |id: i64| {
        busy.set(Some(id));
        spawn(async move {
            match api::retry_backup(id).await {
                Ok(r) if r.ok => toast(if r.message.is_empty() {
                    "Wird erneut versucht.".into()
                } else {
                    r.message
                }),
                Ok(r) => toast_error(r.message),
                Err(e) => toast_error(e),
            }
            busy.set(None);
            on_change.call(());
        });
    };
    let remove = move |b: shared::BackupItem| {
        spawn(async move {
            let ask = Ask::new(
                "Kopie löschen?",
                format!(
                    "„{}“ vom {} wird in Filen gelöscht.",
                    b.filename,
                    fmt_time(b.created_at)
                ),
            )
            .confirm("Löschen")
            .destructive();
            if !confirm(ask).await {
                return;
            }
            busy.set(Some(b.id));
            match api::delete_backup(b.id).await {
                Ok(()) => toast("Kopie gelöscht."),
                Err(e) => toast_error(e),
            }
            busy.set(None);
            on_change.call(());
        });
    };

    rsx! {
        div { class: "overflow-x-auto",
            table { class: "w-full text-sm",
                thead { class: "max-md:hidden",
                    tr { class: "border-b text-left text-xs text-muted-foreground",
                        th { class: "px-5 py-2.5 font-medium", "Datum" }
                        if show_source {
                            th { class: "px-3 py-2.5 font-medium", "Quelle" }
                        }
                        th { class: "px-3 py-2.5 font-medium", "Datei" }
                        th { class: "px-3 py-2.5 text-right font-medium", "Größe" }
                        th { class: "px-3 py-2.5 font-medium", "Status" }
                        th { class: "px-3 py-2.5 font-medium", "Herkunft" }
                        if actions {
                            th { class: "px-5 py-2.5" }
                        }
                    }
                }
                tbody { class: "divide-y",
                    for b in backups {
                        tr { key: "{b.id}", class: "align-top max-md:flex max-md:flex-wrap max-md:items-center max-md:gap-x-3 max-md:gap-y-1 max-md:px-4 max-md:py-3",
                            td { class: "whitespace-nowrap px-5 py-3 tabular-nums max-md:order-1 max-md:p-0 max-md:text-xs max-md:text-muted-foreground",
                                span { title: fmt_relative(b.created_at), {fmt_time(b.created_at)} }
                            }
                            if show_source {
                                td { class: "px-3 py-3 max-md:order-2 max-md:p-0 max-md:text-xs",
                                    Link { to: Route::SourceView { id: b.source_id }, class: "hover:underline underline-offset-4", "{b.source_name}" }
                                }
                            }
                            td { class: "min-w-0 px-3 py-3 max-md:order-0 max-md:w-full max-md:p-0",
                                p { class: "break-all font-medium md:font-normal", title: "{b.stored_name}", "{b.filename}" }
                                if !b.kept_by.is_empty() && b.status == BackupStatus::Stored {
                                    div { class: "mt-1 flex flex-wrap gap-1",
                                        for k in b.kept_by.iter() {
                                            Tag { text: kept_label(k).to_string(), tone: Tone::Outline, title: "Wird von dieser Aufbewahrungsregel behalten" }
                                        }
                                    }
                                }
                                if let Some(e) = b.error.clone() {
                                    p { class: "mt-1 break-words text-xs text-destructive",
                                        "{e}"
                                        if b.attempts > 1 { " ({b.attempts} Versuche)" }
                                    }
                                }
                            }
                            td { class: "whitespace-nowrap px-3 py-3 text-right tabular-nums max-md:order-3 max-md:p-0 max-md:text-xs max-md:text-muted-foreground",
                                {format_bytes(b.size)}
                            }
                            td { class: "px-3 py-3 max-md:order-4 max-md:p-0",
                                Tag { text: b.status.label(), tone: status_tone(b.status) }
                            }
                            td { class: "whitespace-nowrap px-3 py-3 text-muted-foreground max-md:order-5 max-md:p-0 max-md:text-xs",
                                {origin_label(b.origin)}
                            }
                            if actions {
                                td { class: "whitespace-nowrap px-5 py-2 text-right max-md:order-6 max-md:ml-auto max-md:p-0",
                                    div { class: "inline-flex gap-1",
                                        if b.status == BackupStatus::Stored {
                                            a {
                                                class: "{btn(BTN_GHOST)} size-8 p-0 text-muted-foreground",
                                                href: api::download_url(b.id),
                                                download: "{b.filename}",
                                                title: "Herunterladen",
                                                icons::Download { class: "size-4" }
                                            }
                                        }
                                        if matches!(b.status, BackupStatus::Queued | BackupStatus::Failed) {
                                            button {
                                                class: "{btn(BTN_GHOST)} size-8 p-0 text-muted-foreground",
                                                title: "Erneut versuchen",
                                                disabled: busy() == Some(b.id),
                                                onclick: move |_| retry(b.id),
                                                icons::RotateCw { class: "size-4" }
                                            }
                                        }
                                        if b.status == BackupStatus::Stored {
                                            button {
                                                class: "{btn(BTN_GHOST)} size-8 p-0 text-muted-foreground hover:text-destructive",
                                                title: "Löschen",
                                                disabled: busy() == Some(b.id),
                                                onclick: {
                                                    let b = b.clone();
                                                    move |_| remove(b.clone())
                                                },
                                                icons::Trash2 { class: "size-4" }
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
}
