//! The models a backend offers, as the web cockpit's model picker shows them
//! (`plugins/models/web/picker.js`): the providers first, each closed, with
//! how many models it has and its newest; a provider opened shows its models;
//! typing searches every provider's models at once and shows what it finds
//! under each provider; an ID no list holds can still be used.
//!
//! Who serves a model is what the backend says ([`kou_thread::Provider`]),
//! else what the ids make evident (a `provider/model` prefix). A backend
//! whose ids show nothing of the kind gets a flat list. All of it is plain
//! data, so the picker's behaviour is tested without a window.

use kou_thread::{Choice, Provider};

/// One provider and its models, as indices into the models on offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    /// Short and stable, for its colour: `anthropic`.
    pub id: String,
    /// As people say it: `Anthropic`.
    pub name: String,
    pub models: Vec<usize>,
}

/// The models on offer, grouped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub groups: Vec<Group>,
    /// Models no group holds, found only by searching: the web's media
    /// models, or what a backend's providers leave out.
    pub loose: Vec<usize>,
    /// No grouping is evident: the models are one flat list.
    pub flat: bool,
}

/// The names the cockpits give providers (`cockpit.ProviderName`).
const NAMES: [(&str, &str); 19] = [
    ("anthropic", "Anthropic"),
    ("openai", "OpenAI"),
    ("google", "Google"),
    ("xai", "xAI"),
    ("moonshot", "Moonshot"),
    ("meta", "Meta"),
    ("deepseek", "DeepSeek"),
    ("mistral", "Mistral"),
    ("qwen", "Qwen"),
    ("zhipu", "Zhipu"),
    ("antigravity", "Antigravity"),
    ("cognition", "Cognition"),
    ("minimax", "MiniMax"),
    ("iflow", "iFlow"),
    ("openrouter", "OpenRouter"),
    ("ollama", "Ollama"),
    ("groq", "Groq"),
    ("cohere", "Cohere"),
    ("amazon", "Amazon"),
];

/// Who makes a model the list does not describe, by how its name starts
/// (`picker.js`, `FAMILIES`).
const FAMILIES: [(&str, &str); 22] = [
    ("claude", "anthropic"),
    ("gpt", "openai"),
    ("chatgpt", "openai"),
    ("codex", "openai"),
    ("o1", "openai"),
    ("o3", "openai"),
    ("o4", "openai"),
    ("gemini", "google"),
    ("gemma", "google"),
    ("grok", "xai"),
    ("kimi", "moonshot"),
    ("moonshot", "moonshot"),
    ("deepseek", "deepseek"),
    ("qwen", "qwen"),
    ("qwq", "qwen"),
    ("llama", "meta"),
    ("muse", "meta"),
    ("mistral", "mistral"),
    ("codestral", "mistral"),
    ("devstral", "mistral"),
    ("glm", "zhipu"),
    ("minimax", "minimax"),
];

/// A provider's name for people.
pub(crate) fn provider_name(id: &str) -> String {
    let lower = id.to_ascii_lowercase();
    if let Some((_, name)) = NAMES.iter().find(|(known, _)| *known == lower) {
        return (*name).to_owned();
    }
    let mut chars = id.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Other".to_owned(),
    }
}

/// Who makes model `id`: as the backend's providers say, else its prefix,
/// else what its name tells (`picker.js`, `providerOf`).
pub(crate) fn provider_of(id: &str, providers: &[Provider]) -> Option<String> {
    if let Some(provider) = providers
        .iter()
        .find(|provider| provider.models.iter().any(|model| model == id))
    {
        return Some(provider.id.clone());
    }
    let name = id.trim().to_ascii_lowercase();
    if let Some((vendor, _)) = name.split_once('/')
        && !vendor.is_empty()
    {
        return Some(vendor.to_owned());
    }
    FAMILIES
        .iter()
        .find(|(prefix, _)| name.starts_with(prefix))
        .map(|(_, provider)| (*provider).to_owned())
}

