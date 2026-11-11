//! The tools the model is offered and the translators its calls resolve to
//! (Go `harness/tool/registry.go`): the enabled built-in tools in their fixed
//! order, then the registered ones in registration order.
use super::*;
use definitions::built_in;
use skills::quoted;
use std::sync::{PoisonError, RwLock, RwLockReadGuard};

/// The built-in translators the run configured. An enabled tool left unset
/// answers every call with a configuration error instead of being unknown.
#[derive(Clone, Default)]
pub struct StaticTranslators {
    pub bash: Option<Arc<dyn Translator>>,
    pub view_image: Option<Arc<dyn Translator>>,
    pub read: Option<Arc<dyn Translator>>,
    pub edit: Option<Arc<dyn Translator>>,
    pub write: Option<Arc<dyn Translator>>,
    pub apply_patch: Option<Arc<dyn Translator>>,
    pub code: Option<Arc<dyn Translator>>,
    pub transcript_search: Option<Arc<dyn Translator>>,
}

struct Registered {
    definition: Arc<Definition>,
    translator: Arc<dyn Translator>,
}

pub struct Registry {
    /// Fixed at construction, by index into [`STATIC_NAMES`]; reads need no lock.
    enabled: [bool; STATIC_NAMES.len()],
    built_in: [Arc<dyn Translator>; STATIC_NAMES.len()],
    registered: RwLock<Vec<Registered>>,
    skills: SkillCatalog,
}

impl Registry {
    /// Only the names in `enabled` are offered and resolved; repeats and names
    /// of no built-in tool change nothing.
    pub fn new<N: AsRef<str>>(
        configured: StaticTranslators,
        enabled: impl IntoIterator<Item = N>,
    ) -> Self {
        let skills = SkillCatalog::default();
        let mut selected = [false; STATIC_NAMES.len()];
        for name in enabled {
            if let Some(slot) = slot(name.as_ref()) {
                selected[slot] = true;
            }
        }
        let mut configured = [
            configured.bash,
            configured.read,
            configured.edit,
            configured.write,
            configured.apply_patch,
            configured.view_image,
            configured.transcript_search,
            configured.code,
            Some(Arc::new(SkillUse::new(skills.clone())) as Arc<dyn Translator>),
        ];
        Self {
            enabled: selected,
            built_in: std::array::from_fn(|slot| {
                configured[slot].take().unwrap_or_else(|| {
                    Arc::new(Unconfigured {
                        name: STATIC_NAMES[slot],
                    })
                })
            }),
            registered: RwLock::default(),
            skills,
        }
    }

    /// The definitions of the enabled tools: the built-in ones, then those registered.
    pub fn static_definitions(&self) -> Vec<Arc<Definition>> {
        let registered = self.registered();
        self.enabled_slots()
            .map(|slot| built_in()[slot].clone())
            .chain(registered.iter().map(|tool| tool.definition.clone()))
            .collect()
    }

    /// The names of the enabled tools, in the order of their definitions.
    pub fn names(&self) -> Vec<String> {
        let registered = self.registered();
        self.enabled_slots()
            .map(|slot| STATIC_NAMES[slot].to_owned())
            .chain(
                registered
                    .iter()
                    .map(|tool| tool.definition.tool.name.clone()),
            )
            .collect()
    }

    /// Finds the translator of the tool a call names. A gateway between the
    /// harness and the model may show the tools under names of its own, such as
    /// `mcp__<server>__<word>_Read`; a model that shortens such a name sends one
    /// no tool has, which resolves to the one tool its end names, if exactly one
    /// does ([`aliases`]).
    pub fn resolve(&self, name: &str) -> Option<Arc<dyn Translator>> {
        let registered = self.registered();
        if let Some(translator) = self.exact(&registered, name) {
            return Some(translator);
        }
        let mut ends = ends(name).filter_map(|end| self.exact(&registered, end));
        let translator = ends.next()?;
        ends.next().is_none().then_some(translator)
    }

