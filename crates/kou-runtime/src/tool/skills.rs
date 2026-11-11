use super::*;
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::RwLock;

pub type RegistrationId = uuid::Uuid;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Skill {
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub description: String,
    #[serde(deserialize_with = "null_default")]
    pub path: String,
    #[serde(deserialize_with = "null_default")]
    pub manual: bool,
}
fn null_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    de: D,
) -> std::result::Result<T, D::Error> {
    Option::<T>::deserialize(de).map(Option::unwrap_or_default)
}

struct Entry {
    id: RegistrationId,
    skill: Arc<Skill>,
}
/// Shared metadata only: no skill-file contents or translator/catalog ownership cycle.
#[derive(Clone, Default)]
pub struct SkillCatalog {
    entries: Arc<RwLock<Vec<Entry>>>,
}
impl SkillCatalog {
    pub fn register(&self, skill: Skill) -> Result<RegistrationId> {
        for (value, field) in [
            (&skill.path, "path"),
            (&skill.name, "name"),
            (&skill.description, "description"),
        ] {
            if value.trim().is_empty() {
                return Err(Error(format!("skill {field} must be set")));
            }
        }
        let mut entries = self.entries.write().unwrap();
        if entries.iter().any(|entry| entry.skill.path == skill.path) {
            return Err(Error(format!(
                "skill path {} is already registered",
                quoted(&skill.path)
            )));
        }
        if entries.iter().any(|entry| entry.skill.name == skill.name) {
            return Err(Error(format!(
                "skill name {} is already registered",
                quoted(&skill.name)
            )));
        }
        let id = RegistrationId::new_v4();
        entries.push(Entry {
            id,
            skill: Arc::new(skill),
        });
        Ok(id)
    }
    pub fn unregister(&self, id: RegistrationId) {
        if !id.is_nil() {
            self.entries.write().unwrap().retain(|entry| entry.id != id);
        }
    }
    pub fn skills(&self) -> Vec<Arc<Skill>> {
        self.entries
            .read()
            .unwrap()
            .iter()
            .map(|entry| entry.skill.clone())
            .collect()
    }
    pub fn resolve(&self, name: &str) -> Option<Arc<Skill>> {
        self.entries
            .read()
            .unwrap()
            .iter()
            .find(|entry| entry.skill.name == name)
            .map(|entry| entry.skill.clone())
    }
}

pub(super) use kou_domain::go_quote as quoted;

#[cfg(test)]
mod tests;