impl Catalog {
    /// Groups `models`: by the backend's `providers` when it gave any, else by
    /// the `provider/` prefixes of their ids, else not at all.
    pub(crate) fn of(models: &[Choice], providers: &[Provider]) -> Self {
        if !providers.is_empty() {
            let mut held = vec![false; models.len()];
            let groups: Vec<Group> = providers
                .iter()
                .map(|provider| Group {
                    id: provider.id.clone(),
                    name: if provider.name.is_empty() {
                        provider_name(&provider.id)
                    } else {
                        provider.name.clone()
                    },
                    models: provider
                        .models
                        .iter()
                        .filter_map(|id| {
                            let at = models.iter().position(|model| &model.id == id)?;
                            held[at] = true;
                            Some(at)
                        })
                        .collect(),
                })
                .filter(|group| !group.models.is_empty())
                .collect();
            let loose = (0..models.len()).filter(|at| !held[*at]).collect();
            return Self {
                groups,
                loose,
                flat: false,
            };
        }
        // What the ids make evident: a prefix before a slash.
        let prefixed = models.iter().filter(|model| model.id.contains('/')).count();
        if prefixed == 0 {
            return Self {
                groups: Vec::new(),
                loose: Vec::new(),
                flat: true,
            };
        }
        let mut groups: Vec<Group> = Vec::new();
        for (at, model) in models.iter().enumerate() {
            let id = match model.id.split_once('/') {
                Some((vendor, _)) if !vendor.is_empty() => vendor.to_owned(),
                _ => String::new(),
            };
            match groups.iter_mut().find(|group| group.id == id) {
                Some(group) => group.models.push(at),
                None => groups.push(Group {
                    name: provider_name(&id),
                    id,
                    models: vec![at],
                }),
            }
        }
        // Models of no provider come last, as Other.
        groups.sort_by_key(|group| group.id.is_empty());
        Self {
            groups,
            loose: Vec::new(),
            flat: false,
        }
    }
}

/// What a line of the picker is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Line {
    /// A provider, closed: opening it shows its models.
    Provider(usize),
    /// The way back from a provider's models to the providers.
    Back(usize),
    /// A provider's name over what a search found of its models.
    Heading(usize),
    /// A model, by its index among the models on offer.
    Model(usize),
    /// An ID no list holds, to be used as typed.
    Free(String),
    /// Nothing matches.
    Nothing,
}

impl Line {
    /// Whether the cursor stops on it: what Enter can take.
    pub(crate) fn choosable(&self) -> bool {
        matches!(self, Line::Provider(_) | Line::Model(_) | Line::Free(_))
    }
}

/// What a key or a click did to the picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Nothing the picker takes.
    Ignored,
    /// It changed where it is or what it shows.
    Moved,
    /// A provider opened (1) or the providers came back (-1): the panes swap.
    Swapped(i8),
    /// This model was chosen: the picker closes.
    Chosen(String),
    /// It closes without a choice.
    Closed,
}

/// Where the picker is: what is typed, which provider is open, where the
/// cursor is among the choosable lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Browse {
    pub query: String,
    pub open: Option<usize>,
    pub cursor: usize,
}

impl Browse {
    /// The picker as it opens: on the model in force, or its provider. With
    /// one provider there is nothing to choose among, and its models show.
    pub(crate) fn opening(catalog: &Catalog, models: &[Choice], current: Option<&str>) -> Self {
        let mut browse = Self::default();
        if catalog.groups.len() == 1 {
            browse.open = Some(0);
        }
        let current = current.and_then(|id| models.iter().position(|model| model.id == id));
        let lines = browse.lines(catalog, models);
        let wanted = |line: &Line| match (line, current) {
            (Line::Model(at), Some(current)) => *at == current,
            (Line::Provider(group), Some(current)) => {
                catalog.groups[*group].models.contains(&current)
            }
            _ => false,
        };
        browse.cursor = lines
            .iter()
            .filter(|line| line.choosable())
            .position(wanted)
            .unwrap_or(0);
        browse
    }

    /// The words typed, lower case.
    fn words(&self) -> Vec<String> {
        self.query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect()
    }

