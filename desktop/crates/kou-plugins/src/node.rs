//! What a plugin draws, as the app reads it.

use serde::Deserialize;
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
    pub props: Map<String, Value>,
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
