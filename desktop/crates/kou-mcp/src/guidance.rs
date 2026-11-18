//! The know-how, in two flavours rendered from one source.
//!
//! An agent is given one of two means to work with the other panes: the MCP
//! server's tools, or the app's command line run through its own shell. What
//! it has to know is the same either way, so both flavours come from the same
//! two templates, `guidance/instructions.md` and `guidance/guide.md`:
//!
//! - Plain lines are shared. A `{{term}}` in them is replaced by the flavour's
//!   word for it, from [`TERMS`].
//! - A passage that has to differ comes as a pair: `::: tools`, its text for
//!   the tools, `::: commands`, its text for the command line, `:::`.
//! - A passage that belongs to one flavour only is marked as such and named:
//!   `::: commands only <label>` ... `:::`. The tests pin the list of these,
//!   so a rule or a caution cannot end up in one flavour by accident.

use crate::bundle::{PLUGIN_NAME, SKILL_NAME};
use std::path::Path;
use std::sync::OnceLock;

/// The means an agent in a pane is given to work with the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flavour {
    /// The MCP server's tools (`kou-desktop mcp`).
    Tools,
    /// The app's command line (`"$KOU_BIN" pane spawn ...`), run through the
    /// agent's own shell tool. No MCP server is attached.
    Commands,
}

impl Flavour {
    /// Both flavours.
    pub const ALL: [Flavour; 2] = [Flavour::Tools, Flavour::Commands];

    /// The short instructions: what this environment is, the basic loop and
    /// the rules. The MCP server returns the tools' text from `initialize`;
    /// the command line's is put into the context at launch. They name the
    /// full guide by its skill only: [`crate::Guidance::brief`] is the same
    /// text naming the guide's file as well.
    pub fn instructions(self) -> &'static str {
        static RENDERED: OnceLock<[String; 2]> = OnceLock::new();
        &RENDERED.get_or_init(|| {
            Flavour::ALL.map(|flavour| render(INSTRUCTIONS, flavour, Pointer::Skill).text)
        })[self.index()]
    }

    /// Where the agent is, in a few lines: what this app is, that a tab is
    /// also a canvas, and how to look. For a harness that is given the guide
    /// as a skill and nothing else at launch: a skill is read when its
    /// description fits the task, so without this such an agent is never
    /// told that it runs in the app at all.
    pub fn place(self) -> &'static str {
        static RENDERED: OnceLock<[String; 2]> = OnceLock::new();
        &RENDERED
            .get_or_init(|| Flavour::ALL.map(|flavour| render(PLACE, flavour, Pointer::Skill).text))
            [self.index()]
    }

    /// The long how-to: patterns, worked examples, blocked and stuck workers,
    /// choosing a harness. It assumes the rules of [`Flavour::instructions`].
    pub fn guide(self) -> &'static str {
        static RENDERED: OnceLock<[String; 2]> = OnceLock::new();
        &RENDERED
            .get_or_init(|| Flavour::ALL.map(|flavour| render(GUIDE, flavour, Pointer::Skill).text))
            [self.index()]
    }

    /// What makes a model with skills load the guide: Claude Code lists every
    /// skill by its description and loads the body only when that matches.
    pub fn skill_description(self) -> &'static str {
        match self {
            Flavour::Tools => {
                "How to work inside kou desktop, the app you run in, and orchestrate other coding agents there: \
                 find out where you are (where_am_i: your pane and tab, the panes beside you, the canvas and its \
                 edges), decide whether to delegate, write a worker's brief, start agents in panes, wait for them, \
                 read and verify their results, handle a worker that is blocked, stuck or asking a question, and \
                 pick a harness. Use it when the user speaks of the canvas, a node, an edge, a pane, a tab or \
                 another agent, before calling start_agent, when a task could be split across parallel workers or \
                 would gain from a second agent's review, and when a worker you started is blocked or not making \
                 progress."
            }
            Flavour::Commands => {
                "How to work inside kou desktop, the app you run in, and orchestrate other coding agents there with \
                 its command line (\"$KOU_BIN\" where, pane spawn, pane wait, pane read, pane send): find out \
                 where you are (your pane and tab, the panes beside you, the canvas and its edges), decide whether \
                 to delegate, write a worker's brief and pass it without the shell mangling it, start agents in \
                 panes, wait for them within your shell tool's time limit, read and verify their results, handle a \
                 worker that is blocked, stuck or asking a question, and pick a harness. Use it when the user \
                 speaks of the canvas, a node, an edge, a pane, a tab or another agent, before running \
                 \"$KOU_BIN\" pane spawn, when a task could be split across parallel workers or would gain from a \
                 second agent's review, and when a worker you started is blocked or not making progress."
            }
        }
    }

    /// The flavour's word in a template's block markers.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Flavour::Tools => "tools",
            Flavour::Commands => "commands",
        }
    }

    fn index(self) -> usize {
        match self {
            Flavour::Tools => 0,
            Flavour::Commands => 1,
        }
    }
}

