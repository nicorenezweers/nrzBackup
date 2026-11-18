//! File explorer for Filen.io (decrypted on the fly) and umbrelOS (/Home, /External, /Network, /Apps).

use dioxus::prelude::*;
use nrzbackup_shared::format_bytes;

use crate::Route;
use crate::api::{self, Res};
use crate::components::common::*;

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Filen,
    Umbrel,
}

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    Name,
    Size,
    Date,
}

/// One row, the same for both sides.
#[derive(Clone, PartialEq)]
struct Row {
    key: String,
    name: String,
    /// Full path (umbrelOS) or path below the Filen root.
    path: String,
    is_dir: bool,
    size: Option<u64>,
    modified: Option<i64>,
    mime: String,
}

#[derive(Clone, PartialEq, Default)]
struct Listing {
    path: String,
    parent: Option<String>,
    /// Filen folder uuid (for downloads).
    folder: String,
    rows: Vec<Row>,
}

fn join(base: &str, name: &str) -> String {
    format!("{}/{name}", base.trim_end_matches('/'))
}

fn parent_of(p: &str) -> String {
    let cur = p.trim_end_matches('/');
    match cur.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => cur[..i].into(),
    }
}

fn ext(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}

/// Browsers can show these directly.
fn previewable(r: &Row) -> bool {
    let m = r.mime.as_str();
    m.starts_with("image/")
        || m.starts_with("video/")
        || m.starts_with("audio/")
        || m.starts_with("text/")
        || m == "application/pdf"
        || m == "application/json"
        || matches!(
            ext(&r.name).as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "pdf" | "txt" | "md" | "json" | "log" | "mp4" | "mp3"
        )
}

#[component]
fn RowIcon(row: Row) -> Element {
    let cls = "size-4 shrink-0";
    if row.is_dir {
        return rsx! { icons::Folder { class: "{cls} text-sky-400" } };
    }
    let m = row.mime.as_str();
    let e = ext(&row.name);
    if m.starts_with("image/") || matches!(e.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "heic") {
        rsx! { icons::FileImage { class: "{cls} text-emerald-400" } }
    } else if m.starts_with("video/") || matches!(e.as_str(), "mp4" | "mov" | "mkv" | "avi" | "webm") {
        rsx! { icons::FileVideo { class: "{cls} text-violet-400" } }
    } else if m.starts_with("audio/") || matches!(e.as_str(), "mp3" | "wav" | "flac" | "m4a" | "ogg") {
        rsx! { icons::FileAudio { class: "{cls} text-pink-400" } }
    } else if matches!(e.as_str(), "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "zst" | "xz" | "bz2") {
        rsx! { icons::FileArchive { class: "{cls} text-amber-400" } }
    } else if matches!(e.as_str(), "rs" | "js" | "ts" | "py" | "sh" | "json" | "yml" | "yaml" | "toml" | "html" | "css" | "sql") {
        rsx! { icons::FileCode { class: "{cls} text-orange-400" } }
    } else if m.starts_with("text/") || matches!(e.as_str(), "txt" | "md" | "pdf" | "log" | "csv" | "doc" | "docx") {
        rsx! { icons::FileText { class: "{cls} text-muted-foreground" } }
    } else {
        rsx! { icons::File { class: "{cls} text-muted-foreground" } }
    }
}

async fn load(side: Side, path: String) -> Res<Listing> {
    match side {
        Side::Filen => {
            let d = api::filen_browse(&path).await?;
            let rows = d
                .entries
                .iter()
                .map(|e| Row {
                    key: e.uuid.clone(),
                    name: e.name.clone(),
                    path: join(&d.path, &e.name),
                    is_dir: e.is_dir,
                    size: e.size,
                    modified: e.modified,
                    mime: e.mime.clone(),
                })
                .collect();
            Ok(Listing {
                path: d.path,
                parent: d.parent,
                folder: d.uuid,
                rows,
            })
        }
        Side::Umbrel => {
            let d = api::umbrel_browse(&path).await?;
            let rows = d
                .entries
                .iter()
                .map(|e| Row {
                    key: e.path.clone(),
                    name: e.name.clone(),
                    path: e.path.clone(),
                    is_dir: e.is_dir,
                    size: e.size,
                    modified: e.modified,
                    mime: String::new(),
                })
                .collect();
            Ok(Listing {
                path: d.path,
                parent: d.parent,
                folder: String::new(),
                rows,
            })
        }
    }
}

