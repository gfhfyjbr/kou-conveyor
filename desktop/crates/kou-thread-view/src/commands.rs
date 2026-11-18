//! Slash commands: the backend's own, the ones every backend takes through
//! the contract (a model, a mode), the view's, and the app's around it; and
//! what the field suggests while one is typed, as kou-conveyor's web cockpit
//! does: names while the first word is typed, then the values its argument
//! takes, or a line that says what it expects.

use kou_thread::Choice;

/// Who carries a command out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Act {
    /// The backend: the command goes to it as the prompt's text.
    Backend,
    /// Switch the model through the contract.
    Model,
    /// Switch the mode through the contract.
    Mode,
    Access,
    /// Switch the level of effort through the contract.
    Effort,
    /// Stop the running turn.
    Stop,
    /// What waits for the agent.
    Queue,
    /// Copy the last answer.
    Copy,
    /// Open every tool call and every reasoning.
    Expand,
    /// Fold them.
    Collapse,
    /// Clear the view; the conversation goes on.
    Clear,
    /// Keys and commands.
    Help,
    /// Earlier prompts.
    History,
    /// Put an earlier prompt back in the field.
    Edit,
    /// The app: a new session in this pane.
    New,
    /// The app: the sessions to open.
    Sessions,
    /// The app: open a session by id or title.
    Resume,
    /// The app: title the session.
    Rename,
    /// The app: the connection's settings.
    Settings,
}

/// A command the field offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Spec {
    /// Typed after the slash.
    pub name: String,
    /// Other names it answers to.
    pub aliases: Vec<&'static str>,
    /// What it takes after its name, as the list shows it: `[focus]`, `<level>`.
    pub args: Option<String>,
    /// What it does.
    pub help: String,
    pub act: Act,
    /// A backend command whose argument is not known: choosing it completes
    /// it rather than running it, so an argument can follow.
    pub open: bool,
}

impl Spec {
    /// Whether Enter on it in the list runs it rather than completing it.
    pub(crate) fn runs_at_once(&self) -> bool {
        !self.open
            && self
                .args
                .as_deref()
                .is_none_or(|args| args.starts_with('['))
    }
}

/// A value a command's argument takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Value {
    pub value: String,
    pub label: String,
    pub detail: String,
    /// The one in force.
    pub current: bool,
}

/// What the backend, the thread and the view offer now, from which the
/// commands in force are made.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Offer {
    /// The backend's slash commands.
    pub backend: Vec<Choice>,
    /// What the backend's commands take, by id.
    pub arguments: Vec<(String, String)>,
    pub models: Vec<Choice>,
    pub model: Option<String>,
    pub modes: Vec<Choice>,
    pub mode: Option<String>,
    pub access: kou_thread::Access,
    /// The levels of effort, offered apart from the modes.
    pub efforts: Vec<Choice>,
    pub effort: Option<String>,
    /// A turn runs.
    pub running: bool,
    /// Prompts wait in the queue; it is paused.
    pub queued: usize,
    pub paused: bool,
    /// The conversation has an answer to copy, and anything to fold or clear.
    pub answered: bool,
    pub entries: bool,
    /// There is an earlier prompt to put back.
    pub prompts: bool,
    /// The backend rewinds: an earlier prompt is edited where it stands.
    pub rewind: bool,
    /// The session has ended.
    pub closed: bool,
    /// The view asks the app for what is the app's.
    pub app: bool,
}

/// The commands in force, in the order the list shows them: the backend's
/// first, as the agent's own, then the rest in the cockpits' order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub specs: Vec<Spec>,
    pub models: Vec<Value>,
    pub modes: Vec<Value>,
    pub access: Vec<Value>,
    pub efforts: Vec<Value>,
    pub queue: Vec<Value>,
}

/// One line of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Suggestion {
    /// A command, found by its name or by an alias.
    Command {
        spec: usize,
        alias: Option<&'static str>,
    },
    /// A value for the argument of the command typed.
    Value { spec: usize, value: Value },
    /// What the command typed expects; not a choice.
    Hint { spec: usize },
}

fn spec(name: &str, args: Option<&str>, help: &str, act: Act) -> Spec {
    Spec {
        name: name.to_owned(),
        aliases: Vec::new(),
        args: args.map(str::to_owned),
        help: help.to_owned(),
        act,
        open: false,
    }
}

