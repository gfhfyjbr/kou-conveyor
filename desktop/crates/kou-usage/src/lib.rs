//! What an account has used of its plan's limits, asked of the agent's own
//! program: the app holds no login and calls no service itself.
//!
//! Each agent is asked the way it offers, without a prompt and so without
//! spending anything:
//!
//! - Claude Code, over its control protocol (the `get_usage` request behind
//!   its `/usage` screen), in a session that is given nothing to do and is
//!   kept off disk. Claude Code marks the request experimental; an answer
//!   this crate cannot read is reported as unavailable, never guessed at.
//! - Codex, through `codex app-server` (`account/rateLimits/read`), without
//!   starting a thread.
//! - Grok Build, with `grok usage --json`, which answers only while its
//!   background service runs.
//!
//! An account is a program and the environment it is run in: a second
//! Claude Code account is the same program with `CLAUDE_CONFIG_DIR` set
//! (see [`home_variable`]). [`combine`] puts the accounts of one provider
//! together, window by window.

mod claude;
mod codex;
mod grok;
mod run;
mod when;

use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

pub use when::{parse_time, until};

/// How to ask: the agent's program, what its environment gets on top of the
/// app's own (an account's home), and how long to wait for the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub program: PathBuf,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
}

/// Which window of a plan a limit counts over.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// The short rolling window: five hours, where a plan has one.
    Session,
    /// The week, over every model.
    Weekly,
    /// The week, for one model or family, by the name the service gives it.
    Model(String),
    /// A window this crate has no name for, by the service's own.
    Other(String),
}

impl Kind {
    /// What it is called on screen.
    pub fn label(&self) -> String {
        match self {
            Kind::Session => "5h".to_owned(),
            Kind::Weekly => "Weekly".to_owned(),
            Kind::Model(model) => format!("{model} weekly"),
            Kind::Other(name) => name.clone(),
        }
    }

    /// How long the window is, where that is known by what it is: five
    /// hours, or a week. With when it starts over, this says how much of
    /// its time is left.
    pub fn length(&self) -> Option<Duration> {
        match self {
            Kind::Session => Some(Duration::from_secs(5 * 3600)),
            Kind::Weekly | Kind::Model(_) => Some(Duration::from_secs(7 * 24 * 3600)),
            Kind::Other(_) => None,
        }
    }
}

/// One limit: how much of a window is used, and when the window starts over.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub kind: Kind,
    /// The share used, from 0 to 100.
    pub used: f32,
    pub resets: Option<SystemTime>,
}

/// An account's limits as its agent reports them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Limits {
    /// The plan's name, as the service has it: `Max`, `Plus`.
    pub plan: Option<String>,
    /// In the order they are shown: the short window, the week, then the
    /// week by model.
    pub windows: Vec<Window>,
}

/// Why there are no limits to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The login is not a plan's: an API key, or another provider.
    NoPlan,
    /// The agent's sign-in was refused; what to do about it.
    SignIn(String),
    /// The agent does not say here and now; in its own words.
    NotHere(String),
    /// Asking failed, or the answer could not be read.
    Failed(String),
}

impl Reason {
    /// One line for the place the limits would be.
    pub fn message(&self) -> String {
        match self {
            Reason::NoPlan => {
                "This login has no plan limits (an API key or another provider)".into()
            }
            Reason::SignIn(what) | Reason::NotHere(what) | Reason::Failed(what) => what.clone(),
        }
    }
}

/// What asking came to.
pub type Answer = Result<Limits, Reason>;

/// Whether `harness` (an id of `kou-agents`) can be asked for its limits.
pub fn asks(harness: &str) -> bool {
    matches!(harness, "claude" | "codex" | "grok")
}

/// The environment variable that moves `harness`'s home, and with it its
/// login, sessions and settings, to another folder: what makes a second
/// account of the same program. `None` for a harness without one this crate
/// knows of.
pub fn home_variable(harness: &str) -> Option<&'static str> {
    match harness {
        "claude" => Some("CLAUDE_CONFIG_DIR"),
        "codex" => Some("CODEX_HOME"),
        "grok" => Some("GROK_HOME"),
        _ => None,
    }
}

/// Ask `harness` for its account's limits. Blocks for at most the ask's
/// timeout and a moment: call it off the main thread. `None` for a harness
/// that cannot be asked.
pub fn limits(harness: &str, ask: &Ask) -> Option<Answer> {
    Some(match harness {
        "claude" => claude::limits(ask),
        "codex" => codex::limits(ask),
        "grok" => grok::limits(ask),
        _ => return None,
    })
}

/// A window over several accounts of one provider.
#[derive(Clone, Debug, PartialEq)]
pub struct Combined {
    pub kind: Kind,
    /// What is used of the accounts' windows together: each account's
    /// window counts the same, so this is their mean.
    pub used: f32,
    /// The first of them to start over.
    pub resets: Option<SystemTime>,
    /// Each account that has this window, by its place in what was given,
    /// with its own share and reset.
    pub parts: Vec<(usize, f32, Option<SystemTime>)>,
}

