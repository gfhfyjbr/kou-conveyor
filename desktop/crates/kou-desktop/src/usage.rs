//! Usage, as a page of the app (it stands where the workspace does, as the
//! settings do): what each provider's accounts have left of their plans (the
//! short window where a plan has one, the week, the week for a model), and
//! what the open chats have spent.
//!
//! The limits are asked of the agents' own programs (see `kou-usage`), one
//! account at a time and off the main thread: when the page opens, every
//! few minutes while it is open, and when asked for. The accounts of one
//! provider (a harness with its home in several folders, see
//! [`crate::state::Profile`]) are shown together: each window once, with
//! what is left of all of them and a bar for each account.
//!
//! This file gathers what the page shows ([`Shell::usage_data`]); the page
//! itself is drawn by a plugin, `plugins/usage/desktop/usage.js`, which
//! can be changed while the app runs.

use crate::{shell::Shell, state::Id};
use gpui::{AnyElement, App, AppContext as _, Context, Window};
use kou_ui::{Mark, Theme};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime},
};

/// The harnesses whose accounts have plan limits that can be asked for, in
/// the order their blocks stand in on the page.
pub(crate) const PROVIDERS: [&str; 3] = ["claude", "codex", "grok"];

/// How old an answer may be before it is asked for again by itself.
const FRESH: Duration = Duration::from_secs(300);
/// How long an agent's program is given to answer.
const PATIENCE: Duration = Duration::from_secs(20);
/// How often the open page looks at the clock: what is left of a window is
/// said to the minute.
const TICK: Duration = Duration::from_secs(30);
/// How long the bars take to fill when the page opens.
const FILL: Duration = Duration::from_millis(520);

/// An account whose limits can be asked: a harness, and which of its
/// accounts (`None`: its own).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Account {
    pub harness: String,
    pub profile: Option<Id>,
}

/// What is known of an account's limits.
#[derive(Clone, Debug, Default)]
pub(crate) struct Asked {
    /// The last answer; `None` before the first, and for a harness that
    /// cannot be asked.
    pub answer: Option<kou_usage::Answer>,
    /// When that answer came.
    pub at: Option<Instant>,
    /// Whether it is being asked right now.
    pub asking: bool,
}

/// An open chat and what it has spent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Chat {
    pub pane: Id,
    pub title: String,
    /// The account it runs under, by name, when not the harness's own.
    pub account: Option<String>,
    pub mark: Mark,
    pub usage: kou_thread::Usage,
}

/// One of a provider's accounts, as its block names it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AccountLine {
    pub name: String,
    pub plan: Option<String>,
    /// Why it has no limits to show, when it has none.
    pub note: Option<String>,
    /// Asked, and not answered yet for the first time.
    pub pending: bool,
}

/// A provider's block: its accounts' limits together, and its open chats.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Block {
    pub harness: String,
    pub name: String,
    pub code: String,
    pub accounts: Vec<AccountLine>,
    /// Each window once, over the accounts that have it.
    pub windows: Vec<kou_usage::Combined>,
    /// Whether any of its accounts is being asked right now.
    pub asking: bool,
    /// Whether its accounts have limits that can be asked for at all.
    pub asks: bool,
    pub chats: Vec<Chat>,
}

impl Block {
    /// The plan, when every account that says one says the same.
    pub(crate) fn plan(&self) -> Option<&str> {
        let mut plans = self
            .accounts
            .iter()
            .filter_map(|account| account.plan.as_deref());
        let first = plans.next()?;
        plans.all(|plan| plan == first).then_some(first)
    }

    /// What its chats spent together: tokens in, tokens out, and dollars.
    pub(crate) fn spent(&self) -> (u64, u64, f64) {
        self.chats
            .iter()
            .fold((0, 0, 0.), |(input, output, cost), chat| {
                (
                    input + chat.usage.input_tokens.unwrap_or(0),
                    output + chat.usage.output_tokens.unwrap_or(0),
                    cost + chat.usage.cost_usd.unwrap_or(0.),
                )
            })
    }
}