impl Catalog {
    /// The commands in force for what is offered.
    pub(crate) fn of(offer: &Offer) -> Self {
        let mut specs: Vec<Spec> = offer
            .backend
            .iter()
            .filter(|choice| !choice.name.trim_start_matches('/').is_empty())
            .map(|choice| {
                let args = offer
                    .arguments
                    .iter()
                    .find(|(id, _)| *id == choice.id)
                    .map(|(_, args)| args.clone());
                Spec {
                    name: choice.name.trim_start_matches('/').to_owned(),
                    aliases: Vec::new(),
                    open: args.is_none(),
                    args,
                    help: choice
                        .description
                        .clone()
                        .unwrap_or_else(|| "The agent's own command".to_owned()),
                    act: Act::Backend,
                }
            })
            .collect();
        let taken = |specs: &[Spec], name: &str| specs.iter().any(|spec| spec.name == name);
        let mut add = |item: Spec| {
            if !taken(&specs, &item.name) {
                specs.push(item);
            }
        };
        if offer.running {
            add(spec("stop", None, "Stop the run", Act::Stop));
        }
        if offer.queued > 0 || offer.running {
            add(spec(
                "queue",
                Some("[resume|pause|clear]"),
                "What waits for the agent: select it, hold it, run it or drop it",
                Act::Queue,
            ));
        }
        if offer.prompts && !offer.closed {
            add(spec(
                "edit",
                Some("[n]"),
                if offer.rewind {
                    "Edit prompt n, by default the last, and run it again from there"
                } else {
                    "Put prompt n, by default the last, back in the field"
                },
                Act::Edit,
            ));
        }
        if !offer.models.is_empty() && !offer.closed {
            add(spec(
                "model",
                Some("[model]"),
                "The model the next prompts run with; alone, the list",
                Act::Model,
            ));
        }
        // An agent that offers its efforts apart from its modes has a command
        // for them; one that offers them as its modes (kou) has them under
        // the mode's.
        let efforts = !offer.efforts.is_empty() && !offer.closed;
        if !offer.modes.is_empty() && !offer.closed {
            let mut mode = spec(
                "mode",
                Some("<mode>"),
                if efforts {
                    "The mode the agent works in"
                } else {
                    "The mode the agent works in; kou calls it its effort"
                },
                Act::Mode,
            );
            if !efforts {
                mode.aliases = vec!["effort", "think"];
            }
            add(mode);
        }
        if !offer.access.choices.is_empty() && !offer.closed {
            let mut permissions = spec(
                "permissions",
                Some("[preset]"),
                "Choose permissions for this chat",
                Act::Access,
            );
            permissions.aliases = vec!["approvals"];
            add(permissions);
        }
        if efforts {
            let mut effort = spec(
                "effort",
                Some("<level>"),
                "How hard the model thinks; alone, the list",
                Act::Effort,
            );
            effort.aliases = vec!["think"];
            add(effort);
        }
        if offer.app {
            add(spec(
                "new",
                None,
                "Start a new session in this pane",
                Act::New,
            ));
            add(spec(
                "sessions",
                None,
                "Open a saved session",
                Act::Sessions,
            ));
            add(spec(
                "resume",
                Some("<session>"),
                "Open a session by ID or title",
                Act::Resume,
            ));
            add(spec(
                "rename",
                Some("<title>"),
                "Title the session",
                Act::Rename,
            ));
        }
        add(spec(
            "history",
            None,
            "Earlier prompts, to reuse one",
            Act::History,
        ));
        if offer.answered {
            add(spec("copy", None, "Copy the last answer", Act::Copy));
        }
        if offer.entries {
            add(spec(
                "expand",
                None,
                "Expand tool output and thinking",
                Act::Expand,
            ));
            add(spec(
                "collapse",
                None,
                "Collapse tool output and thinking",
                Act::Collapse,
            ));
            add(spec(
                "clear",
                None,
                "Clear the view; the session keeps its history",
                Act::Clear,
            ));
        }
        if offer.app {
            let mut settings = spec(
                "settings",
                None,
                "The connection: provider, address, key, model",
                Act::Settings,
            );
            settings.aliases = vec!["config"];
            add(settings);
        }
        let mut help = spec("help", None, "Keyboard shortcuts and commands", Act::Help);
        help.aliases = vec!["?"];
        add(help);

        let values = |choices: &[Choice], current: Option<&str>| -> Vec<Value> {
            choices
                .iter()
                .map(|choice| {
                    let is_current = current == Some(choice.id.as_str());
                    let mut detail = choice.description.clone().unwrap_or_default();
                    if choice.name != choice.id && !detail.contains(&choice.name) {
                        detail = if detail.is_empty() {
                            choice.name.clone()
                        } else {
                            format!("{} · {detail}", choice.name)
                        };
                    }
                    Value {
                        value: choice.id.clone(),
                        label: choice.id.clone(),
                        detail,
                        current: is_current,
                    }
                })
                .collect()
        };
        let mut queue = Vec::new();
        if offer.paused && offer.queued > 0 {
            queue.push(value(
                "resume",
                "run the queued messages, one after another",
            ));
        }
        if !offer.paused && offer.queued > 0 {
            queue.push(value(
                "pause",
                "keep them from running when the agent finishes",
            ));
        }
        if offer.queued > 0 {
            queue.push(value("clear", "drop the queued messages"));
        }
        Self {
            specs,
            models: values(&offer.models, offer.model.as_deref()),
            modes: values(&offer.modes, offer.mode.as_deref()),
            access: values(&offer.access.choices, offer.access.current.as_deref()),
            efforts: values(&offer.efforts, offer.effort.as_deref()),
            queue,
        }
    }

