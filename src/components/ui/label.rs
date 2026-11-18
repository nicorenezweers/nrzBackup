use dioxus::prelude::*;
use tw_merge::tw_merge;

/// rust-ui `Label`: a caption above a form control.
#[component]
pub fn Label(
    #[props(into, optional)] class: Option<String>,
    #[props(into)] text: String,
    children: Element,
) -> Element {
    let merged_class = tw_merge!(
        "flex flex-col gap-1.5 text-sm",
        class.as_deref().unwrap_or("")
    );
    rsx! {
        label { "data-name": "Label", class: "{merged_class}",
            span { class: "text-xs font-medium text-muted-foreground select-none", "{text}" }
            {children}
        }
    }
}
