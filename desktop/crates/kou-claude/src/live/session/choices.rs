//! What the session offers to choose and what is chosen: the permission
//! modes, the models, and how hard the model in force is asked to think
//! (its level of effort).
//!
//! The levels a model takes are its row of the greeting's `models`
//! (`supportsEffort`, `supportedEffortLevels`); the level in force is what
//! `get_settings` says (`applied.effort`), asked once the session is ready
//! and again when the model changes, and what an `init` says when it
//! carries `effort`. A level chosen goes to Claude Code's flag settings
//! (`apply_flag_settings`), as the SDK's `applyFlagSettings` sends it.

use super::{Pending, Session, wire};
use kou_thread::{Choice, Level, Update};
use serde_json::{Value, json};

/// The mode a session in `auto` goes to when its model cannot run in it,
/// as the ACP adapter has it (`AUTO_MODE_FALLBACK`).
const AUTO_FALLBACK: &str = "acceptEdits";

impl Session {
    /// The row of the greeting's models of the model in force, by its
    /// choice or the model it stands for.
    fn model_row(&self) -> Option<&wire::Model> {
        let model = self.model.as_deref()?;
        self.models
            .iter()
            .find(|row| row.choice.id == model)
            .or_else(|| {
                self.models
                    .iter()
                    .find(|row| row.resolved.as_deref() == Some(model))
            })
    }

    /// The modes on offer: `auto` while the model in force can run in it,
    /// or while Claude Code says it runs in it.
    pub(super) fn modes(&self) -> Vec<Choice> {
        let auto =
            self.model_row().is_some_and(|row| row.auto) || self.mode.as_deref() == Some("auto");
        wire::modes(auto)
    }

    /// The modes on offer changed: the session is announced again, as they
    /// are part of [`Update::Ready`].
    fn refresh_modes(&mut self) {
        if self.ready && self.modes() != self.modes_said {
            self.announce();
        }
    }

    pub(super) fn set_mode(&mut self, mode: String) {
        self.request(
            json!({"subtype": "set_permission_mode", "mode": mode}),
            Pending::SetMode(mode),
        );
    }

    pub(super) fn note_mode(&mut self, mode: &str) {
        if self.mode.as_deref() == Some(mode) {
            return;
        }
        self.mode = Some(mode.to_owned());
        if self.ready {
            self.send(Update::Mode(mode.to_owned()));
            self.refresh_modes();
        }
    }

    pub(super) fn note_model(&mut self, model: &str) {
        if self.model.as_deref() == Some(model) {
            return;
        }
        self.model = Some(model.to_owned());
        self.send(Update::Model(model.to_owned()));
        if !self.ready {
            return;
        }
        self.refresh_modes();
        // A model that cannot run in `auto` is not left in it: the adapter
        // switches such a session to Accept edits, and says so.
        if self.mode.as_deref() == Some("auto") && self.model_row().is_some_and(|row| !row.auto) {
            self.notice(
                Level::Warning,
                "The model chosen cannot run in the Auto mode, so the mode is now Accept edits."
                    .to_owned(),
            );
            self.set_mode(AUTO_FALLBACK.to_owned());
        }
        // The level of effort goes with the model (each keeps its own, and a
        // level the new one lacks is lowered): asked anew.
        self.request(wire::get_settings(), Pending::Settings(None));
    }

    /// The model Claude Code says it runs (`init`), which may be the one an
    /// offered choice stands for: a choice other than the one in force is
    /// said.
    pub(super) fn note_running_model(&mut self, running: &str) {
        let stands_for = |model: &wire::Model| {
            model.choice.id == running || model.resolved.as_deref() == Some(running)
        };
        let current = self
            .models
            .iter()
            .find(|model| Some(model.choice.id.as_str()) == self.model.as_deref());
        if self.model.as_deref() == Some(running) || current.is_some_and(stands_for) {
            return;
        }
        if let Some(model) = self.models.iter().find(|model| stands_for(model)) {
            let id = model.choice.id.clone();
            self.note_model(&id);
        }
    }

    pub(super) fn set_effort(&mut self, level: String) {
        self.request(wire::set_effort(&level), Pending::SetEffort(level));
    }

    /// Claude Code's answer to a level of effort chosen. It takes any
    /// level it is sent (2.1.325 answers success for one that does not
    /// exist): what holds is asked of it.
    pub(super) fn on_effort_set(&mut self, level: String, answer: Result<Value, String>) {
        match answer {
            Ok(_) => self.request(wire::get_settings(), Pending::Settings(Some(level))),
            Err(error) => self.send(Update::Notice(
                Level::Warning,
                format!("Claude Code did not switch to the effort level {level}: {error}"),
            )),
        }
    }

    /// Claude Code's answer to `get_settings`: the level of effort in force.
    /// `asked`: the level just set, which is the user's from now on if it
    /// holds, and said not to otherwise.
    pub(super) fn on_settings(&mut self, answer: Result<Value, String>, asked: Option<String>) {
        let applied = match answer {
            Ok(answer) => wire::applied_effort(&answer),
            // (An older Claude Code: the levels are offered all the same.)
            Err(error) => {
                log::debug!("{} did not say its settings: {error}", self.program);
                None
            }
        };
        if let Some(effort) = &applied {
            self.effort = effort.clone();
        }
        if let Some(level) = asked {
            match applied {
                Some(Some(holds)) if holds != level => self.send(Update::Notice(
                    Level::Warning,
                    format!("Claude Code did not switch to the effort level {level}: it runs at {holds}."),
                )),
                Some(None) => self.send(Update::Notice(
                    Level::Warning,
                    format!("Claude Code did not switch to the effort level {level}: this model takes none."),
                )),
                // It holds, or nothing says otherwise.
                _ => {
                    if applied.is_none() {
                        self.effort = Some(level.clone());
                    }
                    self.pinned_effort = Some(level);
                }
            }
        }
        self.say_efforts();
    }

    /// The level of effort an `init` says is in force for the next message.
    pub(super) fn note_effort(&mut self, effort: Option<String>) {
        if self.effort != effort {
            self.effort = effort;
            if self.ready {
                self.say_efforts();
            }
        }
    }

    /// Say the levels of effort of the model in force and the one in force,
    /// unless that is what was said last.
    pub(super) fn say_efforts(&mut self) {
        let levels = self
            .model_row()
            .map(|row| wire::effort_choices(&row.efforts))
            .unwrap_or_default();
        let current = if levels.is_empty() {
            None
        } else {
            self.effort.clone()
        };
        let said = (levels, current);
        if self.efforts_said.as_ref() == Some(&said) {
            return;
        }
        self.efforts_said = Some(said.clone());
        let (levels, current) = said;
        self.send(Update::Efforts { levels, current });
    }
}