    /// The command a name or an alias names.
    pub(crate) fn named(&self, name: &str) -> Option<usize> {
        let name = name.to_lowercase();
        self.specs
            .iter()
            .position(|spec| spec.name == name || spec.aliases.contains(&name.as_str()))
    }

    /// The values a command's argument takes.
    pub(crate) fn values(&self, spec: usize) -> &[Value] {
        match self.specs[spec].act {
            Act::Model => &self.models,
            Act::Mode => &self.modes,
            Act::Access => &self.access,
            Act::Effort => &self.efforts,
            Act::Queue => &self.queue,
            _ => &[],
        }
    }

    /// What the field suggests for its text.
    pub(crate) fn suggestions(&self, raw: &str) -> Vec<Suggestion> {
        if !raw.starts_with('/') || raw.contains('\n') {
            return Vec::new();
        }
        let typed = &raw[1..];
        if !typed.contains(char::is_whitespace) {
            if typed.contains('/') {
                // A path: a prompt, not a command.
                return Vec::new();
            }
            let typed = typed.to_lowercase();
            let mut found: Vec<Suggestion> = Vec::new();
            let listed = |found: &[Suggestion], at: usize| {
                found
                    .iter()
                    .any(|item| matches!(item, Suggestion::Command { spec, .. } if *spec == at))
            };
            for (at, spec) in self.specs.iter().enumerate() {
                if spec.name.starts_with(&typed) {
                    found.push(Suggestion::Command {
                        spec: at,
                        alias: None,
                    });
                }
            }
            if !typed.is_empty() {
                for (at, spec) in self.specs.iter().enumerate() {
                    if listed(&found, at) {
                        continue;
                    }
                    if let Some(alias) = spec.aliases.iter().find(|alias| alias.starts_with(&typed))
                    {
                        found.push(Suggestion::Command {
                            spec: at,
                            alias: Some(alias),
                        });
                    }
                }
                for (at, spec) in self.specs.iter().enumerate() {
                    if !listed(&found, at) && spec.name.contains(&typed) {
                        found.push(Suggestion::Command {
                            spec: at,
                            alias: None,
                        });
                    }
                }
            }
            return found;
        }
        let Some((name, argument)) = typed.split_once(char::is_whitespace) else {
            return Vec::new();
        };
        let Some(at) = self.named(name) else {
            return Vec::new();
        };
        let values = self.values(at);
        if !values.is_empty() {
            let query = argument.trim().to_lowercase();
            let mut scored: Vec<(u32, &Value)> = values
                .iter()
                .map(|value| {
                    let best = score(&query, &value.label)
                        .max(score(&query, &value.value) * 4 / 5)
                        .max(score(&query, &value.detail) * 3 / 10);
                    (best, value)
                })
                .filter(|(score, _)| *score > 0)
                .collect();
            if !query.is_empty() {
                scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
            }
            return scored
                .into_iter()
                .take(50)
                .map(|(_, value)| Suggestion::Value {
                    spec: at,
                    value: value.clone(),
                })
                .collect();
        }
        if argument.trim().is_empty() && self.specs[at].args.is_some() {
            return vec![Suggestion::Hint { spec: at }];
        }
        Vec::new()
    }
}