/// The windows of several accounts put together, in the order they are
/// shown. An account without limits (`None`) has no part in any.
pub fn combine(accounts: &[Option<&Limits>]) -> Vec<Combined> {
    let mut combined: Vec<Combined> = Vec::new();
    for (at, limits) in accounts.iter().enumerate() {
        let Some(limits) = limits else {
            continue;
        };
        for window in &limits.windows {
            let slot = match combined.iter_mut().find(|slot| slot.kind == window.kind) {
                Some(slot) => slot,
                None => {
                    combined.push(Combined {
                        kind: window.kind.clone(),
                        used: 0.,
                        resets: None,
                        parts: Vec::new(),
                    });
                    combined.last_mut().expect("just pushed")
                }
            };
            slot.parts.push((at, window.used, window.resets));
            slot.resets = match (slot.resets, window.resets) {
                (Some(known), Some(new)) => Some(known.min(new)),
                (known, new) => known.or(new),
            };
        }
    }
    for slot in &mut combined {
        let total: f32 = slot.parts.iter().map(|(_, used, _)| used).sum();
        slot.used = total / slot.parts.len().max(1) as f32;
    }
    combined.sort_by(|a, b| a.kind.cmp(&b.kind));
    combined
}

/// A share as it is kept: a number from 0 to 100, whatever came.
pub(crate) fn share(value: f64) -> f32 {
    if value.is_finite() {
        value.clamp(0., 100.) as f32
    } else {
        0.
    }
}

/// A plan's name with a capital: `max` is `Max`.
pub(crate) fn plan_name(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let mut letters = raw.chars();
    let first = letters.next()?;
    Some(
        first
            .to_uppercase()
            .chain(letters)
            .collect::<String>()
            .replace('_', " "),
    )
}

/// Windows in the order they are shown.
pub(crate) fn in_order(mut windows: Vec<Window>) -> Vec<Window> {
    windows.sort_by(|a, b| a.kind.cmp(&b.kind));
    windows.dedup_by(|b, a| a.kind == b.kind);
    windows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: u64) -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
    }

    fn window(kind: Kind, used: f32, resets: u64) -> Window {
        Window {
            kind,
            used,
            resets: at(resets),
        }
    }

    #[test]
    fn accounts_of_one_provider_are_put_together_window_by_window() {
        let first = Limits {
            plan: Some("Max".into()),
            windows: vec![
                window(Kind::Weekly, 66., 900),
                window(Kind::Model("Fable".into()), 84., 900),
            ],
        };
        let second = Limits {
            plan: Some("Pro".into()),
            windows: vec![
                window(Kind::Session, 10., 300),
                window(Kind::Weekly, 20., 500),
            ],
        };
        // The second of three could not be asked: it has no part in any.
        let combined = combine(&[Some(&first), None, Some(&second)]);
        let kinds: Vec<_> = combined.iter().map(|slot| slot.kind.clone()).collect();
        assert_eq!(
            kinds,
            [Kind::Session, Kind::Weekly, Kind::Model("Fable".into())],
            "the short window, the week, the week by model"
        );
        // A window one account has is that account's.
        assert_eq!(combined[0].used, 10.);
        assert_eq!(combined[0].parts, [(2, 10., at(300))]);
        // One both have is their mean, and starts over when the first does.
        assert_eq!(combined[1].used, 43.);
        assert_eq!(combined[1].resets, at(500));
        assert_eq!(combined[1].parts, [(0, 66., at(900)), (2, 20., at(500))]);
        assert_eq!(combined[2].used, 84.);
        assert!(combine(&[None, None]).is_empty());
        assert!(combine(&[]).is_empty());
    }

    #[test]
    fn windows_have_their_names_and_an_account_its_home_variable() {
        assert_eq!(Kind::Session.label(), "5h");
        assert_eq!(Kind::Weekly.label(), "Weekly");
        assert_eq!(Kind::Model("Fable".into()).label(), "Fable weekly");
        assert_eq!(home_variable("claude"), Some("CLAUDE_CONFIG_DIR"));
        assert_eq!(home_variable("codex"), Some("CODEX_HOME"));
        assert_eq!(home_variable("grok"), Some("GROK_HOME"));
        assert_eq!(home_variable("shell"), None);
        assert!(asks("claude") && asks("codex") && asks("grok") && !asks("opencode"));
        assert_eq!(plan_name("max").as_deref(), Some("Max"));
        assert_eq!(plan_name("pro_plus").as_deref(), Some("Pro plus"));
        assert_eq!(plan_name("  "), None);
        assert_eq!(share(f64::NAN), 0.);
        assert_eq!(share(140.), 100.);
        assert_eq!(share(-3.), 0.);
    }
}
