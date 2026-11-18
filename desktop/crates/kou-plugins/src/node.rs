//! What a plugin draws, as the app reads it.

use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

/// An element a plugin built: its kind (`box`, `text`, `icon`, `button`,
/// ...), what it was given, and what it holds. The app draws the kinds and
/// reads the properties it knows; the rest is passed by.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct Node {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub children: Vec<Node>,
    #[serde(flatten)]
    pub props: Props,
}

/// What an element was given, by name. An element has a few of these and
/// the app asks it for many it has not (every one it knows, each time the
/// element is drawn): so they are a short list looked through in order,
/// where a map would work out a hash of every name asked for.
///
/// Most names asked for are not there: each name it has sets one bit of
/// `tags` (by its length and its ends), and a name whose bit is not set is
/// not looked for at all.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Props {
    given: Vec<(String, Value)>,
    tags: u64,
}

/// The bit a name sets in [`Props`]'s tags. Worked out when the program
/// is built for names written in it, so that a reader can ask in one step
/// whether an element may have any of several names (see [`Props::any`]).
pub const fn tag(name: &str) -> u64 {
    let bytes = name.as_bytes();
    let (first, last) = match bytes.len() {
        0 => (0, 0),
        len => (bytes[0] as usize, bytes[len - 1] as usize),
    };
    1 << ((bytes.len() * 7 + first * 3 + last) % 64)
}

/// The tags of several names together: what [`Props::any`] is asked with.
pub const fn tags(names: &[&str]) -> u64 {
    let mut tags = 0;
    let mut at = 0;
    while at < names.len() {
        tags |= tag(names[at]);
        at += 1;
    }
    tags
}

impl Props {
    /// What it was given under `name`.
    pub fn get(&self, name: &str) -> Option<&Value> {
        if self.tags & tag(name) == 0 {
            return None;
        }
        self.given
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// Whether it may have been given any of the names whose [`tags`]
    /// these are: when not, none of them is there.
    pub fn any(&self, tags: u64) -> bool {
        self.tags & tags != 0
    }

    /// Each name and what was given under it, in the order given.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.given.iter().map(|(key, value)| (key.as_str(), value))
    }
}

impl std::ops::Index<&str> for Props {
    type Output = Value;

    /// What it was given under `name`, or `null`.
    fn index(&self, name: &str) -> &Value {
        self.get(name).unwrap_or(&Value::Null)
    }
}

impl<'de> Deserialize<'de> for Props {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let given: Vec<(String, Value)> = Map::deserialize(deserializer)?.into_iter().collect();
        let tags = given.iter().fold(0, |tags, (key, _)| tags | tag(key));
        Ok(Self { given, tags })
    }
}

impl Node {
    /// A property as text.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.props.get(name)?.as_str()
    }

    /// A property as a number.
    pub fn number(&self, name: &str) -> Option<f32> {
        self.props.get(name)?.as_f64().map(|value| value as f32)
    }

    /// A property as a yes or a no; one that is not there is a no.
    pub fn flag(&self, name: &str) -> bool {
        self.props.get(name).and_then(Value::as_bool) == Some(true)
    }

    /// The handler a property holds: the number the plugin's function is
    /// called back by (see [`crate::Host::dispatch`]).
    pub fn handler(&self, name: &str) -> Option<u64> {
        self.props.get(name)?.get("$h")?.as_u64()
    }

    /// Every element of the tree, this one first, with how deep it is.
    pub fn walk(&self) -> Vec<(usize, &Node)> {
        fn visit<'a>(node: &'a Node, depth: usize, out: &mut Vec<(usize, &'a Node)>) {
            out.push((depth, node));
            for child in &node.children {
                visit(child, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        visit(self, 0, &mut out);
        out
    }

    /// The first element of the tree given `id` by its plugin.
    pub fn find(&self, id: &str) -> Option<&Node> {
        self.walk()
            .into_iter()
            .map(|(_, node)| node)
            .find(|node| node.text("id") == Some(id))
    }

    /// All the text the tree shows, in order, a space between pieces.
    pub fn said(&self) -> String {
        self.walk()
            .into_iter()
            .filter_map(|(_, node)| match node.kind.as_str() {
                "text" => node.text("value"),
                "button" => node.text("label"),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}
