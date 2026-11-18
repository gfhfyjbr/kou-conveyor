//! Transcript entries drawn by the app's plugins (see [`crate::face`]).
//! The native view lays out the rows and supplies slots for streaming
//! prose, command output and animation.
//!
//! An entry is known to the app by its own id where it has one (a tool
//! call's), by its place otherwise, and by the view it is in.

use super::*;
use serde_json::{Value, json};

impl ThreadView {
    pub(crate) fn link_opener(&self, cx: &Context<Self>) -> crate::prose::LinkFn {
        let view = cx.weak_entity();
        Rc::new(move |destination, _, cx| {
            view.update(cx, |_, cx| match destination {
                crate::links::Destination::File(path) => {
                    cx.emit(crate::view::ThreadViewEvent::OpenFile(path))
                }
                crate::links::Destination::Theme(link) => {
                    cx.emit(crate::view::ThreadViewEvent::OpenTheme(link))
                }
                crate::links::Destination::Web(url) => cx.open_url(&url),
            })
            .ok();
        })
    }

    /// Part `view` of the entry known as `key`, drawn by the app from
    /// `data` with the view's own `slots`: `None` when it is not, and the
    /// view draws it itself.
    ///
    /// `memo` is a digest of everything `data` is made from (see
    /// [`crate::ChatPart::memo`]), where the caller works one out.
    pub(crate) fn entry_face<'a>(
        &self,
        view: &'static str,
        key: impl std::fmt::Display,
        memo: Option<u64>,
        data: impl FnOnce() -> Value + 'a,
        slots: Vec<(&'static str, AnyElement)>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let face = self
            .chat_face
            .as_ref()
            .filter(|_| !self.around.studio() || view.starts_with("studio-"))?;
        let key = format!("{view}:{}:{key}", cx.entity_id().as_u64());
        let mut part = crate::face::Handed::new(view, key, data);
        part.memo = memo;
        let asked = Self::entry_asked as crate::face::Asked<Self>;
        crate::face::drawn(face, part, slots, cx.weak_entity(), asked, &self.trace, cx)
    }

