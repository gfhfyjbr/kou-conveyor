use kou_view::{HeaderPage, ItemHeader, SessionDescription, SessionFile};
use leptos::prelude::*;
use serde::{Deserialize, de::DeserializeOwned};
use std::cell::RefCell;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{AbortController, RequestInit, Response};

const PAGE: u64 = 50;

thread_local! {
    static FETCH: RefCell<(u64, Option<AbortController>)> = const { RefCell::new((0, None)) };
}

struct FetchGuard(u64);
impl Drop for FetchGuard {
    fn drop(&mut self) {
        FETCH.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.0 == self.0 {
                slot.1.take();
            }
        });
    }
}

async fn get<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    let controller =
        AbortController::new().map_err(|_| "Cannot create a browser request".to_string())?;
    let ticket = FETCH.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(old) = slot.1.take() {
            old.abort();
        }
        slot.0 = slot.0.wrapping_add(1);
        slot.1 = Some(controller.clone());
        slot.0
    });
    let _guard = FetchGuard(ticket);
    let init = RequestInit::new();
    init.set_signal(Some(&controller.signal()));
    let window = web_sys::window().ok_or("Browser window unavailable")?;
    let response = JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await
        .map_err(|_| "Request failed or was cancelled. Reload the session to retry.".to_string())?
        .dyn_into::<Response>()
        .map_err(|_| "Invalid HTTP response")?;
    if !response.ok() {
        return Err(match response.status() {
            409 => "Session changed. Reload it to read the new revision.".into(),
            413 => "This record exceeds the inline memory budget. Download the session to read its full contents.".into(),
            429 => "The reader is busy. Try again.".into(),
            status => format!("Request failed (HTTP {status}). Reload to retry."),
        });
    }
    let text = JsFuture::from(response.text().map_err(|_| "Cannot read response")?)
        .await
        .map_err(|_| "Cannot read response")?
        .as_string()
        .ok_or("Response is not text")?;
    serde_json::from_str(&text)
        .map_err(|_| "Response does not match the Rust reader protocol".into())
}

#[derive(Deserialize)]
struct SessionPage {
    sessions: Vec<SessionFile>,
    next_after: String,
    more: bool,
}

#[derive(Clone, Copy)]
struct Ui {
    sessions: RwSignal<Vec<SessionFile>>,
    list_after: RwSignal<String>,
    list_more: RwSignal<bool>,
    session: RwSignal<String>,
    view: RwSignal<Option<SessionDescription>>,
    page: RwSignal<Option<HeaderPage>>,
    body: RwSignal<String>,
    body_sequence: RwSignal<u64>,
    error: RwSignal<String>,
    loading: RwSignal<bool>,
    epoch: RwSignal<u64>,
}

impl Ui {
    fn new() -> Self {
        Self {
            sessions: RwSignal::new(Vec::new()),
            list_after: RwSignal::new(String::new()),
            list_more: RwSignal::new(false),
            session: RwSignal::new(String::new()),
            view: RwSignal::new(None),
            page: RwSignal::new(None),
            body: RwSignal::new(String::new()),
            body_sequence: RwSignal::new(0),
            error: RwSignal::new(String::new()),
            loading: RwSignal::new(false),
            epoch: RwSignal::new(0),
        }
    }

    fn begin(self) -> u64 {
        self.epoch.update(|n| *n = n.wrapping_add(1));
        self.error.set(String::new());
        self.loading.set(true);
        self.epoch.get_untracked()
    }

    fn finish(self, epoch: u64, error: Option<String>) {
        if self.epoch.get_untracked() != epoch {
            return;
        }
        if let Some(error) = error {
            self.error.set(error);
        }
        self.loading.set(false);
    }

    fn list(self, next: bool) {
        let epoch = self.begin();
        let after = if next {
            self.list_after.get_untracked()
        } else {
            String::new()
        };
        spawn_local(async move {
            match get::<SessionPage>(&format!("/api/v2/sessions?limit=100&after={after}")).await {
                Ok(page) if self.epoch.get_untracked() == epoch => {
                    self.sessions.set(page.sessions);
                    self.list_after.set(page.next_after);
                    self.list_more.set(page.more);
                    self.finish(epoch, None);
                }
                Err(error) => self.finish(epoch, Some(error)),
                _ => {}
            }
        });
    }

    fn open(self, id: String) {
        let epoch = self.begin();
        self.session.set(id.clone());
        self.view.set(None);
        self.page.set(None);
        self.body.set(String::new());
        self.body_sequence.set(0);
        if id.is_empty() {
            self.finish(epoch, None);
            return;
        }
        spawn_local(async move {
            let result = async {
                let view: SessionDescription = get(&format!("/api/v2/sessions/{id}/view")).await?;
                if self.epoch.get_untracked() != epoch {
                    return Ok::<(), String>(());
                }
                let after = (view.items as u64).saturating_sub(PAGE);
                let page: HeaderPage = get(&format!(
                    "/api/v2/sessions/{id}/headers?generation={}&after={after}&limit={PAGE}",
                    view.generation
                ))
                .await?;
                if self.epoch.get_untracked() == epoch {
                    self.view.set(Some(view));
                    self.page.set(Some(page));
                }
                Ok(())
            }
            .await;
            self.finish(epoch, result.err());
        });
    }