impl Shell {
    /// The accounts whose limits can be asked: of each provider that is
    /// installed, its own and those added to it.
    pub(crate) fn usage_accounts(&self) -> Vec<Account> {
        PROVIDERS
            .into_iter()
            .filter(|harness| self.harness(harness).is_some_and(|harness| harness.ready))
            .flat_map(|harness| {
                std::iter::once(None)
                    .chain(
                        self.store
                            .profiles_of(harness)
                            .map(|profile| Some(profile.id.clone())),
                    )
                    .map(move |profile| Account {
                        harness: harness.to_owned(),
                        profile,
                    })
            })
            .collect()
    }

    /// Ask the accounts for their limits again: all of them with `force`,
    /// otherwise those whose answer is old or missing. Each is asked off the
    /// main thread; the page follows as the answers come.
    pub(crate) fn refresh_usage(&mut self, force: bool, cx: &mut Context<Self>) {
        // (Not in tests and captures: no program is run for those.)
        if !self.probes {
            return;
        }
        for account in self.usage_accounts() {
            let known = self.usage.get(&account);
            let stale = known.is_none_or(|known| known.at.is_none_or(|at| at.elapsed() > FRESH));
            if known.is_some_and(|known| known.asking) || !(force || stale) {
                continue;
            }
            let program = self
                .detected
                .iter()
                .find(|detected| detected.spec.id == account.harness)
                .and_then(|detected| detected.path.clone());
            let Some(program) = program else {
                continue;
            };
            let ask = kou_usage::Ask {
                program,
                env: self.account_env(&account.harness, account.profile.as_deref()),
                timeout: PATIENCE,
            };
            self.usage.entry(account.clone()).or_default().asking = true;
            let harness = account.harness.clone();
            let asked = cx.background_spawn(async move { kou_usage::limits(&harness, &ask) });
            cx.spawn(async move |this, cx| {
                let answer = asked.await;
                this.update(cx, |this, cx| {
                    let known = this.usage.entry(account).or_default();
                    known.asking = false;
                    known.at = Some(Instant::now());
                    known.answer = answer;
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    /// The accounts changed (one was added or forgotten): what was known of
    /// those that are gone is dropped, and the new ones are asked if the
    /// page is open.
    pub(crate) fn usage_changed(&mut self, cx: &mut Context<Self>) {
        let accounts = self.usage_accounts();
        self.usage.retain(|account, _| accounts.contains(account));
        if self.usage_open {
            self.refresh_usage(false, cx);
        }
        cx.notify();
    }

    /// Show the usage page where the workspace is: its bars fill, the
    /// accounts whose answers are old are asked again, and go on being
    /// asked while it stays open.
    pub(crate) fn open_usage(&mut self, cx: &mut Context<Self>) {
        self.close_pages();
        self.usage_open = true;
        self.usage_opened = Some(cx.background_executor().now());
        self.usage_tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let open = this.update(cx, |this, cx| {
                    if this.usage_open {
                        this.refresh_usage(false, cx);
                        cx.notify();
                    }
                    this.usage_open
                });
                if !matches!(open, Ok(true)) {
                    break;
                }
            }
        }));
        self.refresh_usage(false, cx);
        cx.notify();
    }

    /// The usage page, or back from it.
    pub(crate) fn toggle_usage(&mut self, cx: &mut Context<Self>) {
        if self.usage_open {
            self.close_pages();
            self.see_focused();
            cx.notify();
        } else {
            self.open_usage(cx);
        }
    }

    /// The open chats of a harness and what each has spent.
    pub(crate) fn usage_chats(&self, harness: &str, cx: &App) -> Vec<Chat> {
        self.store
            .panes
            .iter()
            .filter(|pane| pane.harness == harness)
            .filter_map(|pane| {
                let talking = self.threads.get(&pane.id)?;
                Some(Chat {
                    pane: pane.id.clone(),
                    title: pane.title.clone(),
                    account: pane
                        .profile
                        .as_deref()
                        .and_then(|profile| self.store.profile(profile))
                        .map(|profile| profile.name.clone()),
                    mark: self
                        .live
                        .get(&pane.id)
                        .map_or(Mark::Idle, crate::shell::Live::shown),
                    usage: talking.view().read(cx).thread().usage,
                })
            })
            .collect()
    }

    /// A harness's block: its accounts' limits put together, and its chats.
    pub(crate) fn usage_block(&self, harness: &str, cx: &App) -> Block {
        let info = self.harness(harness);
        let accounts: Vec<Account> = self
            .usage_accounts()
            .into_iter()
            .filter(|account| account.harness == harness)
            .collect();
        let known: Vec<Option<&Asked>> = accounts
            .iter()
            .map(|account| self.usage.get(account))
            .collect();
        let limits: Vec<Option<&kou_usage::Limits>> = known
            .iter()
            .map(|known| known.and_then(|known| known.answer.as_ref()?.as_ref().ok()))
            .collect();
        let lines = accounts
            .iter()
            .zip(&known)
            .map(|(account, known)| {
                let answer = known.and_then(|known| known.answer.as_ref());
                AccountLine {
                    name: account
                        .profile
                        .as_deref()
                        .and_then(|profile| self.store.profile(profile))
                        .map_or_else(|| "main".to_owned(), |profile| profile.name.clone()),
                    plan: answer
                        .and_then(|answer| answer.as_ref().ok())
                        .and_then(|limits| limits.plan.clone()),
                    note: answer
                        .and_then(|answer| answer.as_ref().err())
                        .map(kou_usage::Reason::message),
                    pending: answer.is_none() && known.is_some_and(|known| known.asking),
                }
            })
            .collect();
        Block {
            harness: harness.to_owned(),
            name: info.map_or_else(|| harness.to_owned(), |info| info.name.clone()),
            code: info.map_or_else(|| harness.to_uppercase(), |info| info.code.clone()),
            accounts: lines,
            windows: kou_usage::combine(&limits),
            asking: known
                .iter()
                .any(|known| known.is_some_and(|known| known.asking)),
            asks: kou_usage::asks(harness),
            chats: self.usage_chats(harness, cx),
        }
    }

    /// Every block of the page: the providers that are installed, then any
    /// other harness with a chat that has spent something.
    pub(crate) fn usage_blocks(&self, cx: &App) -> Vec<Block> {
        let mut blocks: Vec<Block> = PROVIDERS
            .into_iter()
            .filter(|harness| self.harness(harness).is_some_and(|harness| harness.ready))
            .map(|harness| self.usage_block(harness, cx))
            .collect();
        for info in &self.harnesses {
            if PROVIDERS.contains(&info.id.as_str()) {
                continue;
            }
            let block = self.usage_block(&info.id, cx);
            let spent = block
                .chats
                .iter()
                .any(|chat| chat.usage != kou_thread::Usage::default());
            if spent {
                blocks.push(block);
            }
        }
        blocks
    }

    /// When the oldest of the answers on show came, and whether any account
    /// is being asked now.
    pub(crate) fn usage_age(&self) -> (Option<Duration>, bool) {
        let accounts = self.usage_accounts();
        let known = accounts
            .iter()
            .filter_map(|account| self.usage.get(account));
        let asking = known.clone().any(|known| known.asking);
        let oldest = known
            .filter_map(|known| known.at)
            .map(|at| at.elapsed())
            .max();
        (oldest, asking)
    }
}

/// A count of tokens in a few characters: `940`, `48.2k`, `1.2M`.
pub(crate) fn compact(count: u64) -> String {
    let short = |value: f64, unit: &str| {
        // One decimal below a hundred of the unit, none from there.
        if value < 99.95 {
            format!("{value:.1}{unit}")
        } else {
            format!("{value:.0}{unit}")
        }
    };
    match count {
        0..=999 => count.to_string(),
        1_000..=999_499 => short(count as f64 / 1e3, "k"),
        _ => short(count as f64 / 1e6, "M"),
    }
}

/// Dollars as they are said: cents below ten, whole dollars from a hundred.
pub(crate) fn dollars(amount: f64) -> String {
    if amount >= 100. {
        format!("${amount:.0}")
    } else {
        format!("${amount:.2}")
    }
}

/// How long ago, for the line that says when the limits were last asked.
fn ago(age: Duration) -> String {
    match age.as_secs() {
        0..=59 => "just now".to_owned(),
        seconds if seconds < 3600 => format!("{}m ago", seconds / 60),
        seconds => format!("{}h ago", seconds / 3600),
    }
}

/// What the first reset gives back of a window over several accounts: the
/// share of the whole that the accounts starting over then have used, and
/// when that is. `None` when no account says when it starts over.
pub(crate) fn comes_back(window: &kou_usage::Combined) -> Option<(f32, SystemTime)> {
    let first = window.resets?;
    let used: f32 = window
        .parts
        .iter()
        .filter(|(_, _, resets)| {
            resets.is_some_and(|at| {
                at.duration_since(first)
                    .is_ok_and(|later| later < Duration::from_secs(60))
            })
        })
        .map(|(_, used, _)| used)
        .sum();
    Some((used / window.parts.len().max(1) as f32, first))
}

/// How much of a window's time is left, from 0 to 1, where its length and
/// when it starts over are both known.
pub(crate) fn time_left(
    kind: &kou_usage::Kind,
    resets: Option<SystemTime>,
    now: SystemTime,
) -> Option<f32> {
    let length = kind.length()?;
    let left = resets?.duration_since(now).unwrap_or_default();
    Some((left.as_secs_f32() / length.as_secs_f32()).clamp(0., 1.))
}

impl Shell {
    /// How far the bars have filled, from 0 to 1.
    fn usage_filled(&self, window: &mut Window, cx: &Context<Self>) -> f32 {
        let Some(opened) = self.usage_opened else {
            return 1.;
        };
        if cx.reduce_motion() {
            return 1.;
        }
        let since = cx
            .background_executor()
            .now()
            .saturating_duration_since(opened);
        let t = (since.as_secs_f32() / FILL.as_secs_f32()).clamp(0., 1.);
        if t < 1. {
            window.request_animation_frame();
        }
        kou_ui::motion::ease_out(t)
    }

    /// What the Usage page shows, as its plugin is given it
    /// (`plugins/usage/desktop/usage.js`): everything worked out and worded,
    /// so that the script only lays it out.
    ///
    /// ```json
    /// { "updated": "Updated 2m ago", "fill": 1.0,
    ///   "providers": [ { "name": "Claude Code", "code": "CC", "about": "Max · 2 accounts",
    ///     "limits": [ { "label": "Weekly", "left": 83.0, "back": "+14% in 4d 23h",
    ///       "parts": [ { "account": "main", "left": 73.0, "resets": "4d 23h", "time": 0.71 } ] } ],
    ///     "notes": ["work: Asking Claude Code…"],
    ///     "spent": "1 chat · 183k in · 9.8k out · $1.37",
    ///     "chats": [ { "pane": "p3", "title": "…", "account": "work", "mark": "working",
    ///       "context": 48.2, "cost": "$1.37", "said": "183k in · 9.8k out · click to show this chat" } ] } ] }
    /// ```
    pub(crate) fn usage_data(&self, filled: f32, cx: &App) -> Value {
        let now = self.usage_now.unwrap_or_else(SystemTime::now);
        let (age, asking) = self.usage_age();
        let updated = if asking {
            "Asking…".to_owned()
        } else {
            age.map_or_else(String::new, |age| format!("Updated {}", ago(age)))
        };
        let providers: Vec<Value> = self
            .usage_blocks(cx)
            .iter()
            .map(|block| {
                let several = block.accounts.len() > 1;
                // Beside the name: the plan, and how many accounts.
                let mut about: Vec<String> = Vec::new();
                if let Some(plan) = block.plan() {
                    about.push(plan.to_owned());
                }
                if several {
                    about.push(format!("{} accounts", block.accounts.len()));
                }
                // What the accounts without limits say for themselves.
                let mut notes: Vec<String> = block
                    .accounts
                    .iter()
                    .filter_map(|account| {
                        let note = match (&account.note, account.pending) {
                            (Some(note), _) => note.clone(),
                            (None, true) => format!("Asking {}…", block.name),
                            (None, false) => return None,
                        };
                        Some(if several {
                            format!("{}: {note}", account.name)
                        } else {
                            note
                        })
                    })
                    .collect();
                if block.asks
                    && block.windows.is_empty()
                    && notes.is_empty()
                    && block.accounts.iter().all(|account| account.plan.is_none())
                {
                    notes.push("Its limits have not been asked for yet".to_owned());
                }
                let limits: Vec<Value> = block
                    .windows
                    .iter()
                    .map(|window| {
                        let back = comes_back(window).map(|(share, at)| {
                            let until = kou_usage::until(at, now);
                            if share >= 0.5 {
                                format!("+{share:.0}% in {until}")
                            } else {
                                format!("Starts over in {until}")
                            }
                        });
                        let parts: Vec<Value> = window
                            .parts
                            .iter()
                            .map(|(account, used, resets)| {
                                json!({
                                    "account": block
                                        .accounts
                                        .get(*account)
                                        .map_or("", |account| account.name.as_str()),
                                    "left": (100. - used).clamp(0., 100.),
                                    "resets": resets.map(|at| kou_usage::until(at, now)),
                                    "time": time_left(&window.kind, *resets, now),
                                })
                            })
                            .collect();
                        json!({
                            "label": window.kind.label(),
                            "left": (100. - window.used).clamp(0., 100.),
                            "back": back,
                            "parts": parts,
                        })
                    })
                    .collect();
                let (input, output, cost) = block.spent();
                let mut spent: Vec<String> = vec![match block.chats.len() {
                    0 => "No open chats".to_owned(),
                    1 => "1 chat".to_owned(),
                    chats => format!("{chats} chats"),
                }];
                if input > 0 {
                    spent.push(format!("{} in", compact(input)));
                }
                if output > 0 {
                    spent.push(format!("{} out", compact(output)));
                }
                if cost > 0. {
                    spent.push(dollars(cost));
                }
                let chats: Vec<Value> = block
                    .chats
                    .iter()
                    .map(|chat| {
                        let usage = chat.usage;
                        let context = match (usage.context_used, usage.context_limit) {
                            (Some(used), Some(limit)) if limit > 0 => {
                                Some(used as f32 / limit as f32 * 100.)
                            }
                            _ => None,
                        };
                        // The tokens are the block's to total; a row says
                        // them when asked.
                        let mut tokens: Vec<String> = Vec::new();
                        if let Some(input) = usage.input_tokens {
                            tokens.push(format!("{} in", compact(input)));
                        }
                        if let Some(output) = usage.output_tokens {
                            tokens.push(format!("{} out", compact(output)));
                        }
                        let said = if tokens.is_empty() {
                            "Show this chat".to_owned()
                        } else {
                            format!("{} · click to show this chat", tokens.join(" · "))
                        };
                        json!({
                            "pane": chat.pane,
                            "title": chat.title,
                            "account": chat.account,
                            "mark": match chat.mark {
                                Mark::Working => "working",
                                Mark::Done => "done",
                                Mark::Blocked => "blocked",
                                Mark::Failed => "failed",
                                Mark::Idle => "idle",
                            },
                            "context": context,
                            "cost": usage.cost_usd.map(dollars),
                            "said": said,
                        })
                    })
                    .collect();
                json!({
                    "harness": block.harness,
                    "name": block.name,
                    "code": block.code,
                    "about": about.join(" · "),
                    "limits": limits,
                    "notes": notes,
                    "spent": spent.join(" · "),
                    "chats": chats,
                })
            })
            .collect();
        json!({ "updated": updated, "fill": filled, "providers": providers })
    }

    /// The usage page: drawn by the `usage` plugin from [`Self::usage_data`].
    pub(crate) fn usage_page(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filled = self.usage_filled(window, cx);
        let data = self.usage_data(filled, cx);
        self.plugin_page_element("usage", data, theme, cx)
    }
}

/// What the shell's map of answers is keyed by.
pub(crate) type Answers = HashMap<Account, Asked>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_amounts_are_said_in_a_few_characters() {
        assert_eq!(compact(0), "0");
        assert_eq!(compact(940), "940");
        assert_eq!(compact(1_000), "1.0k");
        assert_eq!(compact(48_240), "48.2k");
        assert_eq!(compact(183_400), "183k");
        assert_eq!(compact(999_499), "999k");
        assert_eq!(compact(999_500), "1.0M");
        assert_eq!(compact(1_240_000), "1.2M");
        assert_eq!(compact(361_000_000), "361M");
        assert_eq!(dollars(0.), "$0.00");
        assert_eq!(dollars(1.374), "$1.37");
        assert_eq!(dollars(412.6), "$413");
        assert_eq!(ago(Duration::from_secs(12)), "just now");
        assert_eq!(ago(Duration::from_secs(150)), "2m ago");
        assert_eq!(ago(Duration::from_secs(7300)), "2h ago");
    }
}
