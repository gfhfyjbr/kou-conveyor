//! The prompt field on its own, without a conversation: what the app puts in
//! the middle of its empty screen. It is the prompt area of a thread view,
//! drawn by the same code, so it looks and behaves as the field does in a
//! conversation: it wraps, takes pictures pasted or dropped on it, links a
//! project's files with `$` once it has a project, walks earlier prompts with
//! ↑ ↓ and searches them with Ctrl+R. What needs a backend is not there: no
//! slash commands, no model or mode to choose, no queue, no figures.
//!
//! ```ignore
//! let field = cx.new(|cx| {
//!     let mut field = PromptField::new(window, cx);
//!     field.set_placeholder("What should be done?", cx);
//!     field.set_foot(Some(move |window: &mut Window, cx: &mut App| {
//!         recipient.update(cx, |button, cx| button.render(window, cx)).into_any_element()
//!     }), cx);
//!     field
//! });
//! cx.subscribe(&field, |this, _, event: &PromptFieldEvent, cx| match event {
//!     PromptFieldEvent::Submit(prompt) => this.start(prompt.blocks.clone(), cx),
//!     PromptFieldEvent::Height(height) => this.recentre(*height, cx),
//! });
//! ```

use crate::{
    dock::{Dock, DockEvent, Sync},
    trace,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div,
};
use kou_thread::{Block, Capabilities, ThreadState};
use std::{cell::Cell, path::PathBuf, rc::Rc};

/// What the app draws at the start of the field's foot, built every frame.
pub(crate) type Foot = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// Takes the files dropped on a field.
pub type FieldDrop = Rc<dyn Fn(&gpui::ExternalPaths, &mut Window, &mut App)>;

/// Does something of a field's, by name.
pub type FieldAct = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// The prompt field in parts, for an app that draws its frame itself: how
/// the field stands, and the parts that stay the view's own to draw, which
/// the app places where it likes.
pub struct FieldParts {
    /// Presentation requested by the thread: `cockpit` or `studio`.
    pub look: &'static str,
    /// Which field this is, of all the ones drawn: the same for its life.
    pub key: SharedString,
    /// The keys are in it.
    pub focused: bool,
    /// Its agent is at work.
    pub busy: bool,
    /// Its agent is gone.
    pub closed: bool,
    /// It has no room for the names of its controls.
    pub narrow: bool,
    /// It stands alone, with no conversation over it.
    pub standalone: bool,
    /// How the field's own controls stand, for a frame that draws them
    /// itself instead of placing the view's: `buttons` (`send`; while the
    /// agent works, `stop`, or `force` once something is typed), `ready`
    /// (what is typed can be sent), `attach` (the paperclip is there),
    /// `model` (`{ name, open, dot }`, the dot its provider's colour) and
    /// `mode` (`{ name, open, effort }`, `effort` being `{ level, bars }`
    /// where the modes are efforts). `effort` and `access` describe separate
    /// level and permission controls. Each picker is `null` when unavailable.
    pub controls: serde_json::Value,
    /// The view's own parts by name, each to be placed once: `editor` (what
    /// is typed, with its chips, pictures and menus), `foot` (what the app
    /// puts at the foot's start), `attach`, `model`, `effort`, `mode`, `access`, and
    /// `controls` (the buttons that send and stop). A frame drawing its own
    /// picker control places its open list in the matching `*Menu` slot.
    /// One the field has none of now is not among them.
    pub slots: Vec<(&'static str, AnyElement)>,
    /// Takes files dropped on the field.
    pub dropped: FieldDrop,
    /// Does something of the field's, by name: `attach` (asks for files),
    /// `send` (what is typed; while the agent works, it is queued),
    /// `force` (what is typed, into the running turn), `stop` (the running
    /// turn), `focus`, and the picker names (open or close their lists).
    pub act: FieldAct,
}

/// Draws the field's frame from its parts, in place of the view's own.
/// `None` leaves it to the view, which then draws the field as it always
/// did: an app whose drawing fails still has a field to type in.
pub type FieldFace = Rc<dyn Fn(FieldParts, &Window, &App) -> Option<AnyElement>>;

/// A prompt the field hands over.
#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    /// What was typed, each picture named in it by its label (`[Image 1]`).
    pub text: String,
    /// The prompt as a backend takes it: the text, then each picture as an
    /// image, then each file its `$` links named, when it has a project.
    pub blocks: Vec<Block>,
}

/// What the field tells the app around it.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptFieldEvent {
    /// Enter, or the run button, on a field that held something. The field
    /// is empty again; the prompt is the app's.
    Submit(Prompt),
    /// Enter, or the run button, on a field that held nothing. There is no
    /// prompt; what the gesture means is the app's to say.
    Empty,
    /// A model was chosen in the field's own list (see
    /// [`PromptField::set_models`]): its id. The field shows it as chosen.
    Model(String),
    /// The permissions selected before the first prompt.
    Access(String),
    /// The field's height as drawn, in unzoomed pixels, once it has settled
    /// after it changed: a line more, a picture's strip, a notice.
    Height(f32),
}

/// The prompt field, standing alone. See the [module](self).
pub struct PromptField {
    dock: Entity<Dock>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PromptFieldEvent> for PromptField {}

impl Focusable for PromptField {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.dock.read(cx).focus.clone()
    }
}

