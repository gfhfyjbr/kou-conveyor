//! What the user can put on the canvas, as the app tells it, and the
//! templates an empty canvas offers. The canvas knows nothing of harnesses:
//! the app hands it a list of [`Offer`]s, the canvas shows them in its add
//! menu, and what the user picks goes back to the app as a request the app
//! fulfils by starting a pane and calling [`crate::CanvasView::fulfil`].

use crate::model::{EdgeMode, NodeKind, Wire};

/// Something the app can start as a node: a harness, a shell, a native agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// The app's id for it; it comes back in the request.
    pub id: String,
    /// Its name in the menu.
    pub title: String,
    /// One or two letters for its mark.
    pub code: String,
    /// The kind of node it makes.
    pub kind: NodeKind,
    /// A line under its name: its version, or why it cannot start.
    pub detail: String,
    /// Whether it can be started now; one that cannot shows, and cannot be picked.
    pub ready: bool,
}

/// The user asked for a node from the app's offers.
#[derive(Clone, Debug, PartialEq)]
pub struct AddRequest {
    /// The offer's id.
    pub offer: String,
    /// The world point it was asked for at.
    pub x: f32,
    /// See `x`.
    pub y: f32,
    /// The edge the user drew to that point, which the new node ends.
    pub wire: Option<Wire>,
}

/// A node of a template.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateNode {
    /// The offer that starts it.
    pub offer: String,
    /// Its title.
    pub title: String,
    /// Its top left corner, in world units from the template's origin.
    pub x: f32,
    /// See `x`.
    pub y: f32,
}

/// An edge of a template, between two of its nodes by index, output to input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateEdge {
    /// The node it starts at.
    pub from: usize,
    /// The node it ends at.
    pub to: usize,
    /// Its mode.
    pub mode: EdgeMode,
}

/// A small canvas to start from: nodes to start and how to wire them.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// Its name.
    pub title: String,
    /// What it is for.
    pub description: String,
    /// Its nodes.
    pub nodes: Vec<TemplateNode>,
    /// Its edges.
    pub edges: Vec<TemplateEdge>,
}

/// The user picked a template: the app starts each node (its offer) and hands
/// them back to [`crate::CanvasView::build_template`], in this order.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateRequest {
    /// The template, its places in world units around `x`, `y`.
    pub template: Template,
    /// Where on the board it goes.
    pub x: f32,
    /// See `x`.
    pub y: f32,
}

fn pick<'a>(
    offers: &'a [Offer],
    kind: NodeKind,
    prefer: &[&str],
    not: Option<&str>,
) -> Option<&'a Offer> {
    let ready = || {
        offers.iter().filter(move |offer| {
            offer.ready && offer.kind == kind && Some(offer.id.as_str()) != not
        })
    };
    prefer
        .iter()
        .find_map(|id| ready().find(|offer| offer.id == *id))
        .or_else(|| ready().next())
}

/// The templates the offers make possible, after the web canvas's gallery:
/// a pair that codes and reviews, and a lead that fans work out to shells.
pub fn templates(offers: &[Offer]) -> Vec<Template> {
    let mut list = Vec::new();
    let coder = pick(
        offers,
        NodeKind::Agent,
        &["claude", "codex", "opencode"],
        None,
    );
    if let Some(coder) = coder {
        let reviewer = pick(
            offers,
            NodeKind::Agent,
            &["kou", "codex", "claude"],
            Some(&coder.id),
        )
        .unwrap_or(coder);
        list.push(Template {
            title: "Pair".into(),
            description: format!(
                "{} codes; {} reviews each turn, and the review goes back once you approve it.",
                coder.title, reviewer.title
            ),
            nodes: vec![
                TemplateNode {
                    offer: coder.id.clone(),
                    title: "Coder".into(),
                    x: 0.,
                    y: 0.,
                },
                TemplateNode {
                    offer: reviewer.id.clone(),
                    title: "Reviewer".into(),
                    x: 800.,
                    y: 0.,
                },
            ],
            edges: vec![
                TemplateEdge {
                    from: 0,
                    to: 1,
                    mode: EdgeMode::Auto,
                },
                TemplateEdge {
                    from: 1,
                    to: 0,
                    mode: EdgeMode::Approve,
                },
            ],
        });
        let shell = pick(offers, NodeKind::Terminal, &["shell"], None).unwrap_or(coder);
        let mut nodes = vec![TemplateNode {
            offer: coder.id.clone(),
            title: "Lead".into(),
            x: 0.,
            y: 0.,
        }];
        let mut edges = Vec::new();
        for (i, name) in ["Worker A", "Worker B", "Worker C"].into_iter().enumerate() {
            nodes.push(TemplateNode {
                offer: shell.id.clone(),
                title: name.into(),
                x: 800.,
                y: (i as f32 - 1.) * 480.,
            });
            edges.push(TemplateEdge {
                from: 0,
                to: i + 1,
                mode: EdgeMode::Auto,
            });
            edges.push(TemplateEdge {
                from: i + 1,
                to: 0,
                mode: EdgeMode::Auto,
            });
        }
        list.push(Template {
            title: "Fan-out".into(),
            description: format!(
                "A lead splits the work among three {}s and gathers what they say.",
                shell.title.to_lowercase()
            ),
            nodes,
            edges,
        });
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(id: &str, kind: NodeKind, ready: bool) -> Offer {
        Offer {
            id: id.into(),
            title: id.to_uppercase(),
            code: id[..2].to_uppercase(),
            kind,
            detail: String::new(),
            ready,
        }
    }

    #[test]
    fn templates_use_what_can_start_and_wire_it_as_the_web_gallery_does() {
        assert!(
            templates(&[]).is_empty(),
            "nothing to start, nothing to offer"
        );
        let offers = [
            offer("gemini", NodeKind::Agent, false),
            offer("codex", NodeKind::Agent, true),
            offer("claude", NodeKind::Agent, true),
            offer("shell", NodeKind::Terminal, true),
        ];
        let list = templates(&offers);
        assert_eq!(list.len(), 2);
        let pair = &list[0];
        assert_eq!(pair.nodes[0].offer, "claude");
        assert_eq!(pair.nodes[1].offer, "codex");
        assert_eq!(
            pair.edges[1].mode,
            EdgeMode::Approve,
            "the review waits for you"
        );
        let fan = &list[1];
        assert_eq!(fan.nodes.len(), 4);
        assert!(fan.nodes[1..].iter().all(|node| node.offer == "shell"));
        assert_eq!(fan.edges.len(), 6);
        // Only one agent: it pairs with itself rather than not at all.
        let alone = templates(&[offer("claude", NodeKind::Agent, true)]);
        assert_eq!(alone[0].nodes[1].offer, "claude");
        assert_eq!(alone[1].nodes[1].offer, "claude");
    }
}
