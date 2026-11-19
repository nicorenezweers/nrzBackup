//! The master key dialog: shown any time after entering the app password, with copy,
//! print, saving in the password manager (Google, iCloud Keychain, …) and saving as a
//! file (iCloud Drive, Google Drive, … via the share sheet).

use dioxus::prelude::*;
use nrzbackup_shared as shared;

use super::common::*;
use super::overlay::*;
use crate::api;

const FILE_NAME: &str = "nrzBackup-Hauptschluessel.txt";

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Plain text that is printed / saved: key plus how to restore without nrzBackup.
fn key_text(k: &shared::MasterKeySecret) -> String {
    format!(
        "nrzBackup – Hauptschlüssel\n\
         Erstellt am {date}\n\n\
         GEHEIMER SCHLÜSSEL (sicher aufbewahren, niemandem zeigen):\n{secret}\n\n\
         Öffentlicher Teil (nur zum Verschlüsseln):\n{public}\n\n\
         Wiederherstellen ohne nrzBackup:\n\
         1. Den geheimen Schlüssel in eine Datei schreiben, z. B. hauptschluessel.txt\n\
         2. Das Backup aus Filen laden (Ordner /nrzBackup/<quelle>/)\n\
         3. Mit dem kostenlosen Programm age (age-encryption.org) entschlüsseln:\n   \
            age -d -i hauptschluessel.txt backup.tar.gz.age > backup.tar.gz\n",
        date = fmt_time(now()),
        secret = k.secret_key,
        public = k.public_key,
    )
}

