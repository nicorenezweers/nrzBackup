//! Frame like nrzBudget: login gate (`Shell`), sidebar on wide screens, tab bar at
//! the bottom on phones, plus the toasts and confirmations.

use dioxus::prelude::*;

use crate::Route;
use crate::api::{self, AUTH, VERSION};
use crate::components::common::*;
use crate::components::overlay::*;

const ICON: Asset = asset!("/assets/favicon.svg");

/// Decides between the login page and the app.
#[component]
pub fn Shell() -> Element {
    let mut failed = use_signal(|| None::<String>);

    use_future(move || async move {
        match api::session().await {
            Ok(s) => {
                *VERSION.write() = s.version;
                *AUTH.write() = Some(s.authenticated);
            }
            Err(e) => {
                if AUTH.peek().is_none() {
                    failed.set(Some(e));
                }
            }
        }
    });

    use_effect(|| {
        if let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
        {
            let _ = root.class_list().add_1("dark");
            let _ = root.set_attribute("lang", "de");
        }
    });

    let body = match AUTH() {
        Some(true) => rsx! { AppLayout {} },
        Some(false) => rsx! { super::auth::Login {} },
        None => match failed() {
            Some(e) => rsx! {
                div { class: "grid h-dvh place-items-center p-6",
                    div { class: "grid max-w-sm gap-3",
                        Callout { text: e, error: true }
                        button {
                            class: "{btn(BTN_OUTLINE)}",
                            onclick: move |_| {
                                if let Some(w) = web_sys::window() {
                                    let _ = w.location().reload();
                                }
                            },
                            icons::RefreshCw {}
                            "Erneut versuchen"
                        }
                    }
                }
            },
            None => rsx! { div { class: "grid h-dvh place-items-center", Loading {} } },
        },
    };
    rsx! {
        {body}
        ConfirmHost {}
        Toaster {}
    }
}

fn logout() {
    spawn(async move {
        let ask = Ask::new(
            "Abmelden?",
            "Du kannst dich jederzeit wieder mit dem App-Passwort anmelden.",
        )
        .confirm("Abmelden");
        if !confirm(ask).await {
            return;
        }
        match api::logout().await {
            Ok(()) => *AUTH.write() = Some(false),
            Err(e) => toast_error(e),
        }
    });
}

#[derive(Clone, Copy, PartialEq)]
enum Nav {
    Overview,
    Sources,
    Files,
    Events,
    Settings,
}

fn active(route: &Route) -> Nav {
    match route {
        Route::Sources {} | Route::SourceView { .. } => Nav::Sources,
        Route::Explorer {} => Nav::Files,
        Route::Events {} => Nav::Events,
        Route::Settings {} => Nav::Settings,
        _ => Nav::Overview,
    }
}

const ITEMS: [(Nav, &str); 5] = [
    (Nav::Overview, "Übersicht"),
    (Nav::Sources, "Quellen"),
    (Nav::Files, "Dateien"),
    (Nav::Events, "Protokoll"),
    (Nav::Settings, "Einstellungen"),
];

fn target(n: Nav) -> Route {
    match n {
        Nav::Overview => Route::Overview {},
        Nav::Sources => Route::Sources {},
        Nav::Files => Route::Explorer {},
        Nav::Events => Route::Events {},
        Nav::Settings => Route::Settings {},
    }
}

#[component]
fn NavIcon(n: Nav) -> Element {
    match n {
        Nav::Overview => rsx! { icons::LayoutDashboard {} },
        Nav::Sources => rsx! { icons::DatabaseBackup {} },
        Nav::Files => rsx! { icons::FolderOpen {} },
        Nav::Events => rsx! { icons::ScrollText {} },
        Nav::Settings => rsx! { icons::Settings {} },
    }
}

/// The app for a signed-in user.
#[component]
fn AppLayout() -> Element {
    rsx! {
        div { class: "flex h-dvh overflow-hidden",
            aside { class: "hidden w-60 shrink-0 border-r border-sidenav-border bg-sidenav md:flex md:flex-col",
                Sidebar {}
            }
            div { class: "flex min-w-0 flex-1 flex-col",
                MobileHeader {}
                main { class: "min-h-0 flex-1 overflow-y-auto overscroll-contain",
                    div { class: "mx-auto w-full max-w-6xl px-safe py-5 md:px-8 md:py-8",
                        Outlet::<Route> {}
                    }
                }
                TabBar {}
            }
        }
    }
}

#[component]
fn Sidebar() -> Element {
    let route = use_route::<Route>();
    let current = active(&route);
    let version = VERSION();
    rsx! {
        div { class: "flex h-14 items-center gap-2.5 px-5",
            img { src: ICON, class: "size-7 rounded-lg", alt: "" }
            span { class: "font-semibold tracking-tight", "nrzBackup" }
        }
        nav { class: "mt-2 flex flex-col gap-0.5 px-3",
            for (n, label) in ITEMS {
                Link {
                    to: target(n),
                    class: if n == current {
                        "flex items-center gap-2.5 rounded-md bg-sidenav-accent px-2.5 py-1.5 text-sm font-medium text-sidenav-accent-foreground [&_svg]:size-4 [&_svg]:shrink-0"
                    } else {
                        "flex items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm text-muted-foreground transition-colors hover:bg-sidenav-accent/60 hover:text-sidenav-accent-foreground [&_svg]:size-4 [&_svg]:shrink-0"
                    },
                    NavIcon { n }
                    span { "{label}" }
                }
            }
        }
        div { class: "flex-1" }
        div { class: "border-t border-sidenav-border p-3",
            button {
                class: "flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm text-muted-foreground hover:bg-sidenav-accent/60 hover:text-sidenav-accent-foreground",
                onclick: move |_| logout(),
                icons::LogOut { class: "size-4" }
                "Abmelden"
            }
            if !version.is_empty() {
                p { class: "mt-2 px-2.5 text-[11px] text-muted-foreground tabular-nums", "Version {version}" }
            }
        }
    }
}

/// Phones: app name and sign out at the top.
#[component]
fn MobileHeader() -> Element {
    rsx! {
        header { class: "shrink-0 border-b bg-background pt-safe md:hidden",
            div { class: "flex h-12 items-center gap-2 px-safe",
                img { src: ICON, class: "size-6 rounded-md", alt: "" }
                span { class: "flex-1 font-semibold", "nrzBackup" }
                button {
                    class: "{btn(BTN_GHOST)} {BTN_ICON} text-muted-foreground",
                    title: "Abmelden",
                    onclick: move |_| logout(),
                    icons::LogOut { class: "size-4" }
                }
            }
        }
    }
}

/// Phones: navigation at the bottom, in reach of the thumb.
#[component]
fn TabBar() -> Element {
    let route = use_route::<Route>();
    let current = active(&route);
    rsx! {
        nav { class: "flex shrink-0 border-t border-sidenav-border bg-sidenav pb-safe px-safe select-none md:hidden",
            for (n, label) in ITEMS {
                Link {
                    to: target(n),
                    class: if n == current {
                        "flex flex-1 flex-col items-center gap-1 pt-2 pb-1.5 text-[10.5px] font-medium text-foreground [&_svg]:size-[22px]"
                    } else {
                        "flex flex-1 flex-col items-center gap-1 pt-2 pb-1.5 text-[10.5px] font-medium text-muted-foreground active:text-foreground [&_svg]:size-[22px]"
                    },
                    NavIcon { n }
                    span { "{label}" }
                }
            }
        }
    }
}
