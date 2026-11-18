//! Login: user `admin` and the app password shown in umbrelOS.

use dioxus::prelude::*;

use crate::api::{self, AUTH, VERSION};
use crate::components::common::*;

const ICON: Asset = asset!("/assets/favicon.svg");

#[component]
pub fn Login() -> Element {
    let mut username = use_signal(|| "admin".to_string());
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut busy = use_signal(|| false);

    let submit = move |e: FormEvent| {
        e.prevent_default();
        if password().trim().is_empty() {
            error.set(Some("Bitte das Passwort eingeben.".into()));
            return;
        }
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::login(username().trim(), password().trim()).await {
                Ok(s) => {
                    *VERSION.write() = s.version;
                    *AUTH.write() = Some(true);
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "flex h-dvh overflow-y-auto bg-background p-4 pt-[max(1rem,env(safe-area-inset-top))] pb-safe-6",
            div { class: "m-auto w-full max-w-sm",
                div { class: "mb-6 flex flex-col items-center gap-3 text-center",
                    img { src: ICON, class: "size-12 rounded-xl", alt: "" }
                    div {
                        h1 { class: "text-xl font-semibold tracking-tight", "nrzBackup" }
                        p { class: "mt-1 text-sm text-muted-foreground", "Verschlüsselte Backups nach Filen.io" }
                    }
                }
                div { class: "rounded-xl border bg-card p-6 shadow-sm",
                    form { class: "grid gap-4", onsubmit: submit,
                        if let Some(e) = error() {
                            Callout { text: e, error: true }
                        }
                        Field { label: "Benutzer",
                            input {
                                class: INPUT,
                                r#type: "text",
                                name: "username",
                                autocomplete: "username",
                                autocapitalize: "none",
                                spellcheck: "false",
                                value: "{username}",
                                oninput: move |e| username.set(e.value()),
                            }
                        }
                        Field { label: "Passwort", hint: "Steht in umbrelOS bei der App (Zugangsdaten). Ein im Browser gespeichertes altes Passwort bitte ersetzen.",
                            input {
                                class: INPUT,
                                r#type: "password",
                                name: "password",
                                autocomplete: "current-password",
                                autofocus: true,
                                value: "{password}",
                                oninput: move |e| password.set(e.value()),
                            }
                        }
                        button { class: "{btn(BTN_PRIMARY)} w-full", r#type: "submit", disabled: busy(),
                            if busy() {
                                icons::LoaderCircle { class: "animate-spin" }
                            }
                            "Anmelden"
                        }
                    }
                }
            }
        }
    }
}