fn value(name: &str, detail: &str) -> Value {
    Value {
        value: name.to_owned(),
        label: name.to_owned(),
        detail: detail.to_owned(),
        current: false,
    }
}

/// How well a query matches a text, as the web cockpit scores it: a direct
/// hit, more at the start of a word, else the query's letters in order, fewer
/// gaps better; 0 for no match. An empty query matches everything a little.
pub(crate) fn score(query: &str, text: &str) -> u32 {
    if query.is_empty() {
        return 1;
    }
    let text = text.to_lowercase();
    if let Some(direct) = text.find(query) {
        let start = direct == 0 || text[..direct].ends_with(' ');
        return 100u32.saturating_sub(direct.min(99) as u32) + if start { 50 } else { 0 };
    }
    let (mut at, mut gaps) = (0, 0);
    for ch in query.chars() {
        let Some(next) = text[at..].find(ch) else {
            return 0;
        };
        gaps += next;
        at += next + ch.len_utf8();
    }
    40u32.saturating_sub(gaps as u32).max(1)
}

/// A command line: `/name argument`. `None` for a prompt: text that does not
/// start with a slash, or starts with a path such as `/usr/bin`.
pub(crate) fn parse(raw: &str) -> Option<(String, String)> {
    let raw = raw.trim_end();
    let rest = raw.strip_prefix('/')?;
    let (name, argument) = match rest.split_once(char::is_whitespace) {
        Some((name, argument)) => (name, argument.trim()),
        None => (rest, ""),
    };
    if name.contains('/') {
        return None;
    }
    Some((name.to_lowercase(), argument.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(id: &str, description: &str) -> Choice {
        Choice {
            id: id.into(),
            name: id.into(),
            description: Some(description.into()),
        }
    }

    fn offer() -> Offer {
        Offer {
            backend: vec![
                choice("compact", "Summarize"),
                choice("continue", "Pick up"),
                choice("review", "Review the tree"),
            ],
            arguments: vec![("compact".into(), "[focus]".into())],
            models: vec![
                Choice {
                    id: "opus".into(),
                    name: "Opus 4.5".into(),
                    description: None,
                },
                Choice {
                    id: "sonnet".into(),
                    name: "Sonnet 4.5".into(),
                    description: Some("fast".into()),
                },
            ],
            model: Some("sonnet".into()),
            modes: vec![choice("low", "x"), choice("high", "y")],
            mode: Some("high".into()),
            answered: true,
            entries: true,
            app: true,
            ..Offer::default()
        }
    }

    fn names(catalog: &Catalog, raw: &str) -> Vec<String> {
        catalog
            .suggestions(raw)
            .into_iter()
            .map(|item| match item {
                Suggestion::Command { spec, alias } => match alias {
                    Some(alias) => format!("/{} /{alias}", catalog.specs[spec].name),
                    None => format!("/{}", catalog.specs[spec].name),
                },
                Suggestion::Value { value, .. } => value.value,
                Suggestion::Hint { spec } => format!(
                    "hint {}",
                    catalog.specs[spec].args.clone().unwrap_or_default()
                ),
            })
            .collect()
    }

    #[test]
    fn the_backend_s_commands_lead_and_the_rest_follow_in_order() {
        let catalog = Catalog::of(&offer());
        assert_eq!(
            names(&catalog, "/c")[..5],
            ["/compact", "/continue", "/copy", "/collapse", "/clear"]
        );
        // An alias is found by what it starts with, and says it is one.
        assert_eq!(names(&catalog, "/eff"), ["/mode /effort"]);
        // A name that holds the text comes after those that start with it.
        assert_eq!(names(&catalog, "/ear"), ["/clear"]);
        // A path is a prompt.
        assert!(names(&catalog, "/usr/bin").is_empty());
        assert!(names(&catalog, "no slash").is_empty());
        assert!(names(&catalog, "/c\nmore").is_empty());
        // Every command is listed for a lone slash.
        assert_eq!(names(&catalog, "/").len(), catalog.specs.len());
    }

    #[test]
    fn what_runs_at_once_and_what_completes() {
        let catalog = Catalog::of(&offer());
        let get = |name: &str| &catalog.specs[catalog.named(name).expect(name)];
        assert!(get("compact").runs_at_once(), "its argument is optional");
        assert!(!get("review").runs_at_once(), "its argument is not known");
        assert!(get("copy").runs_at_once());
        assert!(!get("resume").runs_at_once(), "it needs a session");
        assert_eq!(get("think").act, Act::Mode, "by alias");
    }

    #[test]
    fn an_argument_lists_its_values_filtered_with_the_current_one_marked() {
        let catalog = Catalog::of(&offer());
        assert_eq!(names(&catalog, "/model "), ["opus", "sonnet"]);
        assert_eq!(names(&catalog, "/model son"), ["sonnet"]);
        let Suggestion::Value { value, .. } = &catalog.suggestions("/model ")[1] else {
            panic!("a value");
        };
        assert!(value.current);
        assert_eq!(value.detail, "Sonnet 4.5 · fast");
        // By the model's name too.
        assert_eq!(names(&catalog, "/model 4.5")[0], "opus");
        assert_eq!(names(&catalog, "/effort hi"), ["high"]);
        // A command that takes text says what it expects until it is typed.
        assert_eq!(names(&catalog, "/compact "), ["hint [focus]"]);
        assert!(names(&catalog, "/compact the tests").is_empty());
        assert!(names(&catalog, "/nothing x").is_empty());
    }

    #[test]
    fn commands_show_only_when_they_apply() {
        let mut quiet = offer();
        quiet.app = false;
        quiet.answered = false;
        quiet.models.clear();
        let catalog = Catalog::of(&quiet);
        assert!(catalog.named("copy").is_none());
        assert!(catalog.named("new").is_none());
        assert!(catalog.named("model").is_none());
        assert!(catalog.named("stop").is_none());
        quiet.running = true;
        quiet.queued = 2;
        let catalog = Catalog::of(&quiet);
        assert!(catalog.named("stop").is_some());
        assert_eq!(
            names(&catalog, "/queue "),
            ["pause", "clear"],
            "a running queue can be held or dropped"
        );
        // A backend command of the same name wins over the view's.
        let mut shadow = offer();
        shadow.backend.push(choice("clear", "The agent's clear"));
        let catalog = Catalog::of(&shadow);
        let clear = &catalog.specs[catalog.named("clear").expect("clear")];
        assert_eq!(
            (clear.act, clear.help.as_str()),
            (Act::Backend, "The agent's clear")
        );
    }

    #[test]
    fn levels_of_effort_offered_apart_from_the_modes_have_a_command_of_their_own() {
        let mut offer = offer();
        offer.modes = vec![choice("default", "Ask"), choice("plan", "Plan")];
        offer.mode = Some("default".into());
        offer.efforts = vec![
            Choice {
                id: "low".into(),
                name: "Low".into(),
                description: None,
            },
            Choice {
                id: "high".into(),
                name: "High".into(),
                description: None,
            },
        ];
        offer.effort = Some("high".into());
        let catalog = Catalog::of(&offer);
        let get = |name: &str| &catalog.specs[catalog.named(name).expect(name)];
        assert_eq!(get("effort").act, Act::Effort);
        assert_eq!(get("think").act, Act::Effort, "by alias");
        assert!(get("mode").aliases.is_empty());
        assert_eq!(names(&catalog, "/effort "), ["low", "high"]);
        assert_eq!(names(&catalog, "/effort hi"), ["high"]);
        let Suggestion::Value { value, .. } = &catalog.suggestions("/effort ")[1] else {
            panic!("a value");
        };
        assert!(value.current);
        assert_eq!(value.detail, "High");
        // None on offer: the mode's command answers to `/effort`, as kou's does.
        offer.efforts.clear();
        let catalog = Catalog::of(&offer);
        assert_eq!(
            catalog.specs[catalog.named("effort").unwrap()].act,
            Act::Mode
        );
    }

    #[test]
    fn a_command_line_is_parsed_and_a_path_is_not_one() {
        assert_eq!(
            parse("/compact  the tests "),
            Some(("compact".into(), "the tests".into()))
        );
        assert_eq!(parse("/Model"), Some(("model".into(), String::new())));
        assert_eq!(parse("/usr/bin/env is where?"), None);
        assert_eq!(parse("hello"), None);
        assert_eq!(score("", "x"), 1);
        assert!(score("son", "sonnet") > score("son", "parson"));
        assert!(score("snt", "sonnet") > 0);
        assert_eq!(score("zz", "sonnet"), 0);
    }
}