pub(crate) const INSTRUCTIONS: &str = include_str!("guidance/instructions.md");
pub(crate) const PLACE: &str = include_str!("guidance/place.md");
pub(crate) const GUIDE: &str = include_str!("guidance/guide.md");

/// The words that differ between the flavours in otherwise shared lines: the
/// term, then the tools' wording, then the command line's.
pub(crate) const TERMS: &[(&str, &str, &str)] = &[
    ("where", "where_am_i", "`\"$KOU_BIN\" where`"),
    ("start", "start_agent", "`pane spawn`"),
    ("wait", "wait_for_panes", "`pane wait`"),
    ("read", "read_pane", "`pane read`"),
    ("send", "send_to_pane", "`pane send`"),
    ("focus", "focus_pane", "`pane focus`"),
    ("name_tab", "name_tab", "`tab rename`"),
    (
        "who_started_you",
        "describe_pane without arguments says so",
        "the `parent` in `\"$KOU_BIN\" pane info --json` is not null",
    ),
    ("cwd", "cwd", "`--cwd`"),
    ("any", "any=true", "`--any`"),
    ("split", "placement=\"split\"", "`--split`"),
    (
        "split_down",
        "placement=\"split\" with below=true",
        "`--split-down`",
    ),
    ("share", "share=30", "`--share 30`"),
    ("tab", "placement=\"tab\"", "`--tab`"),
    (
        "scrollback",
        "scrollback=true",
        "`--scrollback --lines 300`",
    ),
    ("model", "`model`", "`--model`"),
    ("native", "native=true", "`--native`"),
    ("terminal", "terminal=true", "`--terminal`"),
    ("models", "list_models", "`harness models`"),
    ("ask", "ask_pane", "`pane ask`"),
    ("edge_list", "list_edges", "`edge list`"),
    ("edge_add", "add_edge", "`edge add`"),
    ("edge_set", "set_edge", "`edge set`"),
    ("edge_rm", "remove_edge", "`edge rm`"),
    (
        "shell_example",
        "`start_agent(harness=\"shell\", title=\"Tests\", prompt=\"cargo test 2>&1 | tail -n 40\")`",
        "`\"$KOU_BIN\" pane spawn shell --title Tests --prompt \"cargo test 2>&1 | tail -n 40\"`",
    ),
    (
        "harnesses_first",
        "Call list_harnesses: only the ready ones can start.",
        "Run `\"$KOU_BIN\" harness list`: only the `ready` ones can start, and `native` marks those that can run as a \
         native chat.",
    ),
];

/// Where the short instructions say the full guide is.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Pointer<'a> {
    /// The skill, by name: all that is known when the text is a constant.
    Skill,
    /// The skill, and this file for harnesses without skills.
    File(&'a Path),
    /// Nowhere: the guide follows the text.
    Omit,
}

impl Pointer<'_> {
    fn sentence(self) -> String {
        let skill = format!("{PLUGIN_NAME}:{SKILL_NAME}");
        match self {
            Pointer::Skill => {
                format!(
                    "Before you orchestrate, read the full guide if you have it: in Claude Code it is the {skill} skill."
                )
            }
            Pointer::File(path) => format!(
                "Before you orchestrate, read the full guide: the {skill} skill if your harness lists it, otherwise \
                 the file `{}`.",
                path.display()
            ),
            Pointer::Omit => String::new(),
        }
    }
}

/// One piece of a template.
#[derive(Debug)]
pub(crate) enum Piece<'a> {
    Shared(&'a str),
    /// The two wordings of one passage.
    Pair {
        tools: Vec<&'a str>,
        commands: Vec<&'a str>,
    },
    /// A passage of one flavour only, and its label.
    Only {
        flavour: Flavour,
        /// Read by the test that pins the list of these passages (a Unix test).
        #[cfg_attr(not(all(test, unix)), allow(dead_code))]
        label: &'a str,
        lines: Vec<&'a str>,
    },
}

