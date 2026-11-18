use dioxus::prelude::*;
use tw_merge::tw_merge;

/// rust-ui `Switch`.
#[component]
pub fn Switch(
    #[props(into, optional)] class: Option<String>,
    checked: bool,
    #[props(optional)] disabled: bool,
    onchange: EventHandler<bool>,
) -> Element {
    let track = tw_merge!(
        "peer inline-flex h-[1.15rem] w-8 shrink-0 items-center rounded-full border border-transparent shadow-xs transition-all outline-none disabled:cursor-not-allowed disabled:opacity-50 cursor-pointer",
        if checked { "bg-primary" } else { "bg-input" },
        class.as_deref().unwrap_or("")
    );
    let thumb = tw_merge!(
        "bg-background pointer-events-none block size-4 rounded-full ring-0 transition-transform",
        if checked {
            "translate-x-[calc(100%-2px)]"
        } else {
            "translate-x-0"
        }
    );
    rsx! {
        button {
            "data-name": "Switch",
            r#type: "button",
            role: "switch",
            "aria-checked": "{checked}",
            class: "{track}",
            disabled,
            onclick: move |_| onchange.call(!checked),
            span { class: "{thumb}" }
        }
    }
}
