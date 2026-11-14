//! A prompt edited where it stands, after the web cockpit's edit plugin
//! (`plugins/edit/web/edit.js`, `edit.css`).
//!
//! The editor takes the prompt's place in the transcript: the conversation's
//! own field ([`Composer`]) with the prompt's text and pictures, a note that
//! says what running it replaces, Cancel (Esc) and Run (Enter). The prompt's
//! header says it is being edited, and the entries after it fade until the
//! edit runs or is cancelled. Running it rewinds the session to before that
//! prompt and runs the edited prompt from there ([`Request::Rewind`]); the
//! thread lets go of what it replaces at once and takes it back if the
//! backend could not rewind, and the editor opens again with what was typed.
//! Esc anywhere in the pane cancels it. One edit is open at a time.
//!
//! Only a backend that says it rewinds ([`kou_thread::Offers::rewind`])
//! edits; for any other the prompt's control puts it back in the field
//! instead, and is called Reuse.

use crate::{
    composer::{Composer, ComposerEvent},
    entries::LABEL,
    view::{Shot, ThreadView},
};
use gpui::{
    AnimationExt as _, AnyElement, AppContext as _, ClickEvent, Context, Entity, ExternalPaths,
    FocusHandle, Focusable as _, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    Window, div,
};
use kou_thread::{Block, Entry, Request, ThreadState, Update};
use kou_ui::{
    Theme, key_hint, metric,
    motion::{self, cockpit},
    scale::{self, px},
};
use std::collections::{HashMap, HashSet};

/// What the editor's field says while it is empty.
const PLACEHOLDER: &str = "Edit the prompt";

/// What running an edit replaces, with `after` entries after the prompt
/// (`.edit-note`): nothing is said when there are none.
pub(crate) fn note(after: usize) -> String {
    match after {
        0 => String::new(),
        1 => "Replaces the entry below · changes to files stay".to_owned(),
        count => format!("Replaces the {count} entries below · changes to files stay"),
    }
}

/// A prompt being edited.
pub(crate) struct Editor {
    /// The user entry being edited.
    pub index: usize,
    /// The field it is edited in: the one the conversation's prompts are
    /// written in, standing in the row.
    pub composer: Entity<Composer>,
    pub focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// An edit on its way to the backend: the editor, to open again if the
/// rewind fails, and what the view kept of the entries it replaced.
pub(crate) struct Sent {
    editor: Editor,
    prompt_models: HashMap<usize, String>,
    toggled: HashMap<usize, bool>,
    unfolded: HashSet<(usize, usize)>,
    shots: HashMap<(usize, usize), Shot>,
}

impl ThreadView {
    /// Whether the backend rewinds: a prompt's control edits it in place.
    /// Otherwise it puts the prompt back in the field.
    pub(crate) fn edits(&self) -> bool {
        self.thread.offers.rewind
    }

    /// Why no prompt can be edited now, if none can.
    pub(crate) fn edit_blocked(&self) -> Option<&'static str> {
        match self.thread.state() {
            ThreadState::Idle if self.thread.rewinding().is_some() => {
                Some("The last edit is still on its way.")
            }
            ThreadState::Idle => None,
            ThreadState::Working | ThreadState::Blocked => {
                Some("The agent is working: stop it or let it finish, then edit.")
            }
            ThreadState::Starting => Some("The agent is starting: edit once it is ready."),
            ThreadState::Closed(_) => Some("This session has ended: nothing can be edited."),
        }
    }

