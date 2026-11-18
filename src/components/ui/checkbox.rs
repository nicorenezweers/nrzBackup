use dioxus::prelude::*;
use icons::Check;
use tw_merge::tw_merge;

/// rust-ui `Checkbox` with a label and optional hint below it.
#[component]
pub fn Checkbox(
    #[props(into, optional)] class: Option<String>,
    checked: bool,
    #[props(into)] label: String,
    #[props(into, optional)] sub_label: Option<String>,
    #[props(optional)] disabled: bool,
    onchange: EventHandler<bool>,
) -> Element {
    let box_class = tw_merge!(
        "peer mt-0.5 size-4 shrink-0 rounded-[4px] border border-input shadow-xs transition-shadow outline-none flex items-center justify-center",
        if checked {
            "bg-primary text-primary-foreground border-primary"
        } else {
            "bg-[var(--input-field-background)]"
        }
    );
    let merged = tw_merge!("check", class.as_deref().unwrap_or(""));
    rsx! {
        button {
            "data-name": "Checkbox",
            r#type: "button",
            role: "checkbox",
            "aria-checked": "{checked}",
            class: "{merged} text-left bg-transparent border-0 p-0",
            disabled,
            onclick: move |_| onchange.call(!checked),
            span { class: "{box_class}",
                if checked {
                    Check { class: "size-3" }
                }
            }
            span {
                "{label}"
                if let Some(sub) = sub_label {
                    small { "{sub}" }
                }
            }
        }
    }
}
