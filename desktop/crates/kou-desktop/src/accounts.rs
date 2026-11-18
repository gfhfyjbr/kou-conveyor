//! The accounts of a harness as one pool: the proxy each goes out through,
//! the folder the app makes for one, and which of them a new agent runs
//! under.
//!
//! An account is the same program with its home in another folder (see
//! [`crate::state::Profile`]). Three things here make several of them more
//! than several logins:
//!
//! - **A proxy of its own.** Whatever is started under an account (its chat,
//!   its terminal interface, the question about its limits, its sign-in)
//!   gets the proxy in its environment, and what that program starts
//!   inherits it. The address is kept with the account; an address with a
//!   name and a password is kept apart from the state, in a file only the
//!   user can read ([`secret`]).
//! - **One store of conversations.** A folder the app makes for a Claude
//!   Code account has the folders the conversations are kept in linked to
//!   those of the harness's own home ([`make_home`]): a conversation begun
//!   under one account can be gone on with under another, which is what
//!   lets a pane move to an account that still has room.
//! - **A choice by what is left.** [`balanced`] picks the account a new
//!   agent runs under: the one with the most left of its tightest limit,
//!   less what it is already running.

use crate::{
    shell::Shell,
    state::{Id, PaneKind},
    usage::Account,
};
use gpui::{Context, Window};
use kou_canvas::field::{Field, Outcome};
use kou_ui::Mark;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// The key context of the account form's fields.
pub(crate) const FORM: &str = "AccountForm";

/// An account being added in the settings, or one whose name and proxy are
/// being changed: what is typed so far.
pub struct AccountForm {
    /// The harness it is an account of.
    pub harness: String,
    /// The account being changed; `None` for a new one.
    pub editing: Option<Id>,
    pub name: Field,
    pub proxy: Field,
    /// Which of the two fields has the keys.
    pub on_proxy: bool,
    /// Why what is typed cannot be taken, said under the form.
    pub problem: Option<String>,
}

