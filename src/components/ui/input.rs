use dioxus::prelude::*;
use tw_merge::tw_merge;

/// rust-ui `Input`.
#[component]
pub fn Input(
    #[props(into, optional)] class: Option<String>,
    #[props(into, optional)] id: Option<String>,
    #[props(into, default = "text".to_string())] r#type: String,
    #[props(into, optional)] placeholder: Option<String>,
    #[props(into, optional)] autocomplete: Option<String>,
    #[props(optional)] autofocus: bool,
    #[props(optional)] disabled: bool,
    #[props(into)] value: String,
    #[props(optional)] oninput: Option<EventHandler<String>>,
) -> Element {
    let merged_class = tw_merge!(
        "file:text-foreground placeholder:text-muted-foreground selection:bg-primary selection:text-primary-foreground bg-[var(--input-field-background)] border-input flex h-9 w-full min-w-0 rounded-md border px-3 py-1 text-sm shadow-xs transition-[color,box-shadow] outline-none disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px]",
        class.as_deref().unwrap_or("")
    );
    rsx! {
        input {
            "data-name": "Input",
            class: "{merged_class}",
            id: id.as_deref(),
            r#type: "{r#type}",
            placeholder: placeholder.as_deref(),
            autocomplete: autocomplete.as_deref(),
            autofocus,
            disabled,
            value: "{value}",
            oninput: move |e| {
                if let Some(h) = &oninput {
                    h.call(e.value());
                }
            },
        }
    }
}
