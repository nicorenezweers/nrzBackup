//! Dialog, confirmation and toasts in the look of rust-ui (Dialog, Alert Dialog,
//! Toast), taken over from nrzBudget. Replaces the browser popups
//! (`confirm`, `alert`, `prompt`).

use dioxus::prelude::*;
use futures_channel::oneshot;
use std::cell::RefCell;
use std::rc::Rc;
use tw_merge::tw_merge;

// ------------------------------------------------------------------ Dialog

/// Modales Fenster. Schliesst mit Esc, Klick daneben (abschaltbar) oder dem X.
#[component]
pub fn Modal(
    open: Signal<bool>,
    #[props(into)] title: String,
    #[props(into, optional)] description: Option<String>,
    #[props(into, optional)] class: Option<String>,
    #[props(default = true)] dismissible: bool,
    children: Element,
) -> Element {
    if !open() {
        return rsx! {};
    }
    let content = tw_merge!(
        "relative bg-background border shadow-lg p-6 w-full max-w-lg overflow-y-auto overscroll-contain rounded-t-2xl border-b-0 max-h-[92dvh] pb-safe-6 animate-sheet-in sm:rounded-2xl sm:border-b sm:max-h-[85vh] sm:pb-6 sm:animate-in",
        class.as_deref().unwrap_or("")
    );
    rsx! {
        div {
            class: "fixed inset-0 z-[60] flex items-end justify-center bg-black/60 backdrop-blur-[2px] sm:items-center sm:p-4",
            tabindex: "-1",
            // Erstes Eingabefeld des Dialogs fokussieren, sonst die Fläche selbst (für Escape)
            onmounted: move |_| {
                document::eval(
                    r#"requestAnimationFrame(() => {
                        const d = [...document.querySelectorAll('[role=dialog]')].pop();
                        if (!d) return;
                        if (matchMedia('(max-width: 767px)').matches) { d.parentElement.focus(); return; }
                        const f = d.querySelector('[autofocus], input:not([type=hidden]):not([readonly]):not([disabled]), textarea:not([readonly]), select:not([disabled])');
                        (f || d.parentElement).focus();
                    })"#,
                );
            },
            onkeydown: move |e| if e.key() == Key::Escape && dismissible { open.set(false) },
            onclick: move |_| if dismissible { open.set(false) },
            div {
                class: "{content}",
                role: "dialog",
                "aria-modal": "true",
                onclick: move |e| e.stop_propagation(),
                // grabber like an iOS sheet
                div { class: "mx-auto -mt-3 mb-3 h-1 w-9 rounded-full bg-muted-foreground/30 sm:hidden" }
                button {
                    class: "absolute top-4 right-4 rounded-sm p-1 text-muted-foreground hover:text-foreground focus:outline-none focus:ring-2 focus:ring-ring",
                    title: "Schließen",
                    onclick: move |_| open.set(false),
                    icons::X { class: "size-4" }
                }
                div { class: "flex flex-col gap-2 pr-6",
                    h3 { class: "text-lg leading-none font-semibold", "{title}" }
                    if let Some(d) = description {
                        p { class: "text-muted-foreground text-sm", "{d}" }
                    }
                }
                div { class: "mt-5", {children} }
            }
        }
    }
}

// ------------------------------------------------------------------ Bestätigung

#[derive(Clone)]
pub struct Ask {
    pub title: String,
    pub description: String,
    pub confirm: String,
    pub destructive: bool,
}

impl Ask {
    pub fn new(title: impl Into<String>, description: impl Into<String>) -> Self {
        Ask {
            title: title.into(),
            description: description.into(),
            confirm: "Bestätigen".to_string(),
            destructive: false,
        }
    }
    pub fn confirm(mut self, label: impl Into<String>) -> Self {
        self.confirm = label.into();
        self
    }
    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }
}

type Pending = Rc<RefCell<Option<oneshot::Sender<bool>>>>;

static ASK: GlobalSignal<Option<(Ask, Pending)>> = Signal::global(|| None);

/// Eigene Rückfrage statt `window.confirm` – `true`, wenn bestätigt.
pub async fn confirm(ask: Ask) -> bool {
    let (tx, rx) = oneshot::channel();
    if let Some((_, old)) = ASK.write().replace((ask, Rc::new(RefCell::new(Some(tx)))))
        && let Some(s) = old.borrow_mut().take()
    {
        let _ = s.send(false);
    }
    rx.await.unwrap_or(false)
}

fn answer(yes: bool) {
    if let Some((_, pending)) = ASK.write().take()
        && let Some(s) = pending.borrow_mut().take()
    {
        let _ = s.send(yes);
    }
}