    /// The lines shown: a provider's models, what a search finds, the
    /// providers, or the flat list.
    pub(crate) fn lines(&self, catalog: &Catalog, models: &[Choice]) -> Vec<Line> {
        let words = self.words();
        let found = |at: &usize, group: Option<&Group>| {
            let model = &models[*at];
            let mut hay = format!(
                "{} {} {}",
                model.id,
                model.name,
                model.description.as_deref().unwrap_or_default()
            );
            if let Some(group) = group {
                hay.push(' ');
                hay.push_str(&group.name);
                hay.push(' ');
                hay.push_str(&group.id);
            }
            let hay = hay.to_lowercase();
            words.iter().all(|word| hay.contains(word.as_str()))
        };
        let mut lines = Vec::new();
        if catalog.flat {
            lines.extend(
                (0..models.len())
                    .filter(|at| found(at, None))
                    .map(Line::Model),
            );
        } else if let Some(open) = self.open.filter(|open| *open < catalog.groups.len()) {
            let group = &catalog.groups[open];
            lines.push(Line::Back(open));
            lines.extend(
                group
                    .models
                    .iter()
                    .filter(|at| found(at, Some(group)))
                    .copied()
                    .map(Line::Model),
            );
        } else if !words.is_empty() {
            for (index, group) in catalog.groups.iter().enumerate() {
                let hits: Vec<usize> = group
                    .models
                    .iter()
                    .filter(|at| found(at, Some(group)))
                    .copied()
                    .collect();
                if !hits.is_empty() {
                    lines.push(Line::Heading(index));
                    lines.extend(hits.into_iter().map(Line::Model));
                }
            }
            lines.extend(
                catalog
                    .loose
                    .iter()
                    .filter(|at| found(at, None))
                    .copied()
                    .map(Line::Model),
            );
        } else {
            lines.extend((0..catalog.groups.len()).map(Line::Provider));
        }
        // An ID the list does not hold can still be used.
        let typed = self.query.trim();
        if !typed.is_empty()
            && !typed.contains(char::is_whitespace)
            && !models.iter().any(|model| model.id == typed)
        {
            lines.push(Line::Free(typed.to_owned()));
        }
        if !lines.iter().any(Line::choosable) && !lines.iter().any(|l| matches!(l, Line::Back(_))) {
            lines.push(Line::Nothing);
        }
        lines
    }

