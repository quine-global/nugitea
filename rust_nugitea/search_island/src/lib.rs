//! The one piece of client-side interactivity in nugitea's web UI: a
//! press-`/`-to-open search box that filters the whole repo's file list in
//! real time. Everything else in the file browser is plain SSR (see the
//! main crate's `webui.rs`) — this is deliberately kept as small and
//! separate as possible (its own crate, its own minimal dependency set)
//! so the wasm bundle stays tiny and the rest of the app never needs to
//! know `wasm32-unknown-unknown` exists.
//!
//! `ssr`+`islands` features render this island's static HTML placeholder
//! as part of the server-rendered page; `hydrate`+`islands` (compiled to
//! wasm via `wasm-pack build --target web`) hydrates it in the browser.

use leptos::prelude::*;

#[island]
pub fn SearchBox(repo: String, git_ref: String) -> impl IntoView {
    let open = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let files = RwSignal::new(Vec::<String>::new());
    let input_ref = NodeRef::<leptos::html::Input>::new();

    fetch_files(repo.clone(), git_ref.clone(), files);

    let open_and_focus = move || {
        open.set(true);
        // Deferred a frame: the panel's `display:none` is removed by this
        // same signal update, and a still-hidden input can't take focus —
        // by the next animation frame the DOM has definitely caught up.
        request_animation_frame(move || {
            if let Some(el) = input_ref.get() {
                let _ = el.focus();
            }
        });
    };
    install_shortcuts(open, open_and_focus);

    let repo_for_results = repo.clone();
    let git_ref_for_results = git_ref.clone();
    let results = move || {
        let q = query.get().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        files.get().into_iter().filter(|f| f.to_lowercase().contains(&q)).take(50).collect::<Vec<_>>()
    };

    view! {
        <div class="search-island">
            <button class="search-toggle" on:click=move |_| open_and_focus()>
                "search files ("<kbd>"/"</kbd>")"
            </button>
            <div class="search-panel" class:hidden=move || !open.get()>
                <input
                    node_ref=input_ref
                    type="text"
                    placeholder="filter files..."
                    prop:value=move || query.get()
                    on:input=move |ev| query.set(leptos::leptos_dom::helpers::event_target_value(&ev))
                    on:keydown=move |ev| {
                        if ev.key() == "Escape" {
                            open.set(false);
                        }
                    }
                />
                <ul class="search-results">
                    {move || {
                        results()
                            .into_iter()
                            .map(|f| {
                                let href = format!("/{}/blob/{}/{}", repo_for_results, git_ref_for_results, f);
                                view! { <li><a href=href>{f}</a></li> }
                            })
                            .collect::<Vec<_>>()
                    }}
                </ul>
            </div>
        </div>
    }
}

#[cfg(feature = "hydrate")]
fn fetch_files(repo: String, git_ref: String, files: RwSignal<Vec<String>>) {
    leptos::task::spawn_local(async move {
        let url = format!("/{repo}/api/files/{git_ref}");
        if let Ok(resp) = gloo_net::http::Request::get(&url).send().await {
            if let Ok(list) = resp.json::<Vec<String>>().await {
                files.set(list);
            }
        }
    });
}

#[cfg(not(feature = "hydrate"))]
fn fetch_files(_repo: String, _git_ref: String, _files: RwSignal<Vec<String>>) {}

#[cfg(feature = "hydrate")]
fn install_shortcuts(open: RwSignal<bool>, open_and_focus: impl Fn() + 'static) {
    leptos::leptos_dom::helpers::window_event_listener(leptos::ev::keydown, move |ev| {
        if ev.key() == "/" && !open.get_untracked() {
            ev.prevent_default();
            open_and_focus();
        } else if ev.key() == "Escape" {
            open.set(false);
        }
    });
}

#[cfg(not(feature = "hydrate"))]
fn install_shortcuts(_open: RwSignal<bool>, _open_and_focus: impl Fn() + 'static) {}

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn hydrate() {
    leptos::mount::hydrate_islands();
}