/// Einmal in der App einbinden.
#[component]
pub fn ConfirmHost() -> Element {
    let Some((ask, _)) = ASK.read().clone() else {
        return rsx! {};
    };
    let action = if ask.destructive {
        "bg-destructive text-white hover:bg-destructive/90"
    } else {
        "bg-primary text-primary-foreground hover:bg-primary/90"
    };
    rsx! {
        div {
            class: "fixed inset-0 z-[70] flex items-end justify-center bg-black/60 backdrop-blur-[2px] sm:items-center sm:p-4",
            tabindex: "-1",
            onmounted: move |e| async move { let _ = e.set_focus(true).await; },
            onkeydown: move |e| match e.key() {
                Key::Escape => answer(false),
                Key::Enter => answer(true),
                _ => {}
            },
            div { class: "w-full max-w-md rounded-t-2xl border border-b-0 bg-background p-6 pb-safe-6 shadow-lg animate-sheet-in sm:rounded-2xl sm:border-b sm:pb-6 sm:animate-in", role: "alertdialog",
                div { class: "flex gap-4",
                    if ask.destructive {
                        div { class: "grid size-10 shrink-0 place-items-center rounded-full bg-destructive/15 text-destructive",
                            icons::TriangleAlert { class: "size-5" }
                        }
                    }
                    div { class: "flex flex-col gap-2",
                        h3 { class: "text-lg leading-none font-semibold", "{ask.title}" }
                        p { class: "text-muted-foreground text-sm", "{ask.description}" }
                    }
                }
                footer { class: "mt-6 flex flex-col-reverse gap-2 sm:flex-row sm:justify-end",
                    button {
                        class: "inline-flex h-11 items-center justify-center rounded-lg border bg-background px-4 text-sm font-medium shadow-xs hover:bg-accent dark:bg-input/30 dark:border-input sm:h-9 sm:rounded-md",
                        onclick: move |_| answer(false),
                        "Abbrechen"
                    }
                    button {
                        class: "inline-flex h-11 items-center justify-center rounded-lg px-4 text-sm font-medium shadow-xs sm:h-9 sm:rounded-md {action}",
                        onclick: move |_| answer(true),
                        "{ask.confirm}"
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------ Meldungen

#[derive(Clone, PartialEq)]
struct Toast {
    id: u64,
    text: String,
    error: bool,
    /// Wird gerade ausgeblendet (Animation läuft, danach entfernt)
    leaving: bool,
}

static TOASTS: GlobalSignal<Vec<Toast>> = Signal::global(Vec::new);
static NEXT: GlobalSignal<u64> = Signal::global(|| 0);

/// Dauer der Ausblende-Animation (muss zu `toast-out` in tailwind.css passen).
const LEAVE_MS: u32 = 220;

fn push(text: String, error: bool) {
    let id = {
        let mut n = NEXT.write();
        *n += 1;
        *n
    };
    TOASTS.write().push(Toast {
        id,
        text,
        error,
        leaving: false,
    });
    spawn(async move {
        gloo_timers::future::TimeoutFuture::new(if error { 7000 } else { 3500 }).await;
        dismiss(id);
    });
}

/// Meldung sanft ausblenden und danach entfernen.
fn dismiss(id: u64) {
    let mut found = false;
    for t in TOASTS
        .write()
        .iter_mut()
        .filter(|t| t.id == id && !t.leaving)
    {
        t.leaving = true;
        found = true;
    }
    if found {
        spawn(async move {
            gloo_timers::future::TimeoutFuture::new(LEAVE_MS).await;
            TOASTS.write().retain(|t| t.id != id);
        });
    }
}

/// Kurze Meldung unten rechts.
pub fn toast(text: impl Into<String>) {
    push(text.into(), false);
}

/// Fehlermeldung (bleibt länger stehen).
pub fn toast_error(text: impl std::fmt::Display) {
    push(text.to_string(), true);
}

#[component]
pub fn Toaster() -> Element {
    rsx! {
        div { class: "pointer-events-none fixed inset-x-3 top-[calc(env(safe-area-inset-top)+0.75rem)] z-[80] flex flex-col gap-2 sm:inset-x-auto sm:top-auto sm:right-4 sm:bottom-20 sm:w-full sm:max-w-sm",
            for t in TOASTS.read().iter().cloned() {
                div {
                    key: "{t.id}",
                    role: if t.error { "alert" } else { "status" },
                    class: format!(
                        "pointer-events-auto flex items-start gap-3 rounded-xl border bg-popover px-4 py-3 text-sm text-popover-foreground shadow-lg {} {}",
                        if t.error { "border-destructive/40" } else { "" },
                        if t.leaving { "animate-toast-out" } else { "animate-toast-in" },
                    ),
                    if t.error {
                        icons::CircleAlert { class: "mt-0.5 size-4 shrink-0 text-destructive" }
                    } else {
                        icons::Check { class: "mt-0.5 size-4 shrink-0 text-muted-foreground" }
                    }
                    span { class: "flex-1", "{t.text}" }
                    button {
                        class: "text-muted-foreground hover:text-foreground",
                        onclick: move |_| dismiss(t.id),
                        icons::X { class: "size-3.5" }
                    }
                }
            }
        }
    }
}
