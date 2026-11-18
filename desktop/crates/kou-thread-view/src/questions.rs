//! The questions an agent asks the user ([`kou_thread::Question`]), in the
//! card of the call that asks them, where a plain request has its buttons.
//!
//! [`Answering`] is how far the user has come with one request's questions:
//! which is at hand, what was answered, what is chosen of one that takes
//! several options. A click on an option, a digit pressed over the empty
//! field, Confirm, words sent from the field and Back move it on; once the
//! last question is answered the answers go to the backend together, as one
//! [`Command::Answer`], and the request is answered in the thread as a
//! permitted one is. [`Asks`] is what the card shows of it, the same for the
//! view's own drawing and for the app's (`chat-tool`'s `asks`).

use crate::{
    dock,
    entries::{META, answer_button, label, meta},
    view::ThreadView,
};
use gpui::{
    Context, Div, FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use kou_thread::{Command, PendingPermission, Question};
use kou_ui::{
    Icon, Theme, icon, key_hint, metric,
    scale::{self, px},
};
use std::collections::BTreeSet;

/// How far the user has come with the questions of one request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Answering {
    /// The answers given, one for each question answered so far, in order.
    answers: Vec<Vec<String>>,
    /// Of the question at hand, when it takes several options: those
    /// chosen, by their place.
    chosen: BTreeSet<usize>,
}

/// What a step of answering did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing: there was nothing to answer with, or nothing to answer.
    Nothing,
    /// The question at hand changed: an option chosen, or the next one.
    Moved,
    /// The last question is answered: these are the answers, one list for
    /// each question.
    Done(Vec<Vec<String>>),
}

impl Answering {
    /// The place of the question at hand among the request's questions.
    pub(crate) fn at(&self) -> usize {
        self.answers.len()
    }

    /// The answers given so far.
    pub(crate) fn answers(&self) -> &[Vec<String>] {
        &self.answers
    }

    /// Whether option `option` of the question at hand is chosen.
    pub(crate) fn is_chosen(&self, option: usize) -> bool {
        self.chosen.contains(&option)
    }

    /// Whether anything of the question at hand is chosen.
    pub(crate) fn has_chosen(&self) -> bool {
        !self.chosen.is_empty()
    }

    /// Option `option` of the question at hand: a question that takes one
    /// is answered with it; one that takes several has it chosen, or no
    /// longer chosen.
    pub(crate) fn choose(&mut self, questions: &[Question], option: usize) -> Step {
        let Some(question) = questions.get(self.at()) else {
            return Step::Nothing;
        };
        let Some(picked) = question.options.get(option) else {
            return Step::Nothing;
        };
        if question.multiple {
            if !self.chosen.remove(&option) {
                self.chosen.insert(option);
            }
            return Step::Moved;
        }
        let answer = vec![picked.label.clone()];
        self.answer(questions, answer)
    }

    /// The question at hand answered with what is chosen of it, and the
    /// user's own `words` after, where it takes them. Nothing with neither.
    pub(crate) fn confirm(&mut self, questions: &[Question], words: Option<&str>) -> Step {
        let Some(question) = questions.get(self.at()) else {
            return Step::Nothing;
        };
        let words = words
            .map(str::trim)
            .filter(|words| !words.is_empty() && takes_words(question));
        let mut answer: Vec<String> = if question.multiple {
            self.chosen
                .iter()
                .filter_map(|at| question.options.get(*at))
                .map(|option| option.label.clone())
                .collect()
        } else {
            Vec::new()
        };
        answer.extend(words.map(str::to_owned));
        if answer.is_empty() {
            return Step::Nothing;
        }
        self.answer(questions, answer)
    }

    fn answer(&mut self, questions: &[Question], answer: Vec<String>) -> Step {
        self.answers.push(answer);
        self.chosen.clear();
        if self.at() >= questions.len() {
            Step::Done(std::mem::take(&mut self.answers))
        } else {
            Step::Moved
        }
    }

    /// Back to the question before, its answer undone; of one that takes
    /// several options, those it was answered with are chosen again.
    /// Whether there was one.
    pub(crate) fn back(&mut self, questions: &[Question]) -> bool {
        let Some(answer) = self.answers.pop() else {
            return false;
        };
        self.chosen.clear();
        if let Some(question) = questions.get(self.at())
            && question.multiple
        {
            for label in &answer {
                if let Some(at) = question
                    .options
                    .iter()
                    .position(|option| &option.label == label)
                {
                    self.chosen.insert(at);
                }
            }
        }
        true
    }
}