    /// The choosable line the cursor is on.
    pub(crate) fn current<'a>(&self, lines: &'a [Line]) -> Option<&'a Line> {
        lines
            .iter()
            .filter(|line| line.choosable())
            .nth(self.cursor)
    }

    fn count(lines: &[Line]) -> usize {
        lines.iter().filter(|line| line.choosable()).count()
    }

    /// Puts the cursor on the `nth` choosable line, if there is one.
    pub(crate) fn hover(&mut self, nth: usize, lines: &[Line]) -> Outcome {
        if nth < Self::count(lines) && nth != self.cursor {
            self.cursor = nth;
            return Outcome::Moved;
        }
        Outcome::Ignored
    }

    /// Takes the line the cursor is on: a provider opens, a model is chosen.
    pub(crate) fn take(&mut self, catalog: &Catalog, models: &[Choice]) -> Outcome {
        let lines = self.lines(catalog, models);
        match self.current(&lines).cloned() {
            Some(Line::Provider(group)) => self.enter(group),
            Some(Line::Model(at)) => Outcome::Chosen(models[at].id.clone()),
            Some(Line::Free(id)) => Outcome::Chosen(id),
            _ => Outcome::Ignored,
        }
    }

    fn enter(&mut self, group: usize) -> Outcome {
        self.open = Some(group);
        self.query.clear();
        self.cursor = 0;
        Outcome::Swapped(1)
    }

    /// Back from a provider's models to the providers, on the one that was open.
    pub(crate) fn back(&mut self, catalog: &Catalog, models: &[Choice]) -> Outcome {
        let Some(from) = self.open.take() else {
            return Outcome::Ignored;
        };
        self.query.clear();
        let lines = self.lines(catalog, models);
        self.cursor = lines
            .iter()
            .filter(|line| line.choosable())
            .position(|line| *line == Line::Provider(from))
            .unwrap_or(0);
        Outcome::Swapped(-1)
    }

    /// What is typed changed: the cursor goes to the first line.
    pub(crate) fn typed(&mut self, query: String) -> Outcome {
        if query == self.query {
            return Outcome::Ignored;
        }
        self.query = query;
        self.cursor = 0;
        Outcome::Moved
    }

    /// A key, as `picker.js` answers it in its search field.
    pub(crate) fn key(&mut self, key: &str, catalog: &Catalog, models: &[Choice]) -> Outcome {
        let lines = self.lines(catalog, models);
        let count = Self::count(&lines);
        let empty = self.query.is_empty();
        match key {
            "down" | "up" if count > 0 => {
                self.cursor = if key == "down" {
                    (self.cursor + 1) % count
                } else {
                    (self.cursor + count - 1) % count
                };
                Outcome::Moved
            }
            "pagedown" | "pageup" if count > 0 => {
                self.cursor = if key == "pagedown" {
                    (self.cursor + 8).min(count - 1)
                } else {
                    self.cursor.saturating_sub(8)
                };
                Outcome::Moved
            }
            "right" if empty => match self.current(&lines) {
                Some(Line::Provider(group)) => {
                    let group = *group;
                    self.enter(group)
                }
                _ => Outcome::Ignored,
            },
            "left" | "backspace" if empty && self.open.is_some() => self.back(catalog, models),
            "enter" => self.take(catalog, models),
            // Escape steps back: out of a search, out of a provider, then away.
            "escape" if !empty => self.typed(String::new()),
            "escape" if self.open.is_some() && catalog.groups.len() > 1 => {
                self.back(catalog, models)
            }
            "escape" => Outcome::Closed,
            "backspace" if !empty => {
                let mut query = self.query.clone();
                query.pop();
                self.typed(query)
            }
            _ => Outcome::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, name: &str) -> Choice {
        Choice {
            id: id.into(),
            name: name.into(),
            description: None,
        }
    }

    fn provider(id: &str, name: &str, models: &[&str]) -> Provider {
        Provider {
            id: id.into(),
            name: name.into(),
            models: models.iter().map(|model| (*model).to_owned()).collect(),
        }
    }

    fn kou() -> (Vec<Choice>, Vec<Provider>) {
        (
            vec![
                model("claude-fable-5", "Claude Fable 5"),
                model("claude-opus-5", "Claude Opus 5"),
                model("gpt-6.1-sol", "gpt-6.1-sol"),
                model("grok-4.7", "grok-4.7"),
                model("gpt-image-2", "gpt-image-2"),
            ],
            vec![
                provider(
                    "anthropic",
                    "Anthropic",
                    &["claude-fable-5", "claude-opus-5"],
                ),
                provider("openai", "OpenAI", &["gpt-6.1-sol"]),
                provider("xai", "xAI", &["grok-4.7"]),
            ],
        )
    }

    #[test]
    fn the_providers_come_closed_with_the_current_model_s_provider_under_the_cursor() {
        let (models, providers) = kou();
        let catalog = Catalog::of(&models, &providers);
        assert_eq!(catalog.loose, [4], "media is found by searching");
        let browse = Browse::opening(&catalog, &models, Some("gpt-6.1-sol"));
        let lines = browse.lines(&catalog, &models);
        assert_eq!(
            lines,
            [Line::Provider(0), Line::Provider(1), Line::Provider(2)]
        );
        assert_eq!(browse.current(&lines), Some(&Line::Provider(1)));
    }

    #[test]
    fn a_provider_opens_on_its_models_and_left_goes_back_to_it() {
        let (models, providers) = kou();
        let catalog = Catalog::of(&models, &providers);
        let mut browse = Browse::opening(&catalog, &models, Some("claude-opus-5"));
        assert_eq!(browse.key("right", &catalog, &models), Outcome::Swapped(1));
        let lines = browse.lines(&catalog, &models);
        assert_eq!(lines, [Line::Back(0), Line::Model(0), Line::Model(1)]);
        assert_eq!(browse.key("down", &catalog, &models), Outcome::Moved);
        assert_eq!(
            browse.key("enter", &catalog, &models),
            Outcome::Chosen("claude-opus-5".into())
        );
        assert_eq!(browse.key("left", &catalog, &models), Outcome::Swapped(-1));
        let lines = browse.lines(&catalog, &models);
        assert_eq!(browse.current(&lines), Some(&Line::Provider(0)));
        // Enter on a provider opens it too; Escape steps back, then closes.
        browse.key("down", &catalog, &models);
        assert_eq!(browse.key("enter", &catalog, &models), Outcome::Swapped(1));
        assert_eq!(
            browse.key("escape", &catalog, &models),
            Outcome::Swapped(-1)
        );
        assert_eq!(browse.key("escape", &catalog, &models), Outcome::Closed);
    }

    #[test]
    fn typing_searches_every_provider_and_shows_what_it_finds_under_each() {
        let (models, providers) = kou();
        let catalog = Catalog::of(&models, &providers);
        let mut browse = Browse::opening(&catalog, &models, None);
        browse.typed("g".into());
        let lines = browse.lines(&catalog, &models);
        // "g" is in gpt and grok, and in the media model a search finds.
        assert_eq!(
            lines,
            [
                Line::Heading(1),
                Line::Model(2),
                Line::Heading(2),
                Line::Model(3),
                Line::Model(4),
                Line::Free("g".into()),
            ]
        );
        browse.typed("anthropic opus".into());
        let lines = browse.lines(&catalog, &models);
        assert_eq!(lines[..2], [Line::Heading(0), Line::Model(1)]);
        assert_eq!(
            browse.take(&catalog, &models),
            Outcome::Chosen("claude-opus-5".into())
        );
        // An ID nobody lists is offered as typed; nothing else matches it.
        browse.typed("my-own-model".into());
        assert_eq!(
            browse.lines(&catalog, &models),
            [Line::Free("my-own-model".into())]
        );
        // Escape clears the search before anything else.
        assert_eq!(browse.key("escape", &catalog, &models), Outcome::Moved);
        assert!(browse.query.is_empty());
    }

    #[test]
    fn ids_with_a_provider_prefix_group_by_it_and_plain_ids_are_a_flat_list() {
        let routed = vec![
            model("anthropic/claude-opus-5", "Opus"),
            model("openai/gpt-6", "GPT 6"),
            model("anthropic/claude-fable-5", "Fable"),
            model("local-model", "Local"),
        ];
        let catalog = Catalog::of(&routed, &[]);
        let groups: Vec<(&str, &str, &[usize])> = catalog
            .groups
            .iter()
            .map(|group| {
                (
                    group.id.as_str(),
                    group.name.as_str(),
                    group.models.as_slice(),
                )
            })
            .collect();
        assert_eq!(
            groups,
            [
                ("anthropic", "Anthropic", &[0, 2][..]),
                ("openai", "OpenAI", &[1][..]),
                ("", "Other", &[3][..]),
            ]
        );
        let plain = vec![model("sonnet", "Sonnet 4.5"), model("opus", "Opus 4.5")];
        let catalog = Catalog::of(&plain, &[]);
        assert!(catalog.flat);
        let mut browse = Browse::opening(&catalog, &plain, Some("opus"));
        let lines = browse.lines(&catalog, &plain);
        assert_eq!(lines, [Line::Model(0), Line::Model(1)]);
        assert_eq!(browse.current(&lines), Some(&Line::Model(1)));
        browse.typed("son".into());
        assert_eq!(
            browse.lines(&catalog, &plain),
            [Line::Model(0), Line::Free("son".into())]
        );
        assert_eq!(browse.key("left", &catalog, &plain), Outcome::Ignored);
    }

    #[test]
    fn one_provider_opens_at_once_and_its_models_show() {
        let models = vec![model("a/one", "One"), model("a/two", "Two")];
        let catalog = Catalog::of(&models, &[]);
        let browse = Browse::opening(&catalog, &models, Some("a/two"));
        let lines = browse.lines(&catalog, &models);
        assert_eq!(lines, [Line::Back(0), Line::Model(0), Line::Model(1)]);
        assert_eq!(browse.current(&lines), Some(&Line::Model(1)));
    }

    #[test]
    fn a_model_s_provider_is_what_the_backend_says_then_its_prefix_then_its_name() {
        let (_, providers) = kou();
        assert_eq!(provider_of("grok-4.7", &providers).as_deref(), Some("xai"));
        assert_eq!(
            provider_of("openrouter/qwen3", &[]).as_deref(),
            Some("openrouter")
        );
        assert_eq!(
            provider_of("claude-sonnet-5", &[]).as_deref(),
            Some("anthropic")
        );
        assert_eq!(provider_of("sonnet", &[]), None);
        assert_eq!(provider_name("openai"), "OpenAI");
        assert_eq!(provider_name("acme"), "Acme");
    }
}