/// Prints a sheet with the key (hidden iframe, no popup needed).
pub fn print_key(k: &shared::MasterKeySecret) {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    // the long key in groups of 8 so it can be typed off paper
    let grouped: String = k
        .secret_key
        .chars()
        .collect::<Vec<_>>()
        .chunks(8)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ");
    let html = format!(
        r#"<!doctype html><html lang="de"><head><meta charset="utf-8"><title>nrzBackup Hauptschlüssel</title>
<style>
body{{font-family:-apple-system,system-ui,Segoe UI,Roboto,sans-serif;color:#111;margin:32px;line-height:1.45}}
h1{{font-size:22px;margin:0 0 4px}} .muted{{color:#555;font-size:13px}}
.key{{font-family:ui-monospace,Menlo,Consolas,monospace;font-size:17px;word-break:break-all;border:2px solid #111;border-radius:8px;padding:16px;margin:16px 0;letter-spacing:.5px}}
.grp{{font-family:ui-monospace,Menlo,Consolas,monospace;font-size:14px;word-spacing:6px;color:#333}}
code{{font-family:ui-monospace,Menlo,Consolas,monospace;background:#f2f2f2;padding:2px 5px;border-radius:4px;font-size:12px}}
.warn{{border-left:4px solid #d97706;padding:6px 12px;background:#fff7ed;margin:16px 0}}
ol{{padding-left:20px}} li{{margin:4px 0}}
</style></head><body>
<h1>nrzBackup – Hauptschlüssel</h1>
<div class="muted">Ausgedruckt am {date}</div>
<div class="warn"><b>Streng geheim.</b> Wer diesen Schlüssel und Zugang zu deinem Filen-Konto hat, kann alle Backups lesen. Sicher aufbewahren (z. B. bei wichtigen Dokumenten).</div>
<div class="key">{secret}</div>
<div class="muted">Zum Abtippen in Gruppen zu 8 Zeichen:</div>
<p class="grp">{grouped}</p>
<p class="muted">Öffentlicher Teil (nur zum Verschlüsseln): <code>{public}</code></p>
<h2 style="font-size:16px">Wiederherstellen ohne nrzBackup</h2>
<ol>
<li>Den geheimen Schlüssel in eine Datei schreiben, z. B. <code>hauptschluessel.txt</code></li>
<li>Das Backup aus Filen laden (Ordner <code>/nrzBackup/&lt;quelle&gt;/</code>)</li>
<li>Mit dem kostenlosen Programm <b>age</b> (age-encryption.org) entschlüsseln:<br><code>age -d -i hauptschluessel.txt backup.tar.gz.age &gt; backup.tar.gz</code></li>
</ol>
</body></html>"#,
        date = esc(&fmt_time(now())),
        secret = esc(&k.secret_key),
        grouped = esc(&grouped),
        public = esc(&k.public_key),
    );
    document::eval(&format!(
        r#"(() => {{
            const f = document.createElement('iframe');
            f.style.cssText = 'position:fixed;right:0;bottom:0;width:0;height:0;border:0';
            f.srcdoc = {html};
            f.onload = () => {{
                f.contentWindow.focus();
                f.contentWindow.print();
                setTimeout(() => f.remove(), 60000);
            }};
            document.body.appendChild(f);
        }})()"#,
        html = js_str(&html)
    ));
}

/// Saves the key as a text file: share sheet where available (iPhone/iPad/Android → iCloud
/// Drive, Google Drive, Files, …), otherwise a normal download.
pub fn save_key_file(k: &shared::MasterKeySecret) {
    document::eval(&format!(
        r#"(async () => {{
            const text = {text}, name = {name};
            const file = new File([text], name, {{ type: 'text/plain' }});
            try {{
                if (navigator.canShare && navigator.canShare({{ files: [file] }})) {{
                    await navigator.share({{ files: [file], title: 'nrzBackup Hauptschlüssel' }});
                    return;
                }}
            }} catch (e) {{
                if (e && e.name === 'AbortError') return;
            }}
            const url = URL.createObjectURL(file);
            const a = document.createElement('a');
            a.href = url; a.download = name;
            document.body.appendChild(a); a.click(); a.remove();
            setTimeout(() => URL.revokeObjectURL(url), 10000);
        }})()"#,
        text = js_str(&key_text(k)),
        name = js_str(FILE_NAME),
    ));
}

/// Dialog: password → key with every way to keep it safe.
#[component]
pub fn MasterKeyDialog(
    open: Signal<bool>,
    confirmed: bool,
    on_change: EventHandler<()>,
) -> Element {
    let mut password = use_signal(String::new);
    let mut key = use_signal(|| None::<shared::MasterKeySecret>);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    // forget the key as soon as the dialog closes
    use_effect(move || {
        if !open() {
            key.set(None);
            password.set(String::new());
            error.set(None);
        }
    });

    let reveal = move |e: FormEvent| {
        e.prevent_default();
        if password().trim().is_empty() {
            error.set(Some("Bitte das App-Passwort eingeben.".into()));
            return;
        }
        busy.set(true);
        error.set(None);
        spawn(async move {
            match api::masterkey_reveal(&password()).await {
                Ok(k) => {
                    key.set(Some(k));
                    password.set(String::new());
                }
                Err(e) => error.set(Some(e)),
            }
            busy.set(false);
        });
    };
    let done = move |_| {
        spawn(async move {
            match api::masterkey_confirm().await {
                Ok(()) => {
                    toast("Danke – der Hauptschlüssel ist als gesichert vermerkt.");
                    open.set(false);
                    on_change.call(());
                }
                Err(e) => toast_error(e),
            }
        });
    };

    let host = web_sys::window()
        .and_then(|w| w.location().host().ok())
        .unwrap_or_default();

    rsx! {
        Modal {
            open,
            title: "Hauptschlüssel",
            description: "Mit diesem Schlüssel sind alle Backups verschlüsselt. Ohne ihn kommst du bei einem Defekt dieses Geräts nicht mehr an deine Backups.",
            if let Some(k) = key() {
                div { class: "grid gap-4",
                    div { class: "grid gap-2 rounded-lg border border-warning/40 bg-warning/5 p-3",
                        CodeBlock { text: k.secret_key.clone(), large: true }
                        p { class: "text-xs text-muted-foreground", "Achte darauf, dass niemand mitliest." }
                    }
                    div { class: "grid gap-2 sm:grid-cols-2",
                        CopyButton { text: k.secret_key.clone(), label: true }
                        button { class: "{btn(BTN_OUTLINE)}", onclick: { let k = k.clone(); move |_| print_key(&k) },
                            icons::Printer {}
                            "Ausdrucken"
                        }
                        button { class: "{btn(BTN_OUTLINE)}", onclick: { let k = k.clone(); move |_| save_key_file(&k) },
                            icons::Share {}
                            "In iCloud / Google Drive sichern"
                        }
                    }
                    // a real form submit: Safari (iCloud Keychain), Chrome (Google Password
                    // Manager), Edge, Firefox, 1Password … then offer to save the key
                    form {
                        class: "grid gap-2 rounded-lg border p-3",
                        method: "post",
                        action: "/api/masterkey/saved",
                        autocomplete: "on",
                        p { class: "text-sm font-medium", "Im Passwort-Manager sichern" }
                        p { class: "text-xs text-muted-foreground",
                            "Google Passwortmanager, iCloud-Schlüsselbund (Passwörter-App), 1Password … – nach dem Klick fragt dein Browser, ob er das Passwort speichern soll. Bitte bestätigen."
                        }
                        input {
                            class: "{INPUT}",
                            r#type: "text",
                            name: "username",
                            autocomplete: "username",
                            value: "nrzBackup Hauptschlüssel ({host})",
                        }
                        input {
                            class: "{INPUT}",
                            r#type: "password",
                            name: "password",
                            autocomplete: "new-password",
                            value: "{k.secret_key}",
                        }
                        button { class: "{btn(BTN_OUTLINE)}", r#type: "submit",
                            icons::KeyRound {}
                            "In Google / iCloud Passwörtern speichern"
                        }
                    }
                    div { class: "grid gap-1 text-xs text-muted-foreground",
                        p { "Wiederherstellen ohne nrzBackup mit dem kostenlosen Programm age (age-encryption.org):" }
                        code { class: "block rounded-md bg-muted px-2 py-1.5 font-mono text-[11px] text-foreground", "age -d -i hauptschluessel.txt backup.tar.gz.age > backup.tar.gz" }
                    }
                    div { class: "flex flex-wrap justify-end gap-2",
                        button { class: "{btn(BTN_GHOST)}", onclick: move |_| open.set(false), "Schließen" }
                        if !confirmed {
                            button { class: "{btn(BTN_PRIMARY)}", onclick: done,
                                icons::Check {}
                                "Ich habe ihn sicher abgelegt"
                            }
                        }
                    }
                }
            } else {
                form { class: "grid gap-4", onsubmit: reveal,
                    Field { label: "App-Passwort", hint: "Das Passwort, mit dem du dich bei nrzBackup anmeldest (umbrelOS → nrzBackup → Zugangsdaten).",
                        input {
                            class: "{INPUT}",
                            r#type: "password",
                            autocomplete: "current-password",
                            autofocus: true,
                            value: "{password}",
                            oninput: move |e| password.set(e.value()),
                        }
                    }
                    if let Some(e) = error() {
                        Callout { text: e, error: true }
                    }
                    div { class: "flex justify-end gap-2",
                        button { class: "{btn(BTN_GHOST)}", r#type: "button", onclick: move |_| open.set(false), "Abbrechen" }
                        button { class: "{btn(BTN_PRIMARY)}", r#type: "submit", disabled: busy(),
                            if busy() { icons::LoaderCircle { class: "animate-spin" } } else { icons::LockOpen {} }
                            "Schlüssel anzeigen"
                        }
                    }
                }
            }
        }
    }
}
