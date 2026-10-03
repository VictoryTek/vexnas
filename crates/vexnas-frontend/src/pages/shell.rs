use leptos::prelude::*;

use crate::{api, AuthState};

#[component]
pub fn Shell(session: api::Session) -> impl IntoView {
    let auth = expect_context::<RwSignal<AuthState>>();
    let csrf = session.csrf_token.clone();

    let on_logout = move |_| {
        let csrf = csrf.clone();
        leptos::task::spawn_local(async move {
            let _ = api::post_json::<serde_json::Value>(
                "/api/v1/auth/logout",
                Some(&csrf),
                &serde_json::json!({}),
            )
            .await;
            auth.set(AuthState::LoggedOut);
        });
    };

    view! {
        <div class="app-shell">
            <aside class="sidebar">
                <div class="sidebar-brand">
                    <div class="brand-mark brand-mark-sm">"V"</div>
                    <span>"vexnas"</span>
                </div>
                <nav class="nav">
                    <span class="nav-item nav-item-active">"Overview"</span>
                    <span class="nav-item nav-item-soon">"Dashboard"</span>
                    <span class="nav-item nav-item-soon">"Storage"</span>
                    <span class="nav-item nav-item-soon">"Shares"</span>
                    <span class="nav-item nav-item-soon">"Users"</span>
                    <span class="nav-item nav-item-soon">"Jobs"</span>
                </nav>
                <div class="sidebar-user">
                    <div>
                        <div class="user-name">{session.username.clone()}</div>
                        <div class="muted small">{session.role.clone()}</div>
                    </div>
                    <button class="btn btn-ghost" on:click=on_logout>"Sign out"</button>
                </div>
            </aside>
            <main class="app-main">
                <h2>"Overview"</h2>
                <MetaCard />
            </main>
        </div>
    }
}

#[component]
fn MetaCard() -> impl IntoView {
    let auth = expect_context::<RwSignal<AuthState>>();
    let meta = LocalResource::new(|| api::get_json::<api::Meta>("/api/v1/meta"));

    view! {
        <div class="card">
            <Suspense fallback=|| view! { <p class="muted">"Loading…"</p> }>
                {move || Suspend::new(async move {
                    match meta.await {
                        Ok(m) => view! {
                            <dl class="kv">
                                <dt>"Host"</dt><dd>{m.hostname}</dd>
                                <dt>"Variant"</dt><dd>{m.variant.unwrap_or_else(|| "—".into())}</dd>
                                <dt>"vexnas"</dt><dd>{m.version}</dd>
                                <dt>"Root helper"</dt>
                                <dd>
                                    {if m.helper.ok {
                                        view! { <span class="pill pill-ok">"connected"</span>
                                                " " {m.helper.version.unwrap_or_default()} }.into_any()
                                    } else {
                                        view! { <span class="pill pill-bad">"unavailable"</span>
                                                " " {m.helper.error.unwrap_or_default()} }.into_any()
                                    }}
                                </dd>
                            </dl>
                            <p class="muted small">"Phase 0 shell — dashboard, storage, shares, users and jobs arrive in later phases."</p>
                        }.into_any(),
                        Err(e) => {
                            if e.is_unauthorized() {
                                auth.set(AuthState::LoggedOut);
                            }
                            view! { <div class="alert alert-danger">{e.message}</div> }.into_any()
                        }
                    }
                })}
            </Suspense>
        </div>
    }
}