    /// What an entry the app draws asks of the view: `toggle` (`{ index,
    /// open, keyboard }`: open or close it), `worked` (`{ start, keyboard }`:
    /// open or close a run of work), `permit` (`{ request, option }`),
    /// `choose` (`{ request, option, keyboard }`: an option of a question,
    /// by its place), `confirm` and `back` (`{ request, keyboard }`), `copy`,
    /// `edit` and `reuse` (`{ index }`: a prompt or an answer), `unclear`
    /// (show what `/clear` put out of sight), `nested` (`{ id, keyboard }`:
    /// open an agent started in a pane), `show` (`{ index, keyboard }`: show
    /// the card of the agent the call at `index` started, open), `folder`
    /// (open the project's folder), `beside` (`{ open }`: pull the card
    /// beside out, or put it away), `tick` (`{ index }`: go to a prompt),
    /// `unfold` (`{ index, piece }`: keep more of what a command printed).
    fn entry_asked(
        &mut self,
        name: &str,
        given: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let index = given
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize);
        // A click leaves the keys in the field; a key leaves them where they are.
        let pointer = given.get("keyboard").and_then(Value::as_bool) != Some(true);
        match (name, index) {
            ("toggle", Some(index)) => {
                let open = given.get("open").and_then(Value::as_bool) == Some(true);
                self.toggle(index, open, cx);
                if pointer {
                    self.focus_field(window, cx);
                }
            }
            ("permit", _) => {
                let request = given.get("request").and_then(Value::as_u64);
                let option = given.get("option").and_then(Value::as_str);
                if let (Some(request), Some(option)) = (request, option) {
                    self.permit(request, option.to_owned(), cx);
                    // The buttons are gone with the answer.
                    self.focus_field(window, cx);
                }
            }
            // A question's option, its Confirm, and Back to the one before.
            ("choose" | "confirm" | "back", _) => {
                let Some(request) = given.get("request").and_then(Value::as_u64) else {
                    return;
                };
                match name {
                    "choose" => {
                        if let Some(option) = given.get("option").and_then(Value::as_u64) {
                            self.choose_answer(request, option as usize, cx);
                        }
                    }
                    "confirm" => self.confirm_answer(request, cx),
                    _ => self.answer_back(request, cx),
                }
                if pointer {
                    self.focus_field(window, cx);
                }
            }
            ("copy", Some(index)) => {
                let copied = match (self.thread.entries().get(index), self.cache.get(index)) {
                    (Some(Entry::Assistant(text)), _) => {
                        Some((format!("answer-{index}"), text.clone()))
                    }
                    (_, Some(Some(Parsed::User(message)))) => {
                        Some((format!("prompt-{index}"), message.text.to_string()))
                    }
                    _ => None,
                };
                if let Some((key, text)) = copied {
                    self.copy(key.into(), text, cx);
                }
                if pointer {
                    self.focus_field(window, cx);
                }
            }
            ("worked", _) => {
                if let Some(start) = given.get("start").and_then(Value::as_u64) {
                    self.toggle_worked(start as usize, cx);
                }
                if pointer {
                    self.focus_field(window, cx);
                }
            }
            // What `/clear` put out of sight, shown again.
            ("unclear", _) => {
                self.unclear(cx);
                self.focus_field(window, cx);
            }
            // An agent started in a pane, or by the call at `index` (a row
            // of the card beside): it is opened, or its card shown.
            ("nested", _) => {
                if let Some(id) = given.get("id").and_then(Value::as_str) {
                    cx.emit(crate::view::ThreadViewEvent::OpenNested(id.to_owned()));
                }
                if pointer {
                    self.focus_field(window, cx);
                }
            }
            ("show", Some(index)) => {
                self.show_agent(index, cx);
                if pointer {
                    self.focus_field(window, cx);
                }
            }
            // The card beside: its project's folder opened, the card put
            // away or pulled out again.
            ("folder", _) => {
                if let Some(beside) = &self.nest.beside {
                    cx.emit(crate::view::ThreadViewEvent::OpenFolder(
                        beside.path.clone(),
                    ));
                }
            }
            ("beside", _) => {
                let out = given.get("open").and_then(Value::as_bool) == Some(true);
                self.pull_beside(out, cx);
            }
            // The earlier lines of what a command printed, kept on screen.
            ("unfold", Some(index)) => {
                if let Some(piece) = given.get("piece").and_then(Value::as_u64) {
                    self.unfold(index, piece as usize, cx);
                }
                self.focus_field(window, cx);
            }
            // A tick down the edge: the transcript goes to its prompt.
            ("tick", Some(index)) => {
                self.list.scroll_to(gpui::ListOffset {
                    item_ix: index,
                    offset_in_item: gpui::Pixels::ZERO,
                });
                self.focus_field(window, cx);
            }
            ("edit", Some(index)) => self.begin_edit(index, window, cx),
            ("reuse", Some(index)) => self.reuse(index, window, cx),
            _ => {}
        }
    }

    /// A plan's card (`chat-plan`): `steps` (`{ text, status }`, status
    /// `pending`, `running` or `done`). The mark of the step that runs
    /// blinks: it is the slot `mark`. (A plan with more than one step
    /// running is the view's own.)
    pub(crate) fn plan_face(
        &self,
        index: usize,
        steps: &[PlanStep],
        lit: f32,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let running = steps
            .iter()
            .filter(|step| step.status == PlanStatus::Running)
            .count();
        if running > 1 {
            return None;
        }
        let slots = (running == 1)
            .then(|| {
                let mark = div()
                    .size(px(8.))
                    .rounded(px(metric::RADIUS_MARK))
                    .bg(theme.working.opacity(lit));
                ("mark", mark.into_any_element())
            })
            .into_iter()
            .collect();
        let memo = crate::face::digest(
            steps
                .iter()
                .map(|step| (&step.text, status(step.status)))
                .collect::<Vec<_>>(),
        );
        let data = || {
            json!({
                "steps": steps
                    .iter()
                    .map(|step| json!({
                        "text": step.text,
                        "status": status(step.status),
                    }))
                    .collect::<Vec<_>>(),
            })
        };
        self.entry_face(
            if self.around.studio() {
                "studio-plan"
            } else {
                "chat-plan"
            },
            index,
            Some(memo),
            data,
            slots,
            cx,
        )
    }

    /// A message from the backend (`chat-notice`): `level` (`info`,
    /// `warning`, `error`), `text`, and `turn` (the error a turn ended with).
    pub(crate) fn notice_face(
        &self,
        index: usize,
        level: Level,
        text: &str,
        turn: bool,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let level = match level {
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "error",
        };
        let memo = crate::face::digest((level, text, turn));
        let data = || json!({ "level": level, "text": text, "turn": turn });
        self.entry_face(
            if self.around.studio() {
                "studio-notice"
            } else {
                "chat-notice"
            },
            index,
            Some(memo),
            data,
            Vec::new(),
            cx,
        )
    }
}

/// A plan step's status, as a drawing names it.
fn status(status: PlanStatus) -> &'static str {
    match status {
        PlanStatus::Pending => "pending",
        PlanStatus::Running => "running",
        PlanStatus::Done => "done",
    }
}