#[component]
pub fn Explorer() -> Element {
    let mut side = use_signal(|| Side::Filen);
    let mut filen_path = use_signal(|| None::<String>);
    let mut umbrel_path = use_signal(|| "/".to_string());
    let mut filter = use_signal(String::new);
    let mut sort = use_signal(|| (Sort::Name, true));
    let mut tick = use_signal(|| 0u32);

    // Filen starts in the backup root folder when it exists, otherwise at the drive root.
    use_future(move || async move {
        let root = api::settings()
            .await
            .map(|s| s.root_folder)
            .unwrap_or_default();
        let start = match api::filen_browse("/").await {
            Ok(d) if !root.is_empty() => d
                .entries
                .iter()
                .find(|e| e.is_dir && e.name.eq_ignore_ascii_case(&root))
                .map(|e| format!("/{}", e.name))
                .unwrap_or_else(|| "/".into()),
            _ => "/".into(),
        };
        if filen_path.peek().is_none() {
            filen_path.set(Some(start));
        }
    });

    let current = move || match side() {
        Side::Filen => filen_path().unwrap_or_else(|| "/".into()),
        Side::Umbrel => umbrel_path(),
    };

    let listing = use_resource(move || {
        let _ = tick();
        let s = side();
        let ready = s == Side::Umbrel || filen_path().is_some();
        let p = current();
        async move {
            if !ready {
                return None;
            }
            Some(load(s, p).await)
        }
    });

    let mut go = move |p: String| {
        filter.set(String::new());
        match side() {
            Side::Filen => filen_path.set(Some(p)),
            Side::Umbrel => umbrel_path.set(p),
        }
    };

    let path = current();
    let crumbs: Vec<(String, String)> = {
        let mut acc = String::new();
        let mut v = vec![(
            match side() {
                Side::Filen => "Filen".to_string(),
                Side::Umbrel => "umbrelOS".to_string(),
            },
            "/".to_string(),
        )];
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            acc = format!("{acc}/{seg}");
            v.push((seg.to_string(), acc.clone()));
        }
        v
    };

    let tab = |s: Side| {
        if side() == s {
            "bg-background text-foreground shadow-sm"
        } else {
            "text-muted-foreground hover:text-foreground"
        }
    };

    let sort_btn = move |s: Sort, label: &'static str, extra: &'static str| {
        let (cur, asc) = sort();
        let arrow = if cur == s { if asc { " ↑" } else { " ↓" } } else { "" };
        rsx! {
            button {
                class: "text-left font-medium hover:text-foreground {extra}",
                onclick: move |_| {
                    let (cur, asc) = *sort.peek();
                    sort.set((s, if cur == s { !asc } else { s == Sort::Name }));
                },
                "{label}{arrow}"
            }
        }
    };

    let body = match &*listing.read_unchecked() {
        None | Some(None) => rsx! { div { class: "p-6", Loading {} } },
        Some(Some(Err(e))) => {
            let hint = if side() == Side::Umbrel {
                "Ist nrzBackup auf dem neuesten Stand?"
            } else {
                "Ist Filen in den Einstellungen verbunden?"
            };
            rsx! {
                div { class: "grid gap-3 p-4",
                    Callout { text: e.clone(), error: true }
                    div { class: "flex flex-wrap items-center gap-2 text-sm text-muted-foreground",
                        "{hint}"
                        Link { to: Route::Settings {}, class: "underline underline-offset-4 hover:text-foreground", "Zu den Einstellungen" }
                        if path != "/" {
                            button { class: "{btn(BTN_OUTLINE)} {BTN_SM}", onclick: move |_| go("/".into()), "Zum Anfang" }
                        }
                    }
                }
            }
        }
        Some(Some(Ok(l))) => {
            let q = filter().to_lowercase();
            let mut rows: Vec<Row> = l
                .rows
                .iter()
                .filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q))
                .cloned()
                .collect();
            let (s, asc) = sort();
            rows.sort_by(|a, b| {
                let o = match s {
                    Sort::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                    Sort::Size => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
                    Sort::Date => a.modified.unwrap_or(0).cmp(&b.modified.unwrap_or(0)),
                };
                let o = if asc { o } else { o.reverse() };
                b.is_dir.cmp(&a.is_dir).then(o)
            });
            let dirs = l.rows.iter().filter(|r| r.is_dir).count();
            let files = l.rows.len() - dirs;
            let total: u64 = l.rows.iter().filter_map(|r| r.size).sum();
            let folder = l.folder.clone();
            let sd = side();
            let summary = format!(
                "{dirs} Ordner · {files} Dateien{}",
                if total > 0 { format!(" · {}", format_bytes(total)) } else { String::new() }
            );
            rsx! {
                if rows.is_empty() {
                    div { class: "p-8 text-center text-sm text-muted-foreground",
                        if q.is_empty() { "Dieser Ordner ist leer." } else { "Nichts passt zum Filter." }
                    }
                }
                div { class: "divide-y",
                    for r in rows {
                        {
                            let r2 = r.clone();
                            let open = r.path.clone();
                            let dl = match sd {
                                Side::Filen if !r.is_dir => Some(api::filen_download_url(&folder, &r.key, false)),
                                Side::Filen => None,
                                Side::Umbrel => Some(api::umbrel_download_url(&r.path)),
                            };
                            let view = (sd == Side::Filen && !r.is_dir && previewable(&r))
                                .then(|| api::filen_download_url(&folder, &r.key, true));
                            let size = r.size.map(format_bytes).unwrap_or_default();
                            let date = r.modified.map(|m| fmt_time(m / 1000)).unwrap_or_default();
                            let dl_title = if r.is_dir { "Als Archiv herunterladen" } else { "Herunterladen" };
                            rsx! {
                                div { key: "{r.key}", class: "group flex items-center gap-3 px-4 py-2 text-sm hover:bg-accent/40",
                                    if r.is_dir {
                                        button {
                                            class: "flex min-w-0 flex-1 items-center gap-3 text-left",
                                            onclick: move |_| go(open.clone()),
                                            RowIcon { row: r2 }
                                            span { class: "truncate font-medium", "{r.name}" }
                                        }
                                    } else {
                                        div { class: "flex min-w-0 flex-1 items-center gap-3",
                                            RowIcon { row: r2 }
                                            if let Some(v) = view.clone() {
                                                a { class: "truncate hover:underline underline-offset-4", href: "{v}", target: "_blank", rel: "noopener", "{r.name}" }
                                            } else {
                                                span { class: "truncate", "{r.name}" }
                                            }
                                        }
                                    }
                                    span { class: "hidden w-24 shrink-0 text-right tabular-nums text-muted-foreground sm:block", "{size}" }
                                    span { class: "hidden w-36 shrink-0 tabular-nums text-muted-foreground md:block", "{date}" }
                                    div { class: "flex w-20 shrink-0 justify-end gap-1",
                                        if let Some(v) = view {
                                            a { class: "{btn(BTN_GHOST)} {BTN_ICON} size-8", href: "{v}", target: "_blank", rel: "noopener", title: "Im Browser öffnen",
                                                icons::Eye { class: "size-4" }
                                            }
                                        }
                                        if let Some(d) = dl {
                                            a { class: "{btn(BTN_GHOST)} {BTN_ICON} size-8", href: "{d}", download: "", title: "{dl_title}",
                                                icons::Download { class: "size-4" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "border-t px-4 py-2 text-xs text-muted-foreground", "{summary}" }
            }
        }
    };

    let up = parent_of(&path);
    let at_root = path == "/";
    rsx! {
        PageHeader {
            title: "Dateien",
            description: "Durchsuche Filen.io und umbrelOS und lade Dateien herunter. Filen-Dateien werden dabei entschlüsselt, umbrelOS-Ordner kommen als Archiv.",
            actions: rsx! {
                button { class: "{btn(BTN_OUTLINE)}", onclick: move |_| tick += 1,
                    icons::RefreshCw {}
                    "Aktualisieren"
                }
            },
        }
        div { class: "mb-4 inline-flex rounded-lg bg-muted p-1 text-sm",
            button { class: "inline-flex items-center gap-2 rounded-md px-3 py-1.5 font-medium {tab(Side::Filen)}",
                onclick: move |_| { filter.set(String::new()); side.set(Side::Filen) },
                icons::Cloud { class: "size-4" } "Filen.io"
            }
            button { class: "inline-flex items-center gap-2 rounded-md px-3 py-1.5 font-medium {tab(Side::Umbrel)}",
                onclick: move |_| { filter.set(String::new()); side.set(Side::Umbrel) },
                icons::HardDrive { class: "size-4" } "umbrelOS"
            }
        }
        div { class: "overflow-hidden rounded-xl border bg-card shadow-sm",
            div { class: "flex flex-wrap items-center gap-2 border-b px-3 py-2",
                button { class: "{btn(BTN_GHOST)} {BTN_ICON} size-8", disabled: at_root, title: "Nach oben",
                    onclick: move |_| go(up.clone()),
                    icons::ArrowUp { class: "size-4" }
                }
                nav { class: "flex min-w-0 flex-1 flex-wrap items-center gap-0.5 text-sm",
                    for (i, (label, p)) in crumbs.iter().cloned().enumerate() {
                        if i > 0 { icons::ChevronRight { class: "size-3.5 text-muted-foreground" } }
                        button {
                            class: "max-w-48 truncate rounded px-1.5 py-0.5 hover:bg-accent",
                            class: if p == path { "font-medium text-foreground" } else { "text-muted-foreground" },
                            onclick: move |_| go(p.clone()),
                            if i == 0 {
                                span { class: "inline-flex items-center gap-1",
                                    icons::House { class: "size-3.5" }
                                    "{label}"
                                }
                            } else {
                                "{label}"
                            }
                        }
                    }
                }
                div { class: "relative w-full sm:w-56",
                    icons::Search { class: "pointer-events-none absolute left-2.5 top-2.5 size-4 text-muted-foreground" }
                    input {
                        class: "h-9 w-full rounded-md border bg-transparent pl-8 pr-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring dark:bg-input/30",
                        placeholder: "In diesem Ordner filtern",
                        value: "{filter}",
                        oninput: move |e| filter.set(e.value()),
                    }
                }
            }
            div { class: "flex items-center gap-3 border-b bg-muted/30 px-4 py-2 text-xs text-muted-foreground",
                div { class: "flex min-w-0 flex-1 gap-3", span { class: "w-4" } {sort_btn(Sort::Name, "Name", "")} }
                {sort_btn(Sort::Size, "Größe", "hidden w-24 shrink-0 text-right sm:block")}
                {sort_btn(Sort::Date, "Geändert", "hidden w-36 shrink-0 md:block")}
                span { class: "w-20 shrink-0" }
            }
            {body}
        }
    }
}
