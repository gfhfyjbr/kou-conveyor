//! The native agent pane: a kou-thread conversation and its composer, drawn by GPUI.
//!
//! [`ThreadView`] is the whole of it: give it a [`kou_thread::Backend`] and it
//! folds the backend's updates into a [`kou_thread::Thread`], draws the
//! transcript and the prompt field, and sends the user's commands back.
//! [`PromptField`] is its prompt field alone, for a screen with no
//! conversation yet: it hands what is sent to the app as a [`Prompt`].
//!
//! Inside: [`markdown`], [`diff`], [`terminal`], [`buffer`], [`commands`],
//! [`draft`], [`pictures`], [`history`], [`queue`] and [`highlight`] are plain
//! data in, plain data out, and carry the tests for what they compute;
//! [`prose`], [`entries`], [`composer`], [`dock`] and [`view`] turn that data
//! into elements; [`questions`] holds both for the questions an agent asks
//! the user.
//!
//! The view has two looks ([`Look`]): the cockpit's, after kou's web
//! cockpit, and a quieter one for a chat with a harness ([`studio`]). In
//! either, [`nested`] keeps what the app tells it of the agents its agent
//! started and of what stands beside the conversation ([`Nested`],
//! [`Beside`]), and [`cards`] draws them: a card for each in the
//! transcript, the card beside it, the ticks down its left edge.

mod buffer;
mod cards;
mod commands;
mod composer;
mod diff;
mod dock;
mod draft;
mod edit;
mod entries;
mod face;
mod field;
mod highlight;
mod history;
mod lights;
mod links;
mod markdown;
mod models;
mod motion;
mod nested;
mod picker;
mod pictures;
mod prose;
mod questions;
mod queue;
mod studio;
mod terminal;
#[cfg(test)]
mod tests;
mod trace;
mod view;

pub use face::{ChatAct, ChatFace, ChatPart};
pub use field::{
    FieldAct, FieldDrop, FieldFace, FieldParts, Prompt, PromptField, PromptFieldEvent,
};
pub use nested::{Beside, Look, Nested, NestedState};
pub use view::{
    Header, ThreadView, ThreadViewEvent, ThreadViewRequest, ToolAids, ToolFace, ToolView,
};
