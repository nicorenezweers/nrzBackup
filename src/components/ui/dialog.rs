use dioxus::prelude::*;
use icons::X;

/// rust-ui `Dialog`: modal with title and close button; closes on backdrop click.
#[component]
pub fn Dialog(
    #[props(into)] title: String,
    #[props(optional)] wide: bool,
    onclose: EventHandler<()>,
    children: Element,
) -> Element {
    rsx! {
        div {
            class: "dialog-backdrop",
            onclick: move |_| onclose.call(()),
            onkeydown: move |e| {
                if e.key() == Key::Escape {
                    onclose.call(());
                }
            },
            div {
                "data-name": "Dialog",
                class: if wide { "dialog wide" } else { "dialog" },
                role: "dialog",
                onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { "{title}" }
                    button { class: "icon-btn", r#type: "button", onclick: move |_| onclose.call(()), X {} }
                }
                div { class: "dialog-body", {children} }
            }
        }
    }
}
