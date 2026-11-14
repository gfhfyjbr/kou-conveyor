//! What the field holds when it is sent, and what a backend gets of it.
//!
//! The text names each picture by its label, `[Image 1]`, as the cockpits'
//! prompts do. A backend gets the picture in the label's place: the text up
//! to it, the picture, the text after it. kou's backend writes the label back
//! for the model; an ACP agent gets the picture where the user put it.

use crate::pictures::{self, Prepared};
use kou_thread::{
    Block,
    files::{FileLink, link_target},
};
use std::path::Path;

/// The most of a linked file embedded in a prompt, for a backend that takes
/// files' content.
const MAX_EMBEDDED: u64 = 256 * 1024;

/// A picture the text names by its label.
#[derive(Clone, Debug)]
pub(crate) struct Attached {
    pub label: String,
    pub prepared: Prepared,
}

/// The field's text and the pictures it names.
#[derive(Clone, Debug, Default)]
pub(crate) struct Draft {
    pub text: String,
    pub pictures: Vec<Attached>,
}

impl Draft {
    /// Whether there is nothing to send.
    pub(crate) fn is_blank(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// The blocks a backend gets: the text with each picture in its label's
    /// place, then, for a backend that takes files' content, the files the
    /// text links.
    pub(crate) fn blocks(&self, links: &[FileLink], cwd: &Path, embed: bool) -> Vec<Block> {
        let mut places: Vec<(usize, &Attached)> = self
            .pictures
            .iter()
            .filter_map(|picture| Some((self.text.find(&picture.label)?, picture)))
            .collect();
        places.sort_by_key(|(at, _)| *at);
        let mut blocks = Vec::new();
        let mut from = 0;
        for (at, picture) in places {
            if at < from {
                continue;
            }
            if at > from {
                blocks.push(Block::Text(self.text[from..at].to_owned()));
            }
            blocks.push(Block::Image {
                mime: picture.prepared.mime.to_owned(),
                data: picture.prepared.data.clone(),
            });
            from = at + picture.label.len();
        }
        if from < self.text.len() || blocks.is_empty() {
            blocks.push(Block::Text(self.text[from..].to_owned()));
        }
        if embed {
            for link in links.iter().filter(|link| !link.directory) {
                if let Some(text) = embedded(cwd, link) {
                    blocks.push(Block::File {
                        path: link_target(cwd, &link.path),
                        text: Some(text),
                    });
                }
            }
        }
        blocks
    }
}

/// What a backend that takes files' content gets of a linked file: the lines
/// the link asks for, or the beginning of the file. A file that is not text
/// is left to the reference in the text.
fn embedded(cwd: &Path, link: &FileLink) -> Option<String> {
    use std::io::Read as _;
    let file = std::fs::File::open(link_target(cwd, &link.path)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_EMBEDDED).read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    if link.start_line == 0 {
        return Some(text);
    }
    let first = link.start_line as usize - 1;
    let count = match link.end_line {
        0 => usize::MAX,
        end => end as usize - first,
    };
    Some(
        text.lines()
            .skip(first)
            .take(count)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// What a piece of text may start with right after a label, with no space between.
const CLOSING: &str = ".,;:!?)]}»…'\"";

/// A user message as the transcript shows it: its text with each picture's
/// label in the picture's place, and the pictures with their labels. The
/// labels are numbered as kou's backend numbers them for the model.
pub(crate) fn shown(blocks: &[Block]) -> (String, Vec<(String, usize)>) {
    let typed: String = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let mut number = pictures::label_numbers(&typed)
        .into_iter()
        .max()
        .map_or(1, |most| most + 1);
    let mut text = String::new();
    let mut labels = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        match block {
            Block::Text(piece) => {
                if !text.is_empty()
                    && labels.last().is_some_and(|(_, at)| *at + 1 == index)
                    && !text.ends_with(char::is_whitespace)
                    && !piece.starts_with(|ch: char| ch.is_whitespace() || CLOSING.contains(ch))
                {
                    text.push(' ');
                }
                text.push_str(piece);
            }
            Block::Image { .. } => {
                let label = pictures::label(number);
                number += 1;
                if !text.is_empty() && !text.ends_with(char::is_whitespace) {
                    text.push(' ');
                }
                text.push_str(&label);
                labels.push((label, index));
            }
            Block::File { .. } => {}
        }
    }
    (text, labels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pictures::{prepare, tests::picture};
    use image::ImageFormat;

    fn attached(label: &str) -> Attached {
        Attached {
            label: label.into(),
            prepared: prepare(&picture(8, 8, ImageFormat::Png)).expect("a picture"),
        }
    }

    #[test]
    fn a_picture_goes_in_its_label_s_place() {
        let draft = Draft {
            text: "compare [Image 2] with [Image 1] please".into(),
            pictures: vec![
                attached("[Image 1]"),
                attached("[Image 2]"),
                attached("[Image 9]"),
            ],
        };
        let blocks = draft.blocks(&[], Path::new("/w"), false);
        let kinds: Vec<String> = blocks
            .iter()
            .map(|block| match block {
                Block::Text(text) => format!("text {text:?}"),
                Block::Image { mime, .. } => format!("image {mime}"),
                Block::File { .. } => "file".into(),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "text \"compare \"",
                "image image/png",
                "text \" with \"",
                "image image/png",
                "text \" please\""
            ]
        );
        // Shown again, the pictures are numbered in their order, as the model sees them.
        let (text, labels) = shown(&blocks);
        assert_eq!(text, "compare [Image 1] with [Image 2] please");
        assert_eq!(
            labels,
            [("[Image 1]".to_owned(), 1), ("[Image 2]".to_owned(), 3)]
        );
    }

    #[test]
    fn a_label_the_text_mentions_keeps_its_number_and_new_ones_follow() {
        let blocks = vec![
            Block::Text("unlike [Image 1] from before, ".into()),
            Block::Image {
                mime: "image/png".into(),
                data: vec![1].into(),
            },
        ];
        let (text, labels) = shown(&blocks);
        assert_eq!(text, "unlike [Image 1] from before, [Image 2]");
        assert_eq!(labels, [("[Image 2]".to_owned(), 1)]);
    }

    #[test]
    fn linked_files_are_embedded_only_for_a_backend_that_takes_them() {
        let folder = tempfile::tempdir().expect("a directory");
        std::fs::write(folder.path().join("a.txt"), "one\ntwo\nthree\nfour\n").expect("a file");
        std::fs::create_dir(folder.path().join("dir")).expect("a folder");
        let text = "see $a.txt:2-3 and $dir/";
        let links = kou_thread::files::file_links(text, folder.path());
        let draft = Draft {
            text: text.into(),
            pictures: Vec::new(),
        };
        assert_eq!(
            draft.blocks(&links, folder.path(), false),
            [Block::Text(text.into())]
        );
        let blocks = draft.blocks(&links, folder.path(), true);
        assert_eq!(
            blocks,
            [
                Block::Text(text.into()),
                Block::File {
                    path: folder.path().join("a.txt"),
                    text: Some("two\nthree".into()),
                }
            ]
        );
    }
}
