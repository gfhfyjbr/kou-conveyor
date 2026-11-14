//! Where a session's modes and models come from, and how each is changed.
//!
//! The protocol has two generations of this. Session config options are the
//! current one: a list of selectors of which the ones in the `mode` and `model`
//! categories map onto the thread. The dedicated `modes` field with
//! `session/set_mode` is the older one, kept by agents for older clients. Model
//! selection exists only as a config option.

use agent_client_protocol_schema::v1 as acp;
use kou_thread::{Choice, Provider};

/// How a selector's value is changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// `session/set_config_option` with this option id.
    Option(String),
    /// `session/set_mode`.
    Modes,
}

/// One thing the user can pick: the session's mode, or its model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selector {
    pub source: Source,
    pub choices: Vec<Choice>,
    pub current: Option<String>,
}

/// Which of the two selectors a request is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Mode,
    Model,
}

impl Target {
    /// For a sentence: `mode`, `model`.
    pub fn noun(self) -> &'static str {
        match self {
            Target::Mode => "mode",
            Target::Model => "model",
        }
    }
}

/// The values that changed, to tell the thread.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Changed {
    pub mode: Option<String>,
    pub model: Option<String>,
    /// The groups the agent offers its models in, when those changed.
    pub providers: Option<Vec<Provider>>,
}

/// The mode and the model selector of a session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Selectors {
    pub mode: Option<Selector>,
    pub model: Option<Selector>,
    /// The groups an agent offers its models in (by who serves them, as
    /// a rule): what a picker shows before the models themselves. Empty for
    /// an agent that offers one list.
    pub providers: Vec<Provider>,
}

impl Selectors {
    /// From what `session/new`, `session/load` or `session/resume` returned.
    pub fn new(
        modes: Option<acp::SessionModeState>,
        options: Option<Vec<acp::SessionConfigOption>>,
    ) -> Self {
        let mut selectors = Self {
            mode: modes.map(|state| Selector {
                source: Source::Modes,
                choices: state
                    .available_modes
                    .into_iter()
                    .map(|mode| Choice {
                        id: mode.id.0.to_string(),
                        name: mode.name,
                        description: mode.description,
                    })
                    .collect(),
                current: Some(state.current_mode_id.0.to_string()),
            }),
            model: None,
            providers: Vec::new(),
        };
        if let Some(options) = options {
            selectors.replace_options(options);
        }
        selectors
    }

    /// Take a complete new list of config options, as every answer and
    /// notification about them carries, and say which current values moved.
    pub fn replace_options(&mut self, options: Vec<acp::SessionConfigOption>) -> Changed {
        let before = (self.current(Target::Mode), self.current(Target::Model));
        let mut mode = None;
        let mut model = None;
        let mut providers = Vec::new();
        // The first option of a category wins, as the agent orders them by priority.
        for option in options {
            let models = matches!(
                option.category,
                Some(acp::SessionConfigOptionCategory::Model)
            );
            let slot = match option.category {
                Some(acp::SessionConfigOptionCategory::Mode) => &mut mode,
                Some(acp::SessionConfigOptionCategory::Model) => &mut model,
                _ => continue,
            };
            if slot.is_some() {
                continue;
            }
            let acp::SessionConfigKind::Select(select) = option.kind else {
                continue;
            };
            let choices = match select.options {
                acp::SessionConfigSelectOptions::Ungrouped(options) => options,
                acp::SessionConfigSelectOptions::Grouped(groups) => {
                    if models {
                        providers = groups
                            .iter()
                            .map(|group| Provider {
                                id: group.group.0.to_string(),
                                name: group.name.clone(),
                                models: group
                                    .options
                                    .iter()
                                    .map(|option| option.value.0.to_string())
                                    .collect(),
                            })
                            .collect();
                    }
                    groups.into_iter().flat_map(|group| group.options).collect()
                }
                _ => Vec::new(),
            };
            *slot = Some(Selector {
                source: Source::Option(option.id.0.to_string()),
                choices: choices
                    .into_iter()
                    .map(|choice| Choice {
                        id: choice.value.0.to_string(),
                        name: choice.name,
                        description: choice.description,
                    })
                    .collect(),
                current: Some(select.current_value.0.to_string()),
            });
        }
        // An agent that offers its modes only the older way keeps them.
        if mode.is_some()
            || matches!(&self.mode, Some(selector) if selector.source != Source::Modes)
        {
            self.mode = mode;
        }
        self.model = model;
        let regrouped = (providers != self.providers).then(|| providers.clone());
        self.providers = providers;
        let moved = |target, before: Option<String>| {
            self.current(target)
                .filter(|current| Some(current) != before.as_ref())
        };
        Changed {
            mode: moved(Target::Mode, before.0),
            model: moved(Target::Model, before.1),
            providers: regrouped,
        }
    }