    fn turn_page(self, earlier: bool) {
        let Some(view) = self.view.get_untracked() else {
            return;
        };
        let Some(page) = self.page.get_untracked() else {
            return;
        };
        let first = page
            .items
            .first()
            .map_or(0, |h| h.sequence.saturating_sub(1));
        let after = if earlier {
            first.saturating_sub(PAGE)
        } else {
            page.next_after
        };
        let epoch = self.begin();
        self.body.set(String::new());
        self.body_sequence.set(0);
        let id = self.session.get_untracked();
        spawn_local(async move {
            match get::<HeaderPage>(&format!(
                "/api/v2/sessions/{id}/headers?generation={}&after={after}&limit={PAGE}",
                view.generation
            ))
            .await
            {
                Ok(page) if self.epoch.get_untracked() == epoch => {
                    self.page.set(Some(page));
                    self.finish(epoch, None);
                }
                Err(error) => self.finish(epoch, Some(error)),
                _ => {}
            }
        });
    }

    fn show(self, sequence: u64) {
        let Some(view) = self.view.get_untracked() else {
            return;
        };
        let epoch = self.begin();
        self.body.set(String::new());
        self.body_sequence.set(sequence);
        let id = self.session.get_untracked();
        spawn_local(async move {
            match get::<serde_json::Value>(&format!(
                "/api/v2/sessions/{id}/items/{sequence}?generation={}",
                view.generation
            ))
            .await
            {
                Ok(value) if self.epoch.get_untracked() == epoch => {
                    match serde_json::to_string_pretty(&value) {
                        Ok(text) => {
                            self.body.set(text);
                            self.finish(epoch, None);
                        }
                        Err(_) => self.finish(epoch, Some("Cannot render this record".into())),
                    }
                }
                Err(error) => self.finish(epoch, Some(error)),
                _ => {}
            }
        });
    }
}

#[component]
pub fn App() -> impl IntoView {
    let ui = Ui::new();
    ui.list(false);
    view! {
        <a class="skip" href="#history">"Skip to session history"</a>
        <header class="bar">
            <strong>"kou-conveyor"</strong>
            <span>"Rust session reader · read-only"</span>
        </header>
        <main>
            <section class="picker" aria-label="Session selection">
                <label for="session">"Session"</label>
                <select id="session" prop:value=move || ui.session.get()
                    on:change=move |event| ui.open(event_target_value(&event))>
                    <option value="">"Choose a session"</option>
                    <For each=move || ui.sessions.get() key=|session| session.id.clone() children=move |session| {
                        let id = session.id;
                        view! { <option value=id.clone()>{id.clone()}</option> }
                    }/>
                </select>
                <button on:click=move |_| ui.list(false) disabled=move || ui.loading.get()>"Refresh list"</button>
                <Show when=move || ui.list_more.get()>
                    <button on:click=move |_| ui.list(true) disabled=move || ui.loading.get()>"Next sessions"</button>
                </Show>
            </section>
            <p class="scope">"This migration slice reads durable records without running tools or changing sessions. The existing Go cockpit still owns execution."</p>
            <div class="status" role="status" aria-live="polite">{move || if ui.loading.get() { "Reading session…" } else { "" }}</div>
            <Show when=move || !ui.error.get().is_empty()>
                <p class="error" role="alert">{move || ui.error.get()}</p>
            </Show>
            <Show when=move || ui.sessions.get().is_empty() && !ui.loading.get() && ui.error.get().is_empty()>
                <p>"No session files were found in the configured directory. Select an existing session directory when starting the reader."</p>
            </Show>
            <Show when=move || ui.view.get().is_some()>
                <section class="history" id="history" aria-label="Session history">
                    <div class="history-heading">
                        <h1>{move || ui.view.get().map(|v| format!("{} records", v.items)).unwrap_or_default()}</h1>
                        <button on:click=move |_| ui.open(ui.session.get_untracked()) disabled=move || ui.loading.get()>"Reload session"</button>
                        <a class="download" href=move || ui.view.get().map(|v| format!("/api/v2/sessions/{}/export?generation={}", v.session.id, v.generation)).unwrap_or_default()>"Download full JSONL"</a>
                    </div>
                    <div class="pager" aria-label="History pages">
                        <button on:click=move |_| ui.turn_page(true)
                            disabled=move || ui.loading.get() || ui.page.get().and_then(|p| p.items.first().map(|h| h.sequence <= 1)).unwrap_or(true)>"Earlier records"</button>
                        <button on:click=move |_| ui.turn_page(false)
                            disabled=move || ui.loading.get() || !ui.page.get().is_some_and(|p| p.more)>"Later records"</button>
                        <span>{move || ui.page.get().and_then(|p| p.items.first().map(|h| format!("{}-{}", h.sequence, p.next_after))).unwrap_or_default()}</span>
                    </div>
                    <ol class="records">
                        <For each=move || ui.page.get().map(|p| p.items).unwrap_or_default() key=|item| item.sequence children=move |item| {
                            view! { <RecordRow item=item ui=ui/> }
                        }/>
                    </ol>
                    <Show when=move || ui.page.get().is_some_and(|p| p.items.is_empty())>
                        <p>"This session has no committed history items."</p>
                    </Show>
                </section>
            </Show>
            <Show when=move || !ui.body.get().is_empty()>
                <section class="details" aria-label="Selected durable record">
                    <div class="history-heading">
                        <h2>{move || format!("Record {}", ui.body_sequence.get())}</h2>
                        <button on:click=move |_| { ui.body.set(String::new()); ui.body_sequence.set(0); }>"Close record"</button>
                    </div>
                    <pre tabindex="0">{move || ui.body.get()}</pre>
                </section>
            </Show>
        </main>
    }
}

#[component]
fn RecordRow(item: ItemHeader, ui: Ui) -> impl IntoView {
    let sequence = item.sequence;
    view! {
        <li>
            <button class="record" on:click=move |_| ui.show(sequence)
                aria-pressed=move || (ui.body_sequence.get() == sequence).to_string()>
                <span class="record-number">{sequence}</span>
                <span class="record-kind">{item.kind}</span>
                <span class="record-preview">{item.preview}</span>
                <time>{item.recorded_at.to_string()}</time>
            </button>
        </li>
    }
}
