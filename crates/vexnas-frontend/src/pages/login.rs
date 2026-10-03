use leptos::prelude::*;

use crate::{api, AuthState};

#[component]
pub fn LoginPage() -> impl IntoView {
    let auth = expect_context::<RwSignal<AuthState>>();
    let (username, set_username) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (error, set_error) = signal(Option::<String>::None);
    let (loading, set_loading) = signal(false);

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let body = serde_json::json!({
            "username": username.get(),
            "password": password.get(),
        });
        set_loading.set(true);
        set_error.set(None);

        leptos::task::spawn_local(async move {
            let result = api::post_json::<api::Session>("/api/v1/auth/login", None, &body).await;
            set_loading.set(false);
            match result {
                Ok(session) => {
                    set_password.set(String::new());
                    auth.set(AuthState::LoggedIn(session));
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };

    view! {
        <div class="login-wrap">
            <div class="brand">
                <div class="brand-mark">"V"</div>
                <h1>"vexnas"</h1>
                <p class="muted">"Sign in with your system account"</p>
            </div>

            <div class="card login-card">
                {move || error.get().map(|e| view! { <div class="alert alert-danger" role="alert">{e}</div> })}

                <form on:submit=on_submit>
                    <label class="form-label" for="username">"Username"</label>
                    <input
                        id="username"
                        class="form-input"
                        type="text"
                        autocomplete="username"
                        required=true
                        autofocus=true
                        prop:value=move || username.get()
                        on:input=move |ev| set_username.set(event_target_value(&ev))
                    />
                    <label class="form-label" for="password">"Password"</label>
                    <input
                        id="password"
                        class="form-input"
                        type="password"
                        autocomplete="current-password"
                        required=true
                        prop:value=move || password.get()
                        on:input=move |ev| set_password.set(event_target_value(&ev))
                    />
                    <button type="submit" class="btn btn-primary btn-block" disabled=move || loading.get()>
                        {move || if loading.get() { "Signing in…" } else { "Sign in" }}
                    </button>
                </form>
            </div>
        </div>
    }
}
