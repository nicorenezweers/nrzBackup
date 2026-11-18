//! Log of what happened (uploads, pulls, retention, errors).

use dioxus::prelude::*;
use nrzbackup_shared::EventLevel;

use crate::Route;
use crate::api;
use crate::components::common::*;

#[component]
pub fn Events() -> Element {
    let mut list = use_loader(|| api::events(200));
    use_poll(list, 30);

    rsx! {
        PageHeader {
            title: "Protokoll",
            description: "Die letzten 200 Ereignisse.",
            actions: rsx! {
                button { class: "{btn(BTN_OUTLINE)}", onclick: move |_| list.reload(),
                    icons::RefreshCw {}
                    "Aktualisieren"
                }
            },
        }
        if let Some(e) = list.error.read().clone() {
            div { class: "mb-4", Callout { text: e, error: true } }
        }
        match list.data.read().clone() {
            None => rsx! { Loading {} },
            Some(items) if items.is_empty() => rsx! {
                EmptyState { title: "Noch keine Einträge", icon: rsx! { icons::ScrollText {} } }
            },
            Some(items) => rsx! {
                div { class: "divide-y overflow-hidden rounded-xl border bg-card shadow-sm",
                    for ev in items {
                        div { key: "{ev.id}", class: "flex flex-wrap items-start gap-x-4 gap-y-1 px-4 py-2.5 text-sm sm:flex-nowrap sm:px-5",
                            span { class: "w-36 shrink-0 tabular-nums text-muted-foreground", title: fmt_relative(ev.ts), {fmt_time(ev.ts)} }
                            span { class: "w-16 shrink-0",
                                match ev.level {
                                    EventLevel::Info => rsx! { Tag { text: "Info" } },
                                    EventLevel::Warn => rsx! { Tag { text: "Warnung", tone: Tone::Warning } },
                                    EventLevel::Error => rsx! { Tag { text: "Fehler", tone: Tone::Danger } },
                                }
                            }
                            span { class: "w-36 shrink-0 truncate text-muted-foreground",
                                match (ev.source_id, ev.source_name.clone()) {
                                    (Some(id), Some(name)) => rsx! {
                                        Link { to: Route::SourceView { id }, class: "hover:text-foreground hover:underline underline-offset-4", "{name}" }
                                    },
                                    (_, Some(name)) => rsx! { "{name}" },
                                    _ => rsx! { "System" },
                                }
                            }
                            span { class: "min-w-0 flex-1 break-words max-sm:basis-full", "{ev.message}" }
                        }
                    }
                }
            },
        }
    }
}