/// Whether a question takes an answer in the user's own words: one that
/// says so, and one with no options, which can be answered no other way.
pub(crate) fn takes_words(question: &Question) -> bool {
    question.free || question.options.is_empty()
}

/// What a question is called on the line of what was answered: its header,
/// or else the first line of the question.
fn name_of(question: &Question) -> &str {
    question
        .header
        .as_deref()
        .map(str::trim)
        .filter(|header| !header.is_empty())
        .unwrap_or_else(|| question.text.lines().next().unwrap_or_default().trim())
}

/// The line under a question that says an answer can be typed in the
/// field.
fn words_line(question: &Question) -> &'static str {
    if question.options.is_empty() {
        "Type your answer in the field below"
    } else if question.multiple {
        "Or add your own words in the field below"
    } else {
        "Or type your own answer in the field below"
    }
}

/// What the card of a call shows of the questions it asks, as the view
/// draws it and as the app's drawing is told it (`asks`): the question at
/// hand (`at`, counted from 0, of `count`), its options (`key`: the digit
/// that chooses it, while it is the question the field answers), the line
/// that says words can be typed (`words`), what was answered before it, a
/// line each, whether Back is there, of a question that takes several
/// options whether its Confirm is live, and the ways to refuse.
#[derive(Hash, serde::Serialize)]
pub(crate) struct Asks<'a> {
    pub request: u64,
    pub at: usize,
    pub count: usize,
    pub header: Option<&'a str>,
    pub text: &'a str,
    pub multiple: bool,
    pub words: Option<&'static str>,
    pub options: Vec<AskOption<'a>>,
    pub answered: Vec<Answered<'a>>,
    pub back: bool,
    pub confirm: Option<bool>,
    pub refusals: Vec<Refusal<'a>>,
}

#[derive(Hash, serde::Serialize)]
pub(crate) struct AskOption<'a> {
    pub label: &'a str,
    pub description: Option<&'a str>,
    pub chosen: bool,
    pub key: Option<String>,
}

#[derive(Hash, serde::Serialize)]
pub(crate) struct Answered<'a> {
    pub name: &'a str,
    pub answer: String,
}

#[derive(Hash, serde::Serialize)]
pub(crate) struct Refusal<'a> {
    pub id: &'a str,
    pub label: &'a str,
}

impl ThreadView {
    /// The request whose question the field answers: the oldest that asks
    /// any, with the question at hand.
    pub(crate) fn question_at_hand(&self) -> Option<(&PendingPermission, &Question)> {
        self.thread.permissions().iter().find_map(|pending| {
            let at = self
                .answering
                .get(&pending.request)
                .map_or(0, Answering::at);
            pending
                .questions
                .get(at)
                .map(|question| (pending, question))
        })
    }

    /// The question at hand as the field needs to know it.
    pub(crate) fn asking_for_field(&self) -> Option<dock::Asking> {
        self.question_at_hand().map(|(_, question)| dock::Asking {
            words: takes_words(question),
            multiple: question.multiple,
        })
    }

