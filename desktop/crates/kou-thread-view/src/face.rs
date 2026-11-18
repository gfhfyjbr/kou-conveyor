//! Parts of a chat that the app draws itself, from what the view hands it:
//! which of the app's drawings a part is, the data it is drawn from, and the
//! pieces that stay the view's own (a meter that moves on its own, rows that
//! take the keys). What a drawing asks for when it is used comes back to the
//! view by name. The prompt field's frame has a hook of its own
//! ([`crate::FieldFace`]); this one is for the rest of the chat.
//!
//! A part the app does not draw (it has no drawing for it, or its drawing
//! failed) is the view's own, drawn as it always was.

use crate::trace::{self, Trace};
use gpui::{AnyElement, App, Context, SharedString, WeakEntity, Window};
use serde_json::Value;
use std::{
    hash::{Hash, Hasher},
    rc::Rc,
};

/// Does what a drawing of the app's asked, in order, after one of its
/// handlers ran: each thing by its name, with what the drawing gave. It is
/// called with nothing when the handler only kept something of its own; the
/// part is drawn again after it either way.
pub type ChatAct = Rc<dyn Fn(Vec<(String, Value)>, &mut Window, &mut App)>;

/// A part of a chat, for the app to draw.
pub struct ChatPart<'a> {
    /// Which drawing of the app's it is: the name of a plugin's view, as
    /// `composer-usage` (see the app's plugins).
    pub view: &'static str,
    /// Which one of the things that view draws: the same for the part's
    /// life, and told apart from every other part on screen.
    pub key: SharedString,
    /// A digest of everything [`Self::data`] is made from, where the view
    /// works one out: the same digest for the same part is the same data,
    /// and the app may draw the part as it last did without making the
    /// data at all. A row of the transcript is drawn at every frame it is
    /// on screen, and nearly always as it was. `None`: no digest, the data
    /// is made every time.
    pub memo: Option<u64>,
    /// What it is drawn from, made when the app asks for it. Kept small and
    /// steady: the app draws a part again only when this changes, so
    /// nothing that streams goes in it.
    pub data: Box<dyn FnOnce() -> Value + 'a>,
    /// The view's own pieces by name, each to be placed once.
    pub slots: Vec<(&'static str, AnyElement)>,
    pub act: ChatAct,
}

/// Draws a part of a chat in place of the view's own drawing of it; `None`
/// leaves it to the view.
pub type ChatFace = Rc<dyn Fn(ChatPart, &App) -> Option<AnyElement>>;

/// What a part asks of the view that drew it: the thing asked, by name,
/// with what the drawing gave.
pub(crate) type Asked<T> = fn(&mut T, &str, &Value, &mut Window, &mut Context<T>);

/// A part of a chat as the view hands it over: which drawing, which one of
/// its things, the digest of its data where there is one, and its data.
pub(crate) struct Handed<'a> {
    pub view: &'static str,
    pub key: SharedString,
    pub memo: Option<u64>,
    pub data: Box<dyn FnOnce() -> Value + 'a>,
}

impl<'a> Handed<'a> {
    /// Part `view`, known as `key`, drawn from `data`, which is made anew
    /// every time it is drawn.
    pub fn new(
        view: &'static str,
        key: impl Into<SharedString>,
        data: impl FnOnce() -> Value + 'a,
    ) -> Self {
        Self {
            view,
            key: key.into(),
            memo: None,
            data: Box::new(data),
        }
    }

    /// Part `view`, known as `key`, drawn from `what` as the drawing reads
    /// it: its digest is of the very values its data is written from.
    pub fn of(
        view: &'static str,
        key: impl Into<SharedString>,
        what: impl Hash + serde::Serialize + 'a,
    ) -> Self {
        let memo = digest(&what);
        let mut part = Self::new(view, key, move || {
            serde_json::to_value(&what).unwrap_or(Value::Null)
        });
        part.memo = Some(memo);
        part
    }

    /// The same, `data` being made only from what `made_from` holds (see
    /// [`ChatPart::memo`]): everything that goes into it, and nothing else
    /// that could change it.
    pub fn kept(mut self, made_from: impl Hash) -> Self {
        self.memo = Some(digest(made_from));
        self
    }
}

/// A digest of `what`, the same for the same values.
pub(crate) fn digest(what: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    what.hash(&mut hasher);
    hasher.finish()
}

/// `face`'s drawing of `part` of `entity`; what it asks for is done by
/// `asked`, after which `entity` is drawn again. The time it took is noted
/// in `trace`, from its data being made to the drawing handed back.
pub(crate) fn drawn<T: 'static>(
    face: &ChatFace,
    part: Handed,
    slots: Vec<(&'static str, AnyElement)>,
    entity: WeakEntity<T>,
    asked: Asked<T>,
    trace: &Trace,
    cx: &App,
) -> Option<AnyElement> {
    let started = trace::face_started(trace);
    let act: ChatAct = Rc::new(move |acts, window, cx| {
        entity
            .update(cx, |this, cx| {
                for (name, given) in &acts {
                    asked(this, name, given, window, cx);
                }
                cx.notify();
            })
            .ok();
    });
    let drawn = face(
        ChatPart {
            view: part.view,
            key: part.key,
            memo: part.memo,
            data: part.data,
            slots,
            act,
        },
        cx,
    );
    trace::face_drawn(trace, started);
    drawn
}

/// A colour as it goes into a digest: exactly, bit for bit.
pub(crate) fn bits(color: gpui::Hsla) -> [u32; 4] {
    [color.h, color.s, color.l, color.a].map(f32::to_bits)
}

/// A colour as a drawing names it: `#rrggbb`, or `#rrggbbaa` when it is not
/// opaque.
pub(crate) fn hex(color: gpui::Hsla) -> String {
    let rgba = gpui::Rgba::from(color);
    let byte = |part: f32| (part.clamp(0., 1.) * 255.).round() as u8;
    let (r, g, b, a) = (byte(rgba.r), byte(rgba.g), byte(rgba.b), byte(rgba.a));
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}