impl PromptField {
    /// An empty field, drawn in the app's theme. It takes pictures; it has no
    /// project (no `$` links) and keeps its history in memory until told
    /// otherwise.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::view::bind_keys(cx);
        let dock = cx.new(|cx| {
            let mut dock = Dock::new(
                SharedString::default(),
                None,
                Rc::new(Cell::new(1.)),
                Rc::new(Cell::new(None)),
                trace::new(),
                window,
                cx,
            );
            dock.standalone = true;
            dock.composer.update(cx, |composer, cx| {
                composer.set_commands_off(true, cx);
            });
            dock.sync(
                Sync {
                    state: ThreadState::Idle,
                    ready: true,
                    capabilities: Capabilities {
                        images: true,
                        embedded_files: false,
                        resume: false,
                    },
                    ..Sync::default()
                },
                cx,
            );
            dock
        });
        let subscriptions = vec![
            cx.subscribe(&dock, |_, _, event: &DockEvent, cx| match event {
                DockEvent::Send { blocks, draft } => cx.emit(PromptFieldEvent::Submit(Prompt {
                    text: draft.text.clone(),
                    blocks: blocks.clone(),
                })),
                DockEvent::Blank => cx.emit(PromptFieldEvent::Empty),
                DockEvent::SetModel(model) => cx.emit(PromptFieldEvent::Model(model.clone())),
                DockEvent::SetAccess(id) => cx.emit(PromptFieldEvent::Access(id.clone())),
                DockEvent::Height(height) => cx.emit(PromptFieldEvent::Height(*height)),
                _ => {}
            }),
        ];
        Self {
            dock,
            _subscriptions: subscriptions,
        }
    }

    /// What the empty field says.
    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        self.dock.update(cx, |dock, cx| {
            dock.composer.update(cx, |composer, cx| {
                composer.set_placeholders(text.clone(), text, cx)
            })
        });
    }

    /// Puts the app's own element at the start of the field's foot, left of
    /// the button that sends: `foot` builds it every frame the field is
    /// drawn, in the field's scale. `None` takes it away.
    pub fn set_foot<F>(&mut self, foot: Option<F>, cx: &mut Context<Self>)
    where
        F: Fn(&mut Window, &mut App) -> AnyElement + 'static,
    {
        let foot = foot.map(|foot| Rc::new(foot) as Foot);
        self.dock.update(cx, |dock, cx| {
            dock.foot = foot;
            cx.notify();
        });
    }

    /// Lets the app draw the field's frame and arrange its parts: see
    /// [`FieldFace`]. `None` gives it back to the view.
    pub fn set_face(&mut self, face: Option<FieldFace>, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| {
            dock.face = face;
            cx.notify();
        });
    }

    /// Lets the app draw the lists over the field and the help card, as a
    /// chat's are drawn: see [`crate::ChatFace`]. `None` gives them back.
    pub fn set_chat_face(&mut self, face: Option<crate::ChatFace>, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| {
            dock.composer.update(cx, |composer, cx| {
                composer.chat_face = face.clone();
                cx.notify();
            });
            dock.chat_face = face;
            cx.notify();
        });
    }

    /// The models the agent this field starts can run with, for the field's
    /// own model control (as in a conversation: providers first, where
    /// there are any), with the one in force. Empty takes the control away.
    /// A choice made there comes back as [`PromptFieldEvent::Model`], and
    /// the app says with another call here that it is in force.
    pub fn set_models(
        &mut self,
        models: Vec<kou_thread::Choice>,
        providers: Vec<kou_thread::Provider>,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.dock.update(cx, |dock, cx| {
            let sync = Sync {
                models,
                providers,
                model,
                ..dock.synced().clone()
            };
            dock.sync(sync, cx);
            cx.notify();
        });
    }

    /// The project whose files a `$` links, and in whose `.harness` the
    /// cockpits share their prompt history; `None` for neither.
    pub fn set_project(&mut self, project: Option<PathBuf>, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| {
            let history = project.as_deref().map(crate::history::path);
            dock.set_cwd(project.clone(), cx);
            dock.composer
                .update(cx, |composer, _| composer.set_project(project));
            dock.set_history_file(history, cx);
        });
    }

    /// The permission presets offered for the next conversation.
    pub fn set_access(&mut self, access: kou_thread::Access, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| {
            dock.sync(
                Sync {
                    access,
                    ..dock.synced().clone()
                },
                cx,
            );
        });
    }

    /// Where earlier prompts are kept and shared; `None` keeps them in memory.
    pub fn set_history_file(&mut self, file: Option<PathBuf>, cx: &mut Context<Self>) {
        self.dock
            .update(cx, |dock, cx| dock.set_history_file(file, cx));
    }

    /// Puts the keyboard in the field.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.dock.read(cx).focus.clone();
        window.focus(&focus, cx);
    }

    /// What is typed.
    pub fn text(&self, cx: &App) -> String {
        self.dock.read(cx).composer.read(cx).text().to_owned()
    }

    /// Empties the field, its pictures and links with it.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.dock.update(cx, |dock, cx| {
            dock.composer.update(cx, |composer, cx| composer.clear(cx))
        });
    }

    /// The field's prompt area, for tests.
    #[cfg(test)]
    pub(crate) fn dock(&self) -> &Entity<Dock> {
        &self.dock
    }
}

impl Render for PromptField {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w_full().child(self.dock.clone())
    }
}