/// The home of a harness's own account: where its own variable points when
/// the app was started with it, else the folder the harness uses by itself.
pub(crate) fn own_home(harness: &str) -> Option<PathBuf> {
    let variable = kou_usage::home_variable(harness)?;
    if let Some(set) = std::env::var_os(variable).filter(|set| !set.is_empty()) {
        return Some(PathBuf::from(set));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(home.join(match harness {
        "claude" => ".claude",
        "codex" => ".codex",
        "grok" => ".grok",
        _ => return None,
    }))
}

impl Shell {
    /// The home of a harness's own account (see [`own_home`]).
    pub(crate) fn own_home(&self, harness: &str) -> Option<PathBuf> {
        self.own_homes
            .get(harness)
            .cloned()
            .or_else(|| own_home(harness))
    }

    /// Open the form that adds an account of `harness`, or changes one
    /// (`editing`). The keys go to its first field.
    pub(crate) fn open_account_form(
        &mut self,
        harness: &str,
        editing: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let known = editing.and_then(|id| self.store.profile(id));
        let (name, proxy) = match known {
            // The whole address, as it was given: what is shown of it has
            // no password, and saving that back would lose it.
            Some(profile) => (
                profile.name.clone(),
                secret(&self.home, &profile.id)
                    .or_else(|| profile.proxy.clone())
                    .unwrap_or_default(),
            ),
            None => (String::new(), String::new()),
        };
        self.appearance.page = crate::appearance::Page::Accounts;
        self.account_form = Some(AccountForm {
            harness: harness.to_owned(),
            editing: known.map(|profile| profile.id.clone()),
            name: Field::new(&name, false),
            proxy: Field::new(&proxy, false),
            on_proxy: false,
            problem: None,
        });
        window.focus(&self.account_focus, cx);
        cx.notify();
    }

    pub(crate) fn close_account_form(&mut self, cx: &mut Context<Self>) {
        self.account_form = None;
        cx.notify();
    }

    /// Tab: from one field of the form to the other.
    pub(crate) fn account_form_next(
        &mut self,
        _: &crate::shell::AccountNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = self.account_form.as_mut() {
            form.on_proxy = !form.on_proxy;
            cx.notify();
        }
    }

    /// A press on one of the form's fields gives it the keys.
    pub(crate) fn account_form_at(
        &mut self,
        proxy: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(form) = self.account_form.as_mut() {
            form.on_proxy = proxy;
            window.focus(&self.account_focus, cx);
            cx.notify();
        }
    }

    pub(crate) fn account_form_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        let Some(form) = self.account_form.as_mut() else {
            return;
        };
        let field = if form.on_proxy {
            &mut form.proxy
        } else {
            &mut form.name
        };
        match field.key(&event.keystroke, clipboard) {
            Outcome::Commit => self.commit_account_form(window, cx),
            Outcome::Cancel => self.account_form = None,
            Outcome::Copy(text) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
            Outcome::Edited => form.problem = None,
            Outcome::Moved => {}
            Outcome::Ignored => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Take what the form holds. A new account gets a folder of the app's
    /// own and is signed in to at once, in a pane, through its proxy; one
    /// that is changed keeps its folder. A proxy that cannot be used leaves
    /// the form open, saying why.
    pub(crate) fn commit_account_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.account_form.as_ref() else {
            return;
        };
        let (harness, editing) = (form.harness.clone(), form.editing.clone());
        let name = form.name.text().trim().to_owned();
        let proxy = match Proxy::parse(form.proxy.text()) {
            Ok(proxy) => proxy,
            Err(problem) => {
                if let Some(form) = self.account_form.as_mut() {
                    form.problem = Some(problem);
                    form.on_proxy = true;
                }
                cx.notify();
                return;
            }
        };
        let account = match editing {
            Some(account) => account,
            None => {
                let id = self.store.next_profile_id();
                let own = self.own_home(&harness);
                let folder = match make_home(&self.home, &harness, &id, own.as_deref()) {
                    Ok(folder) => folder,
                    Err(error) => {
                        if let Some(form) = self.account_form.as_mut() {
                            form.problem = Some(format!("Its folder could not be made: {error}."));
                        }
                        cx.notify();
                        return;
                    }
                };
                self.store.add_profile(&harness, folder)
            }
        };
        let fresh = self
            .account_form
            .as_ref()
            .is_some_and(|form| form.editing.is_none());
        if let Err(problem) = self.set_account(&account, &name, proxy.as_ref()) {
            if let Some(form) = self.account_form.as_mut() {
                form.problem = Some(problem);
                // (What was made is the account being changed from now on:
                // a second try does not make a second one.)
                form.editing = Some(account);
            }
            cx.notify();
            return;
        }
        self.account_form = None;
        self.save(cx);
        self.usage_changed(cx);
        if fresh {
            self.sign_in(&account, window, cx);
        }
        cx.notify();
    }

    /// The old way in, kept beside the form: a folder that is there already
    /// (a home that is signed in to) becomes the account, with the name and
    /// the proxy typed. Nobody is signed in for it.
    pub(crate) fn account_from_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.account_form.as_ref() else {
            return;
        };
        let harness = form.harness.clone();
        let Some(variable) = kou_usage::home_variable(&harness) else {
            return;
        };
        let called = self
            .harness(&harness)
            .map_or_else(|| harness.clone(), |harness| harness.name.clone());
        let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(format!("Use as {called}'s home ({variable})").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            let Some(home) = paths.into_iter().next() else {
                return;
            };
            this.update(cx, |this, cx| {
                let Some(form) = this.account_form.as_ref() else {
                    return;
                };
                let name = form.name.text().trim().to_owned();
                let proxy = match Proxy::parse(form.proxy.text()) {
                    Ok(proxy) => proxy,
                    Err(problem) => {
                        if let Some(form) = this.account_form.as_mut() {
                            form.problem = Some(problem);
                        }
                        cx.notify();
                        return;
                    }
                };
                let account = this.store.add_profile(&harness, home);
                if let Err(problem) = this.set_account(&account, &name, proxy.as_ref()) {
                    this.notice = Some(problem.into());
                }
                this.account_form = None;
                this.save(cx);
                this.usage_changed(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Give an account its name (when one is typed that no other account of
    /// its harness has) and its proxy, or take the proxy away.
    fn set_account(
        &mut self,
        account: &str,
        name: &str,
        proxy: Option<&Proxy>,
    ) -> Result<(), String> {
        let harness = self
            .store
            .profile(account)
            .map(|profile| profile.harness.clone())
            .ok_or_else(|| "The account is gone.".to_owned())?;
        let taken = self
            .store
            .profiles_of(&harness)
            .any(|other| other.id != account && other.name == name);
        if taken {
            return Err(format!("Another account is called \"{name}\"."));
        }
        // An address with a password goes to the file beside the state; one
        // without is all there is to it, and is kept with the account.
        let whole = proxy
            .filter(|proxy| proxy.login.is_some())
            .map(Proxy::address);
        set_secret(&self.home, account, whole.as_deref())
            .map_err(|error| format!("The proxy could not be kept: {error}."))?;
        if let Some(profile) = self
            .store
            .profiles
            .iter_mut()
            .find(|profile| profile.id == account)
        {
            if !name.is_empty() {
                name.clone_into(&mut profile.name);
            }
            profile.proxy = proxy.map(Proxy::shown);
        }
        // Its limits are asked through the proxy from now on.
        self.usage.remove(&Account {
            harness,
            profile: Some(account.to_owned()),
        });
        Ok(())
    }

    /// Sign in to an account: its harness's own sign-in command, in a pane
    /// of its own, with the account's home and proxy. Whatever that command
    /// asks (a page in the browser, a code to paste) is the user's to do
    /// there; when it ends the account's limits are asked again, which is
    /// how the settings learn that somebody is signed in.
    pub(crate) fn sign_in(&mut self, account: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.store.profile(account).cloned() else {
            return;
        };
        let command = self
            .detected
            .iter()
            .find(|detected| detected.spec.id == profile.harness)
            .and_then(|detected| detected.spec.login.sign_in());
        if command.is_none() {
            self.notice = Some(
                format!(
                    "{} is added. Sign in to it in a pane started under it.",
                    profile.name
                )
                .into(),
            );
            return;
        }
        let session = self.store.selected.clone().or_else(|| {
            self.store
                .sessions
                .first()
                .map(|session| session.id.clone())
        });
        let Some(session) = session else {
            self.notice = Some(
                format!(
                    "{} is added. Open a project, then sign in to it from the settings: \
                     signing in runs in a pane.",
                    profile.name
                )
                .into(),
            );
            return;
        };
        let Some(pane) = self.add_pane(
            &session,
            &profile.harness,
            None,
            PaneKind::Terminal,
            window,
            cx,
        ) else {
            return;
        };
        if let Some(saved) = self.store.pane_mut(&pane) {
            saved.profile = Some(profile.id.clone());
            saved.title = format!("Sign in · {}", profile.name);
            saved.titled = true;
        }
        self.signing_in.insert(pane.clone(), profile.id.clone());
        self.launch(&pane, None, window, cx);
        self.save(cx);
    }

    /// A pane that was signing in is over: the account is asked for its
    /// limits again.
    pub(crate) fn signed_in(&mut self, pane: &str, cx: &mut Context<Self>) {
        let Some(account) = self.signing_in.remove(pane) else {
            return;
        };
        if let Some(profile) = self.store.profile(&account) {
            self.usage.remove(&Account {
                harness: profile.harness.clone(),
                profile: Some(account),
            });
        }
        self.refresh_usage(false, cx);
    }

    /// What the empty page's field says of the account a prompt goes to:
    /// the one named; "auto" where none is named and the harness's accounts
    /// are one pool; nothing for a harness with one account.
    pub(crate) fn asked_account_shown(&self) -> Option<String> {
        if let Some(profile) = self.asked_profile() {
            return Some(profile.name.clone());
        }
        let (harness, _) = self.asked()?;
        (self.store.balance && self.store.profiles_of(&harness.id).next().is_some())
            .then(|| "auto".to_owned())
    }

    /// What a pane's header says of the account it runs under: its name;
    /// "own" for the harness's own where it has others; nothing where there
    /// is only the one.
    pub(crate) fn account_shown(&self, pane: &crate::state::Pane) -> Option<String> {
        match pane.profile.as_deref() {
            Some(profile) => self
                .store
                .profile(profile)
                .map(|profile| profile.name.clone()),
            None => (pane.harness != "shell"
                && self.store.profiles_of(&pane.harness).next().is_some())
            .then(|| "own".to_owned()),
        }
    }

    /// The accounts of a harness, its own first, each with what is known
    /// of it for placing a new agent.
    pub(crate) fn account_standings(&self, harness: &str) -> Vec<(Option<Id>, Standing)> {
        std::iter::once(None)
            .chain(
                self.store
                    .profiles_of(harness)
                    .map(|profile| Some(profile.id.clone())),
            )
            .map(|profile| {
                let asked = self.usage.get(&Account {
                    harness: harness.to_owned(),
                    profile: profile.clone(),
                });
                let (left, signed_out) = match asked.and_then(|asked| asked.answer.as_ref()) {
                    Some(Ok(limits)) => (left_of(limits), false),
                    Some(Err(kou_usage::Reason::SignIn(_))) => (None, true),
                    _ => (None, false),
                };
                let (mut working, mut resting) = (0, 0);
                for pane in &self.store.panes {
                    if pane.harness != harness || pane.profile != profile {
                        continue;
                    }
                    match self.live.get(&pane.id).map(|live| live.mark) {
                        Some(Mark::Working | Mark::Blocked) => working += 1,
                        Some(_)
                            if self.running.contains_key(&pane.id)
                                || self.threads.contains_key(&pane.id) =>
                        {
                            resting += 1;
                        }
                        _ => {}
                    }
                }
                (
                    profile,
                    Standing {
                        left,
                        signed_out,
                        working,
                        resting,
                    },
                )
            })
            .collect()
    }

    /// The account of `harness` a new agent runs under when none is named
    /// (`None`: its own). See [`balanced`].
    pub(crate) fn balanced_account(&self, harness: &str) -> Option<Id> {
        let standings = self.account_standings(harness);
        let all: Vec<Standing> = standings
            .iter()
            .map(|(_, standing)| standing.clone())
            .collect();
        balanced(&all).and_then(|at| standings[at].0.clone())
    }

    /// What the settings say of an account beside its name: what is left
    /// of its limits, or that nobody is signed in. Empty before it was asked.
    pub(crate) fn account_state(&self, harness: &str, profile: Option<&str>) -> String {
        let asked = self.usage.get(&Account {
            harness: harness.to_owned(),
            profile: profile.map(str::to_owned),
        });
        match asked.and_then(|asked| asked.answer.as_ref()) {
            Some(Ok(limits)) => match left_of(limits) {
                Some(left) => format!("{left:.0}% left"),
                None => "signed in".to_owned(),
            },
            Some(Err(kou_usage::Reason::SignIn(_))) => "not signed in".to_owned(),
            Some(Err(reason)) => reason.message(),
            None => String::new(),
        }
    }

    /// Move a pane to the account of its harness that has the most left,
    /// and go on there with the same conversation. Only between accounts
    /// that share their conversations (see [`make_home`]): elsewhere the
    /// conversation would not be found. False when there is nowhere better
    /// to go.
    pub(crate) fn move_to_best_account(
        &mut self,
        pane: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(saved) = self.store.pane(pane).cloned() else {
            return false;
        };
        let Some(own) = self.own_home(&saved.harness) else {
            return false;
        };
        let shares = |profile: &Option<Id>| match profile {
            None => true,
            Some(id) => self
                .store
                .profile(id)
                .is_some_and(|profile| shares_conversations(&saved.harness, &profile.home, &own)),
        };
        if !shares(&saved.profile) {
            self.notice = Some(
                "This account keeps its conversations to itself: its pane cannot go on under another."
                    .into(),
            );
            return false;
        }
        // The best of the others, this pane not counted against any.
        let others: Vec<(Option<Id>, Standing)> = self
            .account_standings(&saved.harness)
            .into_iter()
            .filter(|(profile, _)| *profile != saved.profile && shares(profile))
            .collect();
        let all: Vec<Standing> = others
            .iter()
            .map(|(_, standing)| standing.clone())
            .collect();
        let Some(to) = balanced(&all).map(|at| others[at].0.clone()) else {
            return false;
        };
        let called = match &to {
            Some(id) => self
                .store
                .profile(id)
                .map_or_else(String::new, |profile| profile.name.clone()),
            None => "its own account".to_owned(),
        };
        if let Some(saved) = self.store.pane_mut(pane) {
            saved.profile = to;
        }
        self.notice = Some(format!("{} goes on under {called}.", saved.title).into());
        // A Claude Code kept apart from the app runs under the account it
        // was started with, and would be found again as it is: it is ended,
        // and the conversation goes on from its record under the other.
        self.end_kept(pane);
        self.save(cx);
        // The same conversation, opened again under the other account.
        self.restart(pane, window, cx);
        true
    }
}

/// What is left of an account's tightest limit over all models, from 0 to
/// 100: the short window and the week. (A limit for one model is no reason
/// to keep an agent on another model away.)
fn left_of(limits: &kou_usage::Limits) -> Option<f32> {
    limits
        .windows
        .iter()
        .filter(|window| {
            matches!(
                window.kind,
                kou_usage::Kind::Session | kou_usage::Kind::Weekly
            )
        })
        .map(|window| (100. - window.used).clamp(0., 100.))
        .reduce(f32::min)
}

/// A proxy as given for an account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Proxy {
    /// `http` or `https`: how the proxy itself is spoken to.
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// Its name and password, when it asks for them.
    pub login: Option<(String, String)>,
}

impl Proxy {
    /// Reads a proxy as people are given them: an address
    /// (`http://name:password@host:port`), `name:password@host:port`,
    /// `host:port:name:password` as sellers of proxies write it, or just
    /// `host:port`. `Ok(None)` for nothing typed; `Err` says what is wrong
    /// in words for the form.
    pub(crate) fn parse(text: &str) -> Result<Option<Self>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        if text.chars().any(char::is_whitespace) {
            return Err("A proxy's address has no spaces in it.".to_owned());
        }
        let (scheme, rest) = match text.split_once("://") {
            Some((scheme, rest)) => (scheme.to_ascii_lowercase(), rest),
            None => ("http".to_owned(), text),
        };
        if scheme.starts_with("socks") {
            return Err(
                "A SOCKS proxy is not one the agents take: give an HTTP or HTTPS proxy.".to_owned(),
            );
        }
        if scheme != "http" && scheme != "https" {
            return Err(format!(
                "\"{scheme}\" is not a kind of proxy this takes: http or https."
            ));
        }
        let rest = rest.trim_end_matches('/');
        // `host:port:name:password`, as sellers of proxies write it: the
        // first two are the place, and the rest may hold anything (looked
        // for first: a password may have an `@` in it).
        let parts: Vec<&str> = rest.splitn(4, ':').collect();
        let (login, place) = match parts.as_slice() {
            [host, port, name, password]
                if !text.contains("://")
                    && !host.contains('@')
                    && !rest.starts_with('[')
                    && port.parse::<u16>().is_ok() =>
            {
                (
                    Some(((*name).to_owned(), (*password).to_owned())),
                    format!("{host}:{port}"),
                )
            }
            _ => match rest.rsplit_once('@') {
                Some((login, place)) => {
                    let (name, password) = login.split_once(':').unwrap_or((login, ""));
                    (Some((decoded(name), decoded(password))), place.to_owned())
                }
                None => (None, rest.to_owned()),
            },
        };
        // The port is what follows the last colon (an IPv6 host is in
        // brackets, with colons of its own).
        let Some((host, port)) = place.rsplit_once(':') else {
            return Err("A proxy needs its port: host:port.".to_owned());
        };
        let port: u16 = port
            .parse()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| format!("\"{port}\" is not a port."))?;
        let plain = |c: char| c.is_ascii_alphanumeric() || "-._[]:".contains(c);
        if host.is_empty() || !host.chars().all(plain) {
            return Err(format!("\"{host}\" is not a host."));
        }
        Ok(Some(Self {
            scheme,
            host: host.to_owned(),
            port,
            login: login.filter(|(name, _)| !name.is_empty()),
        }))
    }

    /// The address without the name and password: what the app shows and
    /// keeps with the account.
    pub(crate) fn shown(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }

    /// The whole address, as a program takes it in its environment.
    pub(crate) fn address(&self) -> String {
        match &self.login {
            Some((name, password)) => format!(
                "{}://{}:{}@{}:{}",
                self.scheme,
                encoded(name),
                encoded(password),
                self.host,
                self.port
            ),
            None => self.shown(),
        }
    }
}

// A name or a password as it stands in an address: everything but letters,
// digits and `-._~` by its bytes.
fn encoded(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// The same read back.
fn decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = (bytes[at] == b'%')
            .then(|| text.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What a program's environment gets to go out through the proxy at
/// `address`: the variables in both cases (programs differ in which they
/// read), and what stays direct, with whatever the app's own environment
/// already keeps direct (`direct`, its `NO_PROXY`).
pub(crate) fn proxy_env(address: &str, direct: Option<&str>) -> Vec<(String, String)> {
    let mut stays: Vec<&str> = direct
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .collect();
    for local in ["localhost", "127.0.0.1", "::1"] {
        if !stays.contains(&local) {
            stays.push(local);
        }
    }
    let stays = stays.join(",");
    let mut env = Vec::new();
    for name in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        env.push((name.to_owned(), address.to_owned()));
    }
    for name in ["NO_PROXY", "no_proxy"] {
        env.push((name.to_owned(), stays.clone()));
    }
    env
}

// Where the addresses that hold a password are kept: beside the accounts'
// folders, apart from the state.
fn secrets_file(home: &Path) -> PathBuf {
    home.join("accounts").join("secrets.json")
}

fn secrets(home: &Path) -> HashMap<String, String> {
    std::fs::read(secrets_file(home))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The whole address of an account's proxy, when it has a name and a
/// password (one without them is kept with the account itself).
pub(crate) fn secret(home: &Path, account: &str) -> Option<String> {
    secrets(home).remove(account)
}

/// Keeps the whole address of an account's proxy, or forgets it (`None`).
/// The file is the user's alone to read.
pub(crate) fn set_secret(home: &Path, account: &str, address: Option<&str>) -> anyhow::Result<()> {
    let mut all = secrets(home);
    match address {
        Some(address) => {
            all.insert(account.to_owned(), address.to_owned());
        }
        None => {
            if all.remove(account).is_none() {
                return Ok(());
            }
        }
    }
    let file = secrets_file(home);
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let draft = file.with_extension("json.new");
    std::fs::write(&draft, serde_json::to_vec_pretty(&all)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&draft, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&draft, &file)?;
    Ok(())
}

/// The folders of a harness's home that hold its conversations, by the
/// harness's id: what a folder made for another account of it shares with
/// the harness's own, so that a conversation can go on under any of them.
/// Claude Code keeps a conversation's record under `projects`, what it
/// changed (for going back) under `file-history`, and its lists under
/// `todos`. Empty for a harness whose store was not looked into.
fn shared(harness: &str) -> &'static [&'static str] {
    match harness {
        "claude" => &["projects", "file-history", "todos"],
        _ => &[],
    }
}

/// Makes the folder of a new account of `harness` under the app's own
/// (`home`), named for the account, and returns it. `own` is the home of
/// the harness's own account: the folders its conversations are in are
/// linked into the new one, those that are there. Nothing is written into
/// `own`.
pub(crate) fn make_home(
    home: &Path,
    harness: &str,
    account: &str,
    own: Option<&Path>,
) -> std::io::Result<PathBuf> {
    let folder = home.join("accounts").join(format!("{harness}-{account}"));
    std::fs::create_dir_all(&folder)?;
    #[cfg(unix)]
    if let Some(own) = own {
        for name in shared(harness) {
            let (theirs, ours) = (own.join(name), folder.join(name));
            // (Only what exists: a link to nothing would stand where the
            // program wants to make a folder.)
            if theirs.is_dir() && std::fs::symlink_metadata(&ours).is_err() {
                std::os::unix::fs::symlink(&theirs, &ours)?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = own;
    Ok(folder)
}

/// Whether the folder of an account shares its conversations with the
/// harness's own home: whether a pane can move to it and go on.
pub(crate) fn shares_conversations(harness: &str, account_home: &Path, own: &Path) -> bool {
    let names = shared(harness);
    !names.is_empty()
        && account_home.join(names[0]).canonicalize().ok() == own.join(names[0]).canonicalize().ok()
        && own.join(names[0]).is_dir()
}

/// What is known of an account when one is picked for a new agent.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Standing {
    /// What is left of its tightest limit, from 0 to 100; `None` when its
    /// limits are not known (never asked, or the asking failed).
    pub left: Option<f32>,
    /// It cannot be used now: nobody is signed in to it.
    pub signed_out: bool,
    /// How many agents run under it now that are at work.
    pub working: usize,
    /// How many more are open under it, at rest.
    pub resting: usize,
}

/// How much an agent at work, and one at rest, counts against an account
/// when the next one is placed: limits are asked every few minutes, and
/// several agents started one after another would otherwise all go to the
/// account that looked best when it was last asked.
const WORKING: f32 = 15.;
const RESTING: f32 = 3.;
/// What an account whose limits are not known counts as having left: not
/// the first choice beside one known to be nearly untouched, and well
/// before one known to be nearly spent.
const UNKNOWN: f32 = 50.;
/// Under this much left an account is spent: it is picked only when every
/// other one is.
const SPENT: f32 = 2.;

/// Which of the accounts a new agent runs under: the index of the best of
/// `standings`, or `None` when there are none. The one with the most left
/// of its tightest limit, less what it already runs; among equals the one
/// given first (the harness's own). An account nobody is signed in to, or
/// one that is spent, is taken only when all are.
pub(crate) fn balanced(standings: &[Standing]) -> Option<usize> {
    let score = |standing: &Standing| {
        standing.left.unwrap_or(UNKNOWN)
            - WORKING * standing.working as f32
            - RESTING * standing.resting as f32
    };
    let usable =
        |standing: &Standing| !standing.signed_out && standing.left.is_none_or(|left| left > SPENT);
    let best = |among: &dyn Fn(&Standing) -> bool| {
        standings
            .iter()
            .enumerate()
            .filter(|(_, standing)| among(standing))
            .fold(None::<(usize, f32)>, |best, (at, standing)| {
                let score = score(standing);
                match best {
                    Some((_, most)) if most >= score => best,
                    _ => Some((at, score)),
                }
            })
            .map(|(at, _)| at)
    };
    best(&usable)
        .or_else(|| best(&|standing| !standing.signed_out))
        .or_else(|| best(&|_| true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_is_read_in_the_forms_people_are_given_them() {
        let read = |text: &str| Proxy::parse(text).unwrap().unwrap();
        let plain = read("10.0.0.7:8080");
        assert_eq!(plain.shown(), "http://10.0.0.7:8080");
        assert_eq!(plain.address(), "http://10.0.0.7:8080");
        // As sellers write it, and as an address.
        for text in [
            "proxy.example.com:3128:alice:s3cret",
            "alice:s3cret@proxy.example.com:3128",
            "http://alice:s3cret@proxy.example.com:3128/",
        ] {
            let proxy = read(text);
            assert_eq!(proxy.shown(), "http://proxy.example.com:3128", "{text}");
            assert_eq!(
                proxy.address(),
                "http://alice:s3cret@proxy.example.com:3128",
                "{text}"
            );
        }
        assert_eq!(read("HTTPS://gate.example.com:443").scheme, "https");
        // What would break an address is written by its bytes, and one
        // that came so written is not written twice.
        let odd = read("gate.example.com:8080:alice:p@ss:w/rd");
        assert_eq!(
            odd.address(),
            "http://alice:p%40ss%3Aw%2Frd@gate.example.com:8080"
        );
        assert_eq!(read(&odd.address()), odd);
        assert_eq!(Proxy::parse("  "), Ok(None));
    }

    #[test]
    fn a_proxy_that_cannot_be_used_is_refused_with_why() {
        let why = |text: &str| Proxy::parse(text).unwrap_err();
        assert!(why("socks5://10.0.0.7:1080").contains("SOCKS"));
        assert!(why("ftp://10.0.0.7:21").contains("http or https"));
        assert!(why("proxy.example.com").contains("port"));
        assert!(why("proxy.example.com:eighty").contains("not a port"));
        assert!(why("proxy.example.com:0").contains("not a port"));
        assert!(why("pro xy:80").contains("spaces"));
        assert!(why("a:b@:8080").contains("not a host"));
    }

    #[test]
    fn a_program_is_sent_through_the_proxy_and_not_for_what_is_local() {
        let env = proxy_env("http://a:b@gate:8080", Some("corp.example, localhost"));
        let of = |name: &str| {
            env.iter()
                .find(|(known, _)| known == name)
                .map(|(_, value)| value.as_str())
        };
        for name in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
            assert_eq!(of(name), Some("http://a:b@gate:8080"), "{name}");
        }
        assert_eq!(of("NO_PROXY"), Some("corp.example,localhost,127.0.0.1,::1"));
        assert_eq!(of("no_proxy"), of("NO_PROXY"));
    }

    #[test]
    fn an_address_with_a_password_is_kept_apart_and_only_for_the_user() {
        let home = std::env::temp_dir().join(format!("kou-accounts-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        assert_eq!(secret(&home, "a1"), None);
        set_secret(&home, "a1", Some("http://a:b@gate:8080")).unwrap();
        set_secret(&home, "a2", Some("http://c:d@gate:8081")).unwrap();
        assert_eq!(secret(&home, "a1").as_deref(), Some("http://a:b@gate:8080"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(secrets_file(&home))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        set_secret(&home, "a1", None).unwrap();
        assert_eq!(secret(&home, "a1"), None);
        assert_eq!(secret(&home, "a2").as_deref(), Some("http://c:d@gate:8081"));
        std::fs::remove_dir_all(&home).ok();
    }

    #[cfg(unix)]
    #[test]
    fn an_account_s_folder_shares_the_conversations_of_the_harness_s_own_home() {
        let root = std::env::temp_dir().join(format!("kou-account-home-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let (home, own) = (root.join("app"), root.join("own"));
        std::fs::create_dir_all(own.join("projects")).unwrap();
        std::fs::write(own.join("projects").join("one.jsonl"), "{}").unwrap();
        let made = make_home(&home, "claude", "a3", Some(&own)).unwrap();
        assert_eq!(made, home.join("accounts").join("claude-a3"));
        // The conversations are the same ones, seen from both.
        assert!(made.join("projects").join("one.jsonl").is_file());
        assert!(shares_conversations("claude", &made, &own));
        // What the own home does not have is not linked to: the program
        // makes it itself, in the account's folder.
        assert!(std::fs::symlink_metadata(made.join("todos")).is_err());
        // Nothing was written into the own home.
        assert_eq!(std::fs::read_dir(&own).unwrap().count(), 1);
        // Made again, it is the same folder, as it is.
        assert_eq!(make_home(&home, "claude", "a3", Some(&own)).unwrap(), made);
        // Another harness's store was not looked into: nothing is shared.
        let other = make_home(&home, "codex", "a4", Some(&own)).unwrap();
        assert!(std::fs::symlink_metadata(other.join("projects")).is_err());
        assert!(!shares_conversations("codex", &other, &own));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_new_agent_goes_to_the_account_with_the_most_left_less_what_it_runs() {
        let standing = |left: Option<f32>, working: usize| Standing {
            left,
            signed_out: false,
            working,
            resting: 0,
        };
        assert_eq!(balanced(&[]), None);
        // The most left.
        assert_eq!(
            balanced(&[standing(Some(20.), 0), standing(Some(80.), 0)]),
            Some(1)
        );
        // Equal: the first, the harness's own.
        assert_eq!(
            balanced(&[standing(Some(60.), 0), standing(Some(60.), 0)]),
            Some(0)
        );
        // What an account already runs counts against it: three agents
        // started one after another do not all go to one account.
        assert_eq!(
            balanced(&[standing(Some(70.), 2), standing(Some(60.), 0)]),
            Some(1)
        );
        // Not known is neither best nor worst.
        assert_eq!(
            balanced(&[standing(None, 0), standing(Some(90.), 0)]),
            Some(1)
        );
        assert_eq!(
            balanced(&[standing(None, 0), standing(Some(10.), 0)]),
            Some(0)
        );
        // A spent one only when all are, and one nobody is signed in to
        // only when there is nothing else.
        assert_eq!(
            balanced(&[standing(Some(1.), 0), standing(Some(30.), 4)]),
            Some(1)
        );
        assert_eq!(
            balanced(&[standing(Some(1.), 0), standing(Some(0.), 0)]),
            Some(0)
        );
        let out = Standing {
            signed_out: true,
            ..standing(Some(100.), 0)
        };
        assert_eq!(balanced(&[out.clone(), standing(Some(5.), 3)]), Some(1));
        assert_eq!(balanced(&[out]), Some(0));
    }
}