    /// What the card of `pending` shows of its questions; `None` for a
    /// plain request.
    pub(crate) fn asks_of<'a>(&'a self, pending: &'a PendingPermission) -> Option<Asks<'a>> {
        let answering = self.answering.get(&pending.request);
        let at = answering.map_or(0, Answering::at);
        let question = pending.questions.get(at)?;
        let at_hand = self
            .question_at_hand()
            .is_some_and(|(asking, _)| asking.request == pending.request);
        let words = takes_words(question);
        let chosen = |option: usize| answering.is_some_and(|answering| answering.is_chosen(option));
        Some(Asks {
            request: pending.request,
            at,
            count: pending.questions.len(),
            header: question
                .header
                .as_deref()
                .map(str::trim)
                .filter(|header| !header.is_empty()),
            text: question.text.trim(),
            multiple: question.multiple,
            words: words.then(|| words_line(question)),
            options: question
                .options
                .iter()
                .enumerate()
                .map(|(place, option)| AskOption {
                    label: &option.label,
                    description: option
                        .description
                        .as_deref()
                        .map(str::trim)
                        .filter(|description| !description.is_empty()),
                    chosen: question.multiple && chosen(place),
                    key: (at_hand && place < 9).then(|| (place + 1).to_string()),
                })
                .collect(),
            answered: answering
                .map(|answering| {
                    answering
                        .answers()
                        .iter()
                        .zip(&pending.questions)
                        .map(|(answer, question)| Answered {
                            name: name_of(question),
                            answer: answer.join(", "),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            back: at > 0,
            confirm: question.multiple.then(|| {
                answering.is_some_and(Answering::has_chosen)
                    || (words && at_hand && self.field_typed)
            }),
            refusals: pending
                .options
                .iter()
                .map(|option| Refusal {
                    id: &option.id,
                    label: &option.label,
                })
                .collect(),
        })
    }

    /// Option `option` of the question at hand of request `request`,
    /// chosen: a click on it, or its digit.
    pub(crate) fn choose_answer(&mut self, request: u64, option: usize, cx: &mut Context<Self>) {
        let Some(questions) = self.questions_of(request) else {
            return;
        };
        let step = self
            .answering
            .entry(request)
            .or_default()
            .choose(&questions, option);
        self.take_step(request, step, cx);
    }

    /// Confirm on request `request`'s question at hand: what is chosen, and
    /// the words typed in the field, where it takes them.
    pub(crate) fn confirm_answer(&mut self, request: u64, cx: &mut Context<Self>) {
        let Some(questions) = self.questions_of(request) else {
            return;
        };
        let at = self.answering.get(&request).map_or(0, Answering::at);
        let words_taken = questions.get(at).is_some_and(takes_words)
            && self
                .question_at_hand()
                .is_some_and(|(pending, _)| pending.request == request);
        // (Words alone answer a question that takes several, too.)
        let words = if words_taken {
            self.dock.update(cx, |dock, cx| dock.take_words(cx))
        } else {
            None
        };
        let step = self
            .answering
            .entry(request)
            .or_default()
            .confirm(&questions, words.as_deref());
        self.take_step(request, step, cx);
    }

    /// Words sent from the field while a question that takes them is at
    /// hand: its answer, with what is chosen of it.
    pub(crate) fn answer_in_words(&mut self, words: String, cx: &mut Context<Self>) {
        let Some(request) = self.question_at_hand().map(|(pending, _)| pending.request) else {
            return;
        };
        let Some(questions) = self.questions_of(request) else {
            return;
        };
        let step = self
            .answering
            .entry(request)
            .or_default()
            .confirm(&questions, Some(&words));
        self.take_step(request, step, cx);
    }

    /// Enter over the empty field: a question at hand that takes several
    /// options is answered with those chosen.
    pub(crate) fn answer_blank(&mut self, cx: &mut Context<Self>) {
        let Some((request, multiple)) = self
            .question_at_hand()
            .map(|(pending, question)| (pending.request, question.multiple))
        else {
            return;
        };
        if multiple {
            self.confirm_answer(request, cx);
        }
    }

    /// Back on request `request`'s card: the question before is at hand
    /// again.
    pub(crate) fn answer_back(&mut self, request: u64, cx: &mut Context<Self>) {
        let Some(questions) = self.questions_of(request) else {
            return;
        };
        if self
            .answering
            .get_mut(&request)
            .is_some_and(|answering| answering.back(&questions))
        {
            self.take_step(request, Step::Moved, cx);
        }
    }

    /// A digit pressed over the empty field, while a question with options
    /// is at hand: the option of that number is chosen, and the digit is
    /// not typed.
    pub(crate) fn question_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.modified() || event.is_held {
            return;
        }
        let Some(digit) = keystroke
            .key
            .parse::<usize>()
            .ok()
            .filter(|digit| (1..=9).contains(digit))
        else {
            return;
        };
        let Some((request, options)) = self
            .question_at_hand()
            .map(|(pending, question)| (pending.request, question.options.len()))
        else {
            return;
        };
        if digit > options || !self.field_focus().is_focused(window) || self.editor.is_some() {
            return;
        }
        let dock = self.dock.read(cx);
        if !dock.field_empty() || dock.composer.read(cx).menu_open() {
            return;
        }
        cx.stop_propagation();
        self.choose_answer(request, digit - 1, cx);
    }

    /// The field came to hold words, or none, while a question that takes
    /// them is at hand: its Confirm may be live now, or no longer.
    pub(crate) fn field_typed(&mut self, typed: bool, cx: &mut Context<Self>) {
        if self.field_typed == typed {
            return;
        }
        self.field_typed = typed;
        if let Some(entry) = self.question_at_hand().map(|(pending, _)| pending.entry) {
            self.remeasure_rows(entry..entry + 1);
            cx.notify();
        }
    }

    /// Lets go of what was answered of requests that are no longer open:
    /// answered, refused, taken back, or ended with their turn.
    pub(crate) fn forget_answering(&mut self) {
        let open = self.thread.permissions();
        self.answering
            .retain(|request, _| open.iter().any(|pending| pending.request == *request));
    }

    /// The questions request `request` asks, while it is open.
    fn questions_of(&self, request: u64) -> Option<Vec<Question>> {
        self.thread
            .permissions()
            .iter()
            .find(|pending| pending.request == request && !pending.questions.is_empty())
            .map(|pending| pending.questions.clone())
    }

    /// Shows what a step did: the card as it stands now, and the field
    /// under the question at hand; or, after the last question, the
    /// answers sent.
    fn take_step(&mut self, request: u64, step: Step, cx: &mut Context<Self>) {
        match step {
            Step::Nothing => {}
            Step::Moved => {
                if let Some(entry) = self.request_entry(request) {
                    self.remeasure_rows(entry..entry + 1);
                }
                self.sync_dock(cx);
                cx.notify();
            }
            Step::Done(answers) => self.send_answers(request, answers, cx),
        }
    }

    /// The row of the call request `request` asks about, while it is open.
    fn request_entry(&self, request: u64) -> Option<usize> {
        self.thread
            .permissions()
            .iter()
            .find(|pending| pending.request == request)
            .map(|pending| pending.entry)
    }

    /// Sends the answers to request `request`'s questions, and answers it
    /// in the thread as a permitted request is: its card closes and the
    /// turn goes on.
    fn send_answers(&mut self, request: u64, answers: Vec<Vec<String>>, cx: &mut Context<Self>) {
        let Some(entry) = self.request_entry(request) else {
            return;
        };
        if self
            .commands
            .try_send(Command::Answer { request, answers })
            .is_err()
        {
            return;
        }
        self.answering.remove(&request);
        let state = self.thread.state().clone();
        self.thread.answered(request);
        let count = self.thread.entries().len();
        self.remeasure_rows(entry..entry + 1);
        self.remeasure_rows(count..count + 1);
        self.sync_dock(cx);
        self.after_change(state, self.thread.title.clone(), cx);
    }

    /// The questions of a call's card, as the view draws them: in the band
    /// where a plain request has its buttons (`.tool-ask`), what was
    /// answered before, a line each; the question at hand under its header
    /// with its options as rows (a mark, the label, the description under
    /// it, the digit that chooses it); the line that says words can be
    /// typed; and Confirm and the refusals, as the request's buttons are.
    pub(crate) fn question_block(&self, asks: &Asks, theme: &Theme, cx: &Context<Self>) -> Div {
        let request = asks.request;
        let (hover, ring) = (theme.hover, theme.accent);
        // The plan's mark of a step: 14 wide, the mark in its middle.
        let cell = || {
            div()
                .flex_none()
                .w(px(14.))
                .flex()
                .items_center()
                .justify_center()
        };
        let answered = asks.answered.iter().map(|answered| {
            div()
                .h(px(META))
                .flex()
                .items_center()
                .gap(px(10.))
                .font_family(theme.mono_font.clone())
                .text_size(px(11.))
                .child(cell().child(icon(Icon::Check, theme.ok).size(px(12.))))
                .child(
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .truncate()
                        .text_color(theme.fg_muted)
                        .child(scale::text(answered.name.to_owned())),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_shrink(1.)
                        .truncate()
                        .text_color(theme.fg_dim)
                        .child(scale::text(answered.answer.clone())),
                )
        });
        let head = div()
            .h(px(META))
            .flex()
            .items_center()
            .gap(px(10.))
            .child(label(
                asks.header.unwrap_or("Question").to_owned(),
                theme.fg_dim,
                theme,
            ))
            .children((asks.count > 1).then(|| {
                meta(theme).flex_none().child(scale::text(format!(
                    "{} of {}",
                    asks.at + 1,
                    asks.count
                )))
            }))
            .child(div().flex_1())
            .children(asks.back.then(|| {
                crate::prose::act("ask-back", "Back", theme)
                    .debug_selector(|| "ask-back".into())
                    .on_click(cx.listener(move |this, click, window, cx| {
                        this.answer_back(request, cx);
                        this.rest_focus(click, window, cx);
                    }))
            }));
        let text = div()
            .text_size(px(13.5))
            .line_height(px(22.))
            .text_color(theme.fg)
            .child(scale::text(asks.text.to_owned()));
        let options = asks.options.iter().enumerate().map(|(place, option)| {
            let mark = if option.chosen {
                icon(Icon::Check, theme.accent)
                    .size(px(12.))
                    .into_any_element()
            } else {
                div()
                    .size(px(8.))
                    .rounded(px(metric::RADIUS_MARK))
                    .border_1()
                    .border_color(theme.fg_faint)
                    .into_any_element()
            };
            div()
                .id(("ask-option", place))
                .debug_selector(move || format!("ask-option-{place}"))
                // Its frame lies outside the column, its words in it.
                .mx(px(-7.))
                .px(px(6.))
                .flex()
                .items_start()
                .gap(px(10.))
                .rounded(px(metric::RADIUS))
                .border_1()
                .border_color(gpui::transparent_black())
                .cursor_pointer()
                .tab_index(0)
                .hover(move |style| style.bg(hover))
                .focus_visible(move |style| style.border_color(ring))
                .on_click(cx.listener(move |this, click, window, cx| {
                    this.choose_answer(request, place, cx);
                    this.rest_focus(click, window, cx);
                }))
                .child(cell().h(px(22.)).child(mark))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_size(px(13.5))
                                .line_height(px(22.))
                                .text_color(theme.fg)
                                .child(scale::text(option.label.to_owned())),
                        )
                        .children(option.description.map(|description| {
                            div()
                                .pb(px(4.))
                                .font_family(theme.mono_font.clone())
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .text_color(theme.fg_muted)
                                .child(scale::text(description.to_owned()))
                        })),
                )
                .children(option.key.clone().map(|key| {
                    div()
                        .flex_none()
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .child(key_hint(key, theme))
                }))
        });
        let words = asks.words.map(|words| {
            div()
                .h(px(22.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(cell().child(icon(Icon::Keyboard, theme.fg_muted).size(px(14.))))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme.mono_font.clone())
                        .text_size(px(11.))
                        .text_color(theme.fg_muted)
                        .child(scale::text(words)),
                )
        });
        let buttons = (asks.confirm.is_some() || !asks.refusals.is_empty()).then(|| {
            div()
                .mt(px(8.))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .children(asks.confirm.map(|live| {
                    if live {
                        answer_button("ask-confirm", "Confirm".to_owned(), true, theme)
                            .debug_selector(|| "ask-confirm".into())
                            .on_click(cx.listener(move |this, click, window, cx| {
                                this.confirm_answer(request, cx);
                                this.rest_focus(click, window, cx);
                            }))
                            .into_any_element()
                    } else {
                        confirm_unready(theme).into_any_element()
                    }
                }))
                .children(asks.refusals.iter().enumerate().map(|(place, refusal)| {
                    let id = refusal.id.to_owned();
                    let selector = format!("answer-{id}");
                    answer_button(("answer", place), refusal.label.to_owned(), false, theme)
                        .debug_selector(move || selector.clone())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.permit(request, id.clone(), cx);
                            // The buttons are gone with the answer.
                            this.focus_field(window, cx);
                        }))
                }))
                .children(asks.confirm.filter(|live| *live).map(|_| {
                    div()
                        .ml(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .font_family(theme.mono_font.clone())
                        .text_size(px(10.5))
                        .text_color(theme.fg_muted)
                        .child(key_hint("enter", theme))
                        .child(scale::text("confirm"))
                }))
        });
        div()
            .w_full()
            .px(px(12.))
            .py(px(10.))
            .border_t_1()
            .border_color(theme.blocked.opacity(0.35))
            .bg(theme.blocked.opacity(0.06))
            .flex()
            .flex_col()
            .children(answered)
            .child(head)
            .child(text)
            .child(
                div()
                    .mt(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(options)
                    .children(words),
            )
            .children(buttons)
    }
}