    /// Adds a tool beside the built-in ones, such as a plugin's. Its name may
    /// not be taken by an enabled tool; a built-in tool that is not enabled
    /// leaves its name free.
    pub fn register_tool(
        &self,
        definition: Definition,
        translator: Option<Arc<dyn Translator>>,
    ) -> Result<()> {
        let name = &definition.tool.name;
        if name.trim().is_empty() {
            return Err(Error("tool name must be set".into()));
        }
        let Some(translator) = translator else {
            return Err(Error(format!("tool {} has no translator", quoted(name))));
        };
        if slot(name).is_some_and(|slot| self.enabled[slot]) {
            return Err(Error(format!("tool {} is built in", quoted(name))));
        }
        let mut registered = self
            .registered
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if registered
            .iter()
            .any(|tool| tool.definition.tool.name == *name)
        {
            return Err(Error(format!(
                "tool {} is already registered",
                quoted(name)
            )));
        }
        registered.push(Registered {
            definition: Arc::new(definition),
            translator,
        });
        Ok(())
    }

    /// The error of a call to a tool no name resolves: which tools there are,
    /// or which of them the name may stand for.
    pub fn unavailable(&self, name: &str) -> String {
        unavailable(name, &self.names())
    }

    pub fn register_skill(&self, skill: Skill) -> Result<RegistrationId> {
        self.skills.register(skill)
    }
    pub fn unregister_skill(&self, id: RegistrationId) {
        self.skills.unregister(id);
    }
    pub fn skills(&self) -> Vec<Arc<Skill>> {
        self.skills.skills()
    }
    /// The catalog the registry's own SkillUse translator reads.
    pub fn skill_catalog(&self) -> &SkillCatalog {
        &self.skills
    }

    fn registered(&self) -> RwLockReadGuard<'_, Vec<Registered>> {
        self.registered
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }
    fn enabled_slots(&self) -> impl Iterator<Item = usize> {
        (0..STATIC_NAMES.len()).filter(|slot| self.enabled[*slot])
    }
    fn exact(&self, registered: &[Registered], name: &str) -> Option<Arc<dyn Translator>> {
        if let Some(slot) = slot(name).filter(|slot| self.enabled[*slot]) {
            return Some(self.built_in[slot].clone());
        }
        registered
            .iter()
            .find(|tool| tool.definition.tool.name == name)
            .map(|tool| tool.translator.clone())
    }
}

fn slot(name: &str) -> Option<usize> {
    STATIC_NAMES.iter().position(|known| *known == name)
}

/// The ends of `name` that follow an underscore which is not its first byte
/// and is not followed by another underscore, longest first.
fn ends(name: &str) -> impl Iterator<Item = &str> {
    let bytes = name.as_bytes();
    (1..bytes.len().saturating_sub(1))
        .filter(move |index| bytes[*index] == b'_' && bytes[index + 1] != b'_')
        .map(move |index| &name[index + 1..])
}

/// Returns the tools of `names` that `name`, which none of them has, may stand
/// for: the ends of `name` that follow an underscore, such as `Read` for
/// `mcp__server__grace_Read`, `server__grace_Read` or `grace_Read`, and
/// `apply_patch` for `grace_apply_patch`. They come sorted, each once.
pub fn aliases<'a, N: AsRef<str>>(name: &'a str, names: &[N]) -> Vec<&'a str> {
    let mut matches: Vec<_> = ends(name)
        .filter(|end| names.iter().any(|known| known.as_ref() == *end))
        .collect();
    matches.sort_unstable();
    matches
}

/// The error of a call to a tool that is none of `names`, the tools there are.
pub fn unavailable<N: AsRef<str>>(name: &str, names: &[N]) -> String {
    let matches = aliases(name, names);
    if matches.len() > 1 {
        return format!(
            "tool {} is not available: it may stand for any of {}; call the one you mean by its exact name",
            quoted(name),
            matches.join(", ")
        );
    }
    if names.is_empty() {
        return format!("tool {} is not available", quoted(name));
    }
    let names: Vec<&str> = names.iter().map(AsRef::as_ref).collect();
    format!(
        "tool {} is not available; the tools are {}",
        quoted(name),
        names.join(", ")
    )
}

/// Stands in an enabled built-in slot the run gave no translator.
struct Unconfigured {
    name: &'static str,
}
impl Unconfigured {
    fn message(&self) -> String {
        format!("static tool {} is not configured", quoted(self.name))
    }
}
impl Translator for Unconfigured {
    fn translate(&self, _: &str, _: &mut dyn FnMut(Spec) -> String) -> CallStatus {
        CallStatus {
            error: self.message(),
            ..CallStatus::default()
        }
    }
    fn result(&self, call_id: &str, _: &CallStatus, _: &[Arc<Operation>]) -> Result<ToolResult> {
        Ok(text_result(call_id, self.message()))
    }
}

#[cfg(test)]
mod tests;
