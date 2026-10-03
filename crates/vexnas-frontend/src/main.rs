mod api;
mod pages;

use leptos::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    Loading,
    LoggedOut,
    LoggedIn(api::Session),
}

fn main() {
    console_error_panic_hook::set_once();

    if let Some(loader) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("initial-loader"))
    {
        loader.remove();
    }

    mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let auth = RwSignal::new(AuthState::Loading);
    provide_context(auth);

    leptos::task::spawn_local(async move {
        match api::get_json::<api::Session>("/api/v1/auth/session").await {
            Ok(session) => auth.set(AuthState::LoggedIn(session)),
            Err(_) => auth.set(AuthState::LoggedOut),
        }
    });

    view! {
        {move || match auth.get() {
            AuthState::Loading => view! { <div class="splash"><div class="spinner"></div></div> }.into_any(),
            AuthState::LoggedOut => view! { <pages::login::LoginPage /> }.into_any(),
            AuthState::LoggedIn(session) => view! { <pages::shell::Shell session=session /> }.into_any(),
        }}
    }
}