/// Confirm while nothing is chosen or typed: there, but not to be pressed,
/// as the field's own button is with nothing to send.
fn confirm_unready(theme: &Theme) -> Div {
    div()
        .debug_selector(|| "ask-confirm".into())
        .flex_none()
        .h(px(28.))
        .px(px(12.))
        .rounded(px(metric::RADIUS))
        .border_1()
        .border_color(theme.line)
        .flex()
        .items_center()
        .font_family(theme.mono_font.clone())
        .font_weight(FontWeight::BOLD)
        .text_size(px(10.5))
        .text_color(theme.fg_faint)
        .child(scale::text("CONFIRM"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kou_thread::QuestionOption;

    fn question(options: &[&str], multiple: bool, free: bool) -> Question {
        Question {
            header: None,
            text: "Which?".into(),
            options: options
                .iter()
                .map(|label| QuestionOption {
                    label: (*label).into(),
                    description: None,
                })
                .collect(),
            multiple,
            free,
        }
    }

    fn answers(lists: &[&[&str]]) -> Vec<Vec<String>> {
        lists
            .iter()
            .map(|list| list.iter().map(|label| (*label).to_owned()).collect())
            .collect()
    }

    #[test]
    fn a_question_that_takes_one_option_is_answered_by_it_and_the_last_ends_the_asking() {
        let questions = [
            question(&["a", "b"], false, false),
            question(&["c"], false, false),
        ];
        let mut answering = Answering::default();
        assert_eq!(answering.choose(&questions, 1), Step::Moved);
        assert_eq!(answering.at(), 1);
        // No such option: nothing.
        assert_eq!(answering.choose(&questions, 5), Step::Nothing);
        assert_eq!(
            answering.choose(&questions, 0),
            Step::Done(answers(&[&["b"], &["c"]]))
        );
    }

    #[test]
    fn options_of_a_question_that_takes_several_are_toggled_and_confirmed_in_their_order() {
        let questions = [question(&["a", "b", "c"], true, true)];
        let mut answering = Answering::default();
        // Nothing chosen, nothing typed: Confirm does nothing.
        assert_eq!(answering.confirm(&questions, None), Step::Nothing);
        for option in [2, 0, 1, 1] {
            assert_eq!(answering.choose(&questions, option), Step::Moved);
        }
        assert!(answering.is_chosen(0) && answering.is_chosen(2) && !answering.is_chosen(1));
        // The words typed go after what is chosen.
        assert_eq!(
            answering.confirm(&questions, Some("  and d ")),
            Step::Done(answers(&[&["a", "c", "and d"]]))
        );
    }

    #[test]
    fn words_answer_only_a_question_that_takes_them() {
        let closed = [question(&["a"], false, false)];
        let mut answering = Answering::default();
        assert_eq!(answering.confirm(&closed, Some("mine")), Step::Nothing);
        // One with no options takes words though it does not say so.
        for free in [question(&["a"], false, true), question(&[], false, false)] {
            let mut answering = Answering::default();
            assert_eq!(
                answering.confirm(std::slice::from_ref(&free), Some("mine")),
                Step::Done(answers(&[&["mine"]]))
            );
        }
        // Blank words are none.
        let free = [question(&[], false, true)];
        assert_eq!(
            Answering::default().confirm(&free, Some("  ")),
            Step::Nothing
        );
    }

    #[test]
    fn back_undoes_the_answer_before_and_chooses_again_what_it_chose() {
        let questions = [
            question(&["a", "b", "c"], true, false),
            question(&["x"], false, false),
            question(&["y"], false, false),
        ];
        let mut answering = Answering::default();
        assert!(!answering.back(&questions), "nothing to go back to");
        answering.choose(&questions, 0);
        answering.choose(&questions, 2);
        assert_eq!(answering.confirm(&questions, None), Step::Moved);
        assert_eq!(answering.choose(&questions, 0), Step::Moved);
        assert!(answering.back(&questions));
        assert!(answering.back(&questions));
        assert_eq!(answering.at(), 0);
        assert!(answering.is_chosen(0) && answering.is_chosen(2));
        assert_eq!(answering.choose(&questions, 2), Step::Moved);
        assert_eq!(answering.confirm(&questions, None), Step::Moved);
        assert_eq!(answering.answers(), answers(&[&["a"]]));
    }

    #[test]
    fn a_question_is_named_by_its_header_or_its_first_line() {
        let mut asked = question(&[], false, true);
        asked.text = "Which store?\nIt matters.".into();
        assert_eq!(name_of(&asked), "Which store?");
        asked.header = Some(" Store ".into());
        assert_eq!(name_of(&asked), "Store");
        asked.header = Some(" ".into());
        assert_eq!(name_of(&asked), "Which store?");
    }
}