    /// The selector for `target`, if the agent offers one.
    pub fn get(&self, target: Target) -> Option<&Selector> {
        match target {
            Target::Mode => self.mode.as_ref(),
            Target::Model => self.model.as_ref(),
        }
    }

    /// The value in force.
    pub fn current(&self, target: Target) -> Option<String> {
        self.get(target)?.current.clone()
    }

    /// Record a value the agent confirmed or announced. `true` if it is news.
    pub fn set(&mut self, target: Target, value: &str) -> bool {
        let selector = match target {
            Target::Mode => &mut self.mode,
            Target::Model => &mut self.model,
        };
        // A mode announced by an agent that never listed its modes still counts.
        let selector = selector.get_or_insert_with(|| Selector {
            source: Source::Modes,
            choices: Vec::new(),
            current: None,
        });
        if selector.current.as_deref() == Some(value) {
            return false;
        }
        selector.current = Some(value.to_owned());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Parsed the way they arrive, where an option of a type this client
    /// does not know is skipped instead of failing the whole list.
    fn options(value: serde_json::Value) -> Vec<acp::SessionConfigOption> {
        let response: acp::SetSessionConfigOptionResponse =
            serde_json::from_value(json!({ "configOptions": value })).expect("config options");
        response.config_options
    }

    fn choice(id: &str, name: &str, description: Option<&str>) -> Choice {
        Choice {
            id: id.into(),
            name: name.into(),
            description: description.map(str::to_owned),
        }
    }

    fn spec_options(mode: &str, model: &str) -> serde_json::Value {
        json!([
            {"id": "mode", "name": "Session Mode", "category": "mode", "type": "select",
             "currentValue": mode,
             "options": [
                {"value": "ask", "name": "Ask", "description": "Request permission before making any changes"},
                {"value": "code", "name": "Code"}]},
            {"id": "llm", "name": "Model", "category": "model", "type": "select",
             "currentValue": model,
             "options": [
                {"group": "fast", "name": "Fast", "options": [{"value": "model-1", "name": "Model 1"}]},
                {"group": "strong", "name": "Strong", "options": [{"value": "model-2", "name": "Model 2"}]}]},
            {"id": "second-model", "name": "Other", "category": "model", "type": "select",
             "currentValue": "x", "options": [{"value": "x", "name": "X"}]},
            {"id": "context_size", "name": "Context Size", "category": "model_config", "type": "select",
             "currentValue": "200k", "options": [{"value": "200k", "name": "200K"}]},
            {"id": "brave", "name": "Brave", "category": "mode", "type": "boolean", "currentValue": true},
            {"id": "odd", "name": "Odd", "type": "slider", "currentValue": 3}
        ])
    }

    #[test]
    fn config_options_give_the_mode_and_the_model_selector() {
        let selectors = Selectors::new(None, Some(options(spec_options("ask", "model-1"))));
        assert_eq!(
            selectors.mode,
            Some(Selector {
                source: Source::Option("mode".into()),
                choices: vec![
                    choice(
                        "ask",
                        "Ask",
                        Some("Request permission before making any changes")
                    ),
                    choice("code", "Code", None),
                ],
                current: Some("ask".into()),
            })
        );
        // Groups are flattened; the first option of the category is the one.
        assert_eq!(
            selectors.model,
            Some(Selector {
                source: Source::Option("llm".into()),
                choices: vec![
                    choice("model-1", "Model 1", None),
                    choice("model-2", "Model 2", None)
                ],
                current: Some("model-1".into()),
            })
        );
    }

    #[test]
    fn a_new_list_reports_only_the_values_that_moved() {
        let mut selectors = Selectors::new(None, Some(options(spec_options("ask", "model-1"))));
        assert_eq!(
            selectors.replace_options(options(spec_options("ask", "model-1"))),
            Changed::default()
        );
        assert_eq!(
            selectors.replace_options(options(spec_options("code", "model-2"))),
            Changed {
                mode: Some("code".into()),
                model: Some("model-2".into()),
                providers: None,
            }
        );
        // The groups the models come in are what a picker shows first, and
        // are told again only when they change.
        let group = |id: &str, name: &str, models: &[&str]| Provider {
            id: id.into(),
            name: name.into(),
            models: models.iter().map(|model| (*model).to_owned()).collect(),
        };
        assert_eq!(
            selectors.providers,
            [
                group("fast", "Fast", &["model-1"]),
                group("strong", "Strong", &["model-2"])
            ]
        );
        let one_list = json!([
            {"id": "llm", "name": "Model", "category": "model", "type": "select",
             "currentValue": "model-2",
             "options": [{"value": "model-1", "name": "Model 1"}, {"value": "model-2", "name": "Model 2"}]}
        ]);
        assert_eq!(
            selectors.replace_options(options(one_list)).providers,
            Some(Vec::new())
        );
        assert_eq!(
            selectors
                .replace_options(options(spec_options("code", "model-2")))
                .providers
                .map(|providers| providers.len()),
            Some(2)
        );
        assert!(!selectors.set(Target::Mode, "code"));
        assert!(selectors.set(Target::Mode, "ask"));
        assert_eq!(selectors.current(Target::Mode), Some("ask".into()));
    }

    #[test]
    fn the_older_modes_serve_when_no_option_is_a_mode() {
        let modes: acp::SessionModeState = serde_json::from_value(json!({
            "currentModeId": "ask",
            "availableModes": [
                {"id": "ask", "name": "Ask", "description": "Request permission first"},
                {"id": "code", "name": "Code"}]
        }))
        .unwrap();
        let only_model = json!([{"id": "model", "name": "Model", "category": "model",
            "type": "select", "currentValue": "m", "options": [{"value": "m", "name": "M"}]}]);

        let mut selectors = Selectors::new(Some(modes.clone()), Some(options(only_model.clone())));
        let mode = selectors.mode.clone().expect("the older modes");
        assert_eq!(mode.source, Source::Modes);
        assert_eq!(
            mode.choices[0],
            choice("ask", "Ask", Some("Request permission first"))
        );
        assert_eq!(
            selectors.model.as_ref().unwrap().source,
            Source::Option("model".into())
        );
        // A later list without a mode option does not take the modes away.
        assert_eq!(
            selectors.replace_options(options(only_model)),
            Changed::default()
        );
        assert_eq!(selectors.mode, Some(mode));

        // With a mode option the options win, as the specification asks.
        let both = Selectors::new(Some(modes), Some(options(spec_options("code", "model-1"))));
        assert_eq!(both.mode.unwrap().source, Source::Option("mode".into()));

        let mut none = Selectors::new(None, None);
        assert_eq!(none.get(Target::Mode), None);
        assert!(none.set(Target::Mode, "plan"));
        assert_eq!(none.current(Target::Mode), Some("plan".into()));
    }
}
