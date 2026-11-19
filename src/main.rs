//! nrzBackup web app: encrypted backups of other apps to Filen.io.
//! Built with Dioxus and the nrzUI components from rust-ui (https://rust-ui.com).

use dioxus::prelude::*;

mod api;
mod components;
mod pages;

use pages::calendar::Calendar;
use pages::events::Events;
use pages::explorer::Explorer;
use pages::layout::Shell;
use pages::overview::Overview;
use pages::settings::Settings;
use pages::source_detail::SourceView;
use pages::sources::Sources;

const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const FAVICON: Asset = asset!("/assets/favicon.svg");

#[derive(Debug, Clone, Routable, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Overview {},
        #[route("/sources")]
        Sources {},
        #[route("/sources/:id")]
        SourceView { id: i64 },
        #[route("/calendar")]
        Calendar {},
        #[route("/files")]
        Explorer {},
        #[route("/events")]
        Events {},
        #[route("/settings")]
        Settings {},
        #[route("/:..rest")]
        NotFound { rest: Vec<String> },
}

#[component]
fn NotFound(rest: Vec<String>) -> Element {
    let nav = navigator();
    let _ = rest;
    use_effect(move || {
        nav.replace(Route::Overview {});
    });
    rsx! {}
}

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Title { "nrzBackup" }
        document::Link { rel: "icon", href: FAVICON, r#type: "image/svg+xml" }
        document::Stylesheet { href: TAILWIND_CSS }
        Router::<Route> {}
    }
}