    /// Opens the editor of the prompt of entry `index`, with its text and
    /// pictures, focused, the caret at the end. An editor open for another
    /// prompt closes; one open for this prompt keeps what was typed.
    pub(crate) fn begin_edit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.thread.entries().get(index), Some(Entry::User(_))) {
            return;
        }
        if let Some(blocked) = self.edit_blocked() {
            self.dock
                .update(cx, |dock, cx| dock.tell(blocked, true, cx));
            return;
        }
        if self
            .editor
            .as_ref()
            .is_some_and(|editor| editor.index != index)
        {
            self.close_edit(false, window, cx);
        }
        if self.editor.is_none() {
            let editor = self.make_editor(index, window, cx);
            self.editor = Some(editor);
            self.motion
                .edit_opened(index, cx.background_executor().now());
            self.remeasure_rows(index..index + 1);
        }
        if let Some(editor) = &self.editor {
            window.focus(&editor.focus, cx);
        }
        self.reveal(index);
        cx.notify();
    }

    /// Scrolls entry `index` into sight when it is not, its top at the top:
    /// the list stops following its end. Where the prompt area covers the
    /// transcript's bottom is out of sight.
    fn reveal(&mut self, index: usize) {
        let viewport = self.list.viewport_bounds();
        let covered = gpui::px(self.dock_height * self.scale.get());
        let shown = self.list.bounds_for_item(index).is_some_and(|row| {
            row.top() >= viewport.top() && row.bottom() <= viewport.bottom() - covered
        });
        if !shown {
            self.list.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: gpui::Pixels::ZERO,
            });
        }
    }

    /// The editor of entry `index`: the conversation's field with the prompt
    /// in it.
    fn make_editor(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> Editor {
        let (text, pictures) = match self.thread.entries().get(index) {
            Some(Entry::User(blocks)) => {
                let (text, labels) = crate::draft::shown(blocks);
                let pictures: Vec<(String, std::sync::Arc<[u8]>)> = labels
                    .into_iter()
                    .filter_map(|(label, at)| match &blocks[at] {
                        Block::Image { data, .. } => Some((label, data.clone())),
                        _ => None,
                    })
                    .collect();
                (text, pictures)
            }
            _ => (String::new(), Vec::new()),
        };
        let (cwd, scale, trace) = (
            self.header.cwd.clone(),
            self.scale.clone(),
            self.trace.clone(),
        );
        let takes = self.thread.capabilities.images;
        let composer = cx.new(|cx| {
            let mut composer = Composer::new(PLACEHOLDER.into(), cx);
            composer.set_inline(true);
            composer.set_scale(scale, trace);
            composer.set_project(Some(cwd));
            composer.set_takes_pictures(Some(takes));
            composer.set_commands_off(true, cx);
            composer.reuse(&text, pictures, cx);
            composer
        });
        let focus = composer.read(cx).focus_handle(cx);
        let subscriptions = vec![
            cx.subscribe_in(&composer, window, Self::editor_event),
            // What is typed changes the row's height: it is measured again.
            cx.observe(&composer, |this, _, cx| {
                if let Some(editor) = &this.editor {
                    let index = editor.index;
                    this.remeasure_rows(index..index + 1);
                    cx.notify();
                }
            }),
            // The frame follows the focus.
            cx.on_focus_in(&focus, window, |this, _, cx| this.redraw_editor(cx)),
            cx.on_focus_out(&focus, window, |this, _, _, cx| this.redraw_editor(cx)),
        ];
        Editor {
            index,
            composer,
            focus,
            _subscriptions: subscriptions,
        }
    }

    fn redraw_editor(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = &self.editor {
            let index = editor.index;
            self.remeasure_rows(index..index + 1);
            cx.notify();
        }
    }

    fn editor_event(
        &mut self,
        _: &Entity<Composer>,
        event: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::Submit(_) | ComposerEvent::Force(_) => self.run_edit(window, cx),
            // The editor has no history to walk, and nothing to run when
            // empty; what it holds is drawn as it changes (`make_editor`).
            ComposerEvent::Emptied(_) | ComposerEvent::Recall { .. } | ComposerEvent::Blank => {}
        }
    }

    /// Closes the editor and keeps the prompt as it was; with `focus_field`
    /// the keyboard goes to the field under the transcript.
    pub(crate) fn close_edit(
        &mut self,
        focus_field: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shut_edit(cx) && focus_field {
            self.focus_field(window, cx);
        }
    }

    /// Closes the editor, if one is open, and keeps the prompt as it was.
    pub(crate) fn shut_edit(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.editor.take() else {
            return false;
        };
        self.motion.edit_closed(cx.background_executor().now());
        self.remeasure_rows(editor.index..editor.index + 1);
        cx.notify();
        true
    }

    /// Closes an editor the session moved past. What was typed goes to the
    /// field under the transcript when that is empty, so it is not lost;
    /// with `always`, even when it is the prompt as it was. `message` says
    /// why, over the field.
    pub(crate) fn drop_edit(
        &mut self,
        message: Option<&str>,
        always: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.editor.take() else {
            return;
        };
        self.motion.edit_closed(cx.background_executor().now());
        self.remeasure_rows(editor.index..editor.index + 1);
        let typed = editor.composer.read(cx).text().trim().to_owned();
        let was = match self.thread.entries().get(editor.index) {
            Some(Entry::User(blocks)) => crate::draft::shown(blocks).0,
            _ => String::new(),
        };
        let dock_empty = self
            .dock
            .read(cx)
            .composer
            .read(cx)
            .text()
            .trim()
            .is_empty();
        let kept = !typed.is_empty() && (always || typed != was.trim()) && dock_empty;
        if kept {
            let pictures = editor
                .composer
                .read(cx)
                .referenced()
                .into_iter()
                .filter_map(|picture| match &picture.state {
                    crate::composer::PictureState::Ready(prepared) => {
                        Some((picture.label.clone(), prepared.data.clone()))
                    }
                    crate::composer::PictureState::Reading => None,
                })
                .collect();
            self.dock.update(cx, |dock, cx| {
                dock.composer
                    .update(cx, |composer, cx| composer.reuse(&typed, pictures, cx));
            });
        }
        if let Some(message) = message {
            let text = if kept {
                format!("{message} What you typed is in the field.")
            } else {
                message.to_owned()
            };
            self.dock.update(cx, |dock, cx| dock.tell(text, true, cx));
        }
        cx.notify();
    }

    /// Runs the edit: the thread lets go of the prompt and what followed it,
    /// shows the edited prompt in its place, and the backend is asked to
    /// rewind the session to before it and run the edited prompt.
    pub(crate) fn run_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        if let Some(blocked) = self.edit_blocked() {
            self.dock
                .update(cx, |dock, cx| dock.tell(blocked, true, cx));
            return;
        }
        let (draft, links) = match editor.composer.read(cx).draft() {
            Ok(draft) => draft,
            Err(reason) => {
                editor.composer.update(cx, |composer, cx| {
                    composer.tell(format!("Not yet: {reason}."), true, cx)
                });
                return;
            }
        };
        if draft.is_blank() {
            return;
        }
        let index = editor.index;
        let blocks = draft.blocks(
            &links,
            &self.header.cwd,
            self.thread.capabilities.embedded_files,
        );
        let rewinding = self.thread.rewind(index, blocks.clone());
        self.forget_folds();
        let Some(rewinding) = rewinding else {
            return;
        };
        let request = Request::Rewind {
            prompt: rewinding.prompt,
            blocks: blocks.clone(),
        };
        if self.requests.try_send(request).is_err() {
            // Nothing reached the view yet: the thread takes it all back.
            self.thread
                .apply(Update::Rewound(Err("the backend is gone".into())));
            self.dock.update(cx, |dock, cx| {
                dock.tell(
                    "The agent's backend is gone: the edit was not run.",
                    true,
                    cx,
                )
            });
            return;
        }
        let editor = self.editor.take().expect("an editor");
        let keep = |at: &usize| *at >= index;
        let sent = Sent {
            prompt_models: self
                .prompt_models
                .iter()
                .filter(|(at, _)| keep(at))
                .map(|(at, model)| (*at, model.clone()))
                .collect(),
            toggled: self
                .toggled
                .iter()
                .filter(|(at, _)| keep(at))
                .map(|(at, open)| (*at, *open))
                .collect(),
            unfolded: self
                .unfolded
                .iter()
                .filter(|(at, _)| keep(at))
                .copied()
                .collect(),
            shots: self
                .shots
                .iter()
                .filter(|((at, _), _)| keep(at))
                .map(|(at, shot)| (*at, shot.clone()))
                .collect(),
            editor,
        };
        // What it faded is gone: what comes next is not faded on its way back.
        self.motion.after_edit = None;
        // The edited prompt goes to the model the prompt went to.
        let model = self.prompt_models.get(&index).cloned();
        self.take_changes(rewinding.changes, cx);
        if let Some(model) = model {
            self.prompt_models.insert(index, model);
        }
        // Its pictures are the transcript's at once.
        for (at, block) in blocks.iter().enumerate() {
            let Block::Image { data, .. } = block else {
                continue;
            };
            if let Some(picture) = draft
                .pictures
                .iter()
                .find(|picture| std::sync::Arc::ptr_eq(&picture.prepared.data, data))
            {
                let prepared = &picture.prepared;
                self.shots.insert(
                    (index, at),
                    Shot::Ready {
                        thumb: prepared.thumb.clone(),
                        large: prepared.large.clone(),
                        width: prepared.width,
                        height: prepared.height,
                    },
                );
            }
        }
        self.sent_edit = Some(sent);
        self.list.scroll_to_end();
        self.focus_field(window, cx);
        cx.notify();
    }

    /// What the backend said of the edit on its way, after the thread took it
    /// in. A rewind that failed gave the entries back: the view gets back
    /// what it kept of them, and the editor opens again as it was.
    pub(crate) fn rewound(&mut self, result: &Result<String, String>, cx: &mut Context<Self>) {
        let Some(sent) = self.sent_edit.take() else {
            return;
        };
        let Err(reason) = result else {
            return;
        };
        let index = sent.editor.index;
        self.prompt_models.extend(sent.prompt_models);
        self.toggled.extend(sent.toggled);
        self.unfolded.extend(sent.unfolded);
        self.shots.extend(sent.shots);
        if matches!(self.thread.entries().get(index), Some(Entry::User(_))) && self.editor.is_none()
        {
            self.editor = Some(sent.editor);
            self.motion
                .edit_opened(index, cx.background_executor().now());
            self.remeasure_rows(index..index + 1);
            // The keyboard goes back to it once the view is drawn.
            self.focus_editor = true;
        }
        self.dock.update(cx, |dock, cx| {
            dock.tell(format!("The edit did not run: {reason}"), true, cx)
        });
    }

    /// Follows the session while an editor is open: an edit the session
    /// moved past, or whose prompt is gone, closes.
    pub(crate) fn follow_edit(&mut self, started: bool, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        if matches!(self.thread.state(), ThreadState::Closed(_)) {
            self.drop_edit(None, true, cx);
        } else if started
            || !matches!(
                self.thread.entries().get(editor.index),
                Some(Entry::User(_))
            )
        {
            self.drop_edit(
                Some("The session moved on, so the edit was closed."),
                false,
                cx,
            );
        }
    }

    /// The prompt's editor as its row draws it (`.edit`): the field with the
    /// prompt's pictures over it, what running it replaces, Cancel and Run.
    pub(crate) fn editor_element(
        &self,
        editor: &Editor,
        window: &Window,
        theme: &Theme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let index = editor.index;
        let focused = editor.focus.contains_focused(window, cx);
        let empty = editor.composer.read(cx).text().trim().is_empty();
        let blocked = self.edit_blocked();
        let note = note(self.thread.entries().len().saturating_sub(index + 1));
        let runs = !empty && blocked.is_none();
        // The quieter look's primary is the text's colour, as its answers' is.
        let studio = self.around.studio();
        let (accent, ink, frame, faint) = if studio {
            (theme.fg, theme.pane, theme.line_strong, theme.fg_faint)
        } else {
            (
                theme.primary,
                theme.primary_ink,
                theme.line_strong,
                theme.fg_faint,
            )
        };
        // The cockpit's run is the one primary action: its rim shows it on
        // the dark, as a primary button's does.
        let rim = if studio { accent } else { theme.primary_rim };
        // In the place of a prompt's bubble or card, as large a corner.
        let corner = metric::RADIUS_LARGE;
        let focus = editor.focus.clone();
        let cancel = div()
            .id(("edit-cancel", index))
            .debug_selector(|| "edit-cancel".into())
            .flex_none()
            .h(px(32.))
            .pl(px(12.))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded(px(metric::RADIUS))
            .border_1()
            .border_color(frame)
            .text_size(px(metric::TEXT_SMALL))
            .text_color(theme.fg_dim)
            .cursor_pointer()
            .hover(move |style| style.border_color(faint))
            .child(scale::text("Cancel"))
            .child(key_hint("Esc", theme))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.close_edit(true, window, cx);
            }));
        let run = div()
            .id(("edit-run", index))
            .debug_selector(|| "edit-run".into())
            .flex_none()
            .h(px(32.))
            .pl(px(16.))
            .pr(px(10.))
            .flex()
            .items_center()
            .gap(px(10.))
            .rounded(px(metric::RADIUS))
            .border_1()
            .font_family(theme.mono_font.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(11.))
            .child(scale::text(if studio { "Run" } else { "RUN" }))
            .child(
                div()
                    .flex_none()
                    .px(px(4.))
                    .rounded(px(metric::RADIUS_SMALL))
                    .border_1()
                    .border_color(if runs { ink.opacity(0.35) } else { frame })
                    .opacity(if runs { 0.75 } else { 1. })
                    .font_weight(FontWeight::NORMAL)
                    .text_size(px(LABEL))
                    .child(scale::text("↵")),
            );
        let run = if runs {
            let lit = crate::lights::mix(accent, theme.fg, 0.15);
            run.bg(accent)
                .border_color(rim)
                .text_color(ink)
                .cursor_pointer()
                .hover(move |style| style.bg(lit))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.run_edit(window, cx)))
        } else {
            run.border_color(frame).text_color(faint)
        };
        let frame = div()
            .id(("editor", index))
            .debug_selector(|| "editor".into())
            .w_full()
            .pl(px(15.))
            .pr(px(10.))
            .pt(px(11.))
            .pb(px(10.))
            .rounded(px(corner))
            .border_1()
            .border_color(if focused {
                crate::lights::mix(frame, accent, 0.45)
            } else {
                frame
            })
            .bg(theme.raised)
            .flex()
            .flex_col()
            // A press on the frame is the field's, not the pane's.
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&focus, cx);
                window.prevent_default();
            })
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                if let Some(editor) = &this.editor {
                    editor
                        .composer
                        .update(cx, |composer, cx| composer.drop_paths(paths, window, cx));
                }
            }))
            .child(editor.composer.clone())
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .debug_selector(|| "edit-note".into())
                            .flex_1()
                            .min_w(px(160.))
                            .font_family(theme.mono_font.clone())
                            .text_size(px(11.))
                            .text_color(theme.fg_muted)
                            .child(scale::text(note)),
                    )
                    .child(div().ml_auto().flex().gap(px(8.)).child(cancel).child(run)),
            );
        if cx.reduce_motion() {
            return frame.into_any_element();
        }
        frame
            .with_animation(
                ("editor-in", index),
                motion::enter(cockpit::EDITOR),
                |frame, t| frame.opacity(t),
            )
            .into_any_element()
    }

    /// The flag in the header of a prompt being edited (`.flag.editing`).
    pub(crate) fn editing_flag(theme: &Theme) -> impl IntoElement {
        div()
            .debug_selector(|| "editing-flag".into())
            .flex_none()
            .h(px(16.))
            .px(px(5.))
            .flex()
            .items_center()
            .rounded(px(metric::RADIUS_SMALL))
            .border_1()
            .border_color(theme.accent)
            .font_family(theme.mono_font.clone())
            .font_weight(FontWeight::BOLD)
            .text_size(px(LABEL))
            .text_color(theme.accent)
            .child(scale::text(SharedString::from("EDITING")))
    }
}