/// A template split into its pieces, and what is wrong with it.
pub(crate) fn parse(source: &str) -> (Vec<Piece<'_>>, Vec<String>) {
    enum State<'a> {
        Shared,
        Tools(Vec<&'a str>),
        Commands(Vec<&'a str>, Vec<&'a str>),
        Only(Flavour, &'a str, Vec<&'a str>),
    }
    let mut pieces = Vec::new();
    let mut problems = Vec::new();
    let mut state = State::Shared;
    for (number, line) in source.lines().enumerate() {
        let number = number + 1;
        let Some(marker) = line.strip_prefix(":::").map(str::trim) else {
            match &mut state {
                State::Shared => pieces.push(Piece::Shared(line)),
                State::Tools(lines) | State::Commands(_, lines) | State::Only(_, _, lines) => {
                    lines.push(line)
                }
            }
            continue;
        };
        let words: Vec<&str> = marker.split_whitespace().collect();
        state = match (state, words.as_slice()) {
            (State::Shared, ["tools"]) => State::Tools(Vec::new()),
            (State::Tools(tools), ["commands"]) => State::Commands(tools, Vec::new()),
            (State::Shared, [word, "only", label]) => {
                let flavour = Flavour::ALL.into_iter().find(|known| known.word() == *word);
                match flavour {
                    Some(flavour) => State::Only(flavour, label, Vec::new()),
                    None => {
                        problems.push(format!("line {number}: no flavour called {word}"));
                        State::Shared
                    }
                }
            }
            (State::Commands(tools, commands), []) => {
                pieces.push(Piece::Pair { tools, commands });
                State::Shared
            }
            (State::Only(flavour, label, lines), []) => {
                pieces.push(Piece::Only {
                    flavour,
                    label,
                    lines,
                });
                State::Shared
            }
            (State::Tools(_), []) => {
                problems.push(format!(
                    "line {number}: a tools passage with no commands passage beside it"
                ));
                State::Shared
            }
            (state, _) => {
                problems.push(format!("line {number}: `{line}` does not belong here"));
                state
            }
        };
    }
    if !matches!(state, State::Shared) {
        problems.push("the template ends inside a passage".into());
    }
    (pieces, problems)
}

/// A template rendered for one flavour, and what went wrong.
pub(crate) struct Rendered {
    pub text: String,
    /// A malformed template still renders; the tests require this to be empty.
    #[cfg_attr(not(all(test, unix)), allow(dead_code))]
    pub problems: Vec<String>,
}

pub(crate) fn render(source: &str, flavour: Flavour, pointer: Pointer) -> Rendered {
    let (pieces, mut problems) = parse(source);
    let mut lines: Vec<&str> = Vec::new();
    for piece in &pieces {
        match piece {
            Piece::Shared(line) => lines.push(line),
            Piece::Pair { tools, commands } => lines.extend(match flavour {
                Flavour::Tools => tools,
                Flavour::Commands => commands,
            }),
            Piece::Only {
                flavour: only,
                lines: only_lines,
                ..
            } if *only == flavour => lines.extend(only_lines),
            Piece::Only { .. } => {}
        }
    }
    let mut text = String::new();
    for line in lines {
        let mut rest = line;
        while let Some(start) = rest.find("{{") {
            text.push_str(&rest[..start]);
            let Some(end) = rest[start..].find("}}") else {
                problems.push(format!("an unclosed term in: {line}"));
                break;
            };
            let term = &rest[start + 2..start + end];
            match term {
                "guide_pointer" => text.push_str(&pointer.sentence()),
                _ => match TERMS.iter().find(|(name, _, _)| *name == term) {
                    Some((_, tools, commands)) => text.push_str(match flavour {
                        Flavour::Tools => tools,
                        Flavour::Commands => commands,
                    }),
                    None => problems.push(format!("no term called {term}")),
                },
            }
            rest = &rest[start + end + 2..];
        }
        text.push_str(rest);
        text.push('\n');
    }
    // A passage left out leaves its blank lines behind.
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    Rendered {
        text: text.trim().to_owned() + "\n",
        problems,
    }
}
