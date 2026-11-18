use dioxus::prelude::*;
use tw_merge::tw_merge;

#[derive(Clone, PartialEq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

impl SelectOption {
    pub fn new(value: impl ToString, label: impl ToString) -> Self {
        Self {
            value: value.to_string(),
            label: label.to_string(),
        }
    }
}

/// rust-ui `Select`, rendered as a native select so it works with keyboard,
/// search-by-typing and on phones.
#[component]
pub fn Select(
    #[props(into, optional)] class: Option<String>,
    #[props(into)] value: String,
    options: Vec<SelectOption>,
    #[props(optional)] disabled: bool,
    onchange: EventHandler<String>,
) -> Element {
    let merged_class = tw_merge!(
        "border-input bg-[var(--input-field-background)] flex h-9 w-full items-center justify-between gap-2 rounded-md border px-3 py-1 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px] disabled:cursor-not-allowed disabled:opacity-50 cursor-pointer",
        class.as_deref().unwrap_or("")
    );
    rsx! {
        select {
            "data-name": "Select",
            class: "{merged_class}",
            disabled,
            onchange: move |e| onchange.call(e.value()),
            for o in options {
                option { value: "{o.value}", selected: o.value == value, "{o.label}" }
            }
        }
    }
}
