//! Renders scripted conversations to PNG files without showing a window, in
//! both themes and at two widths, so every entry kind and every state of the
//! pane can be looked at. Scenes with a strip also capture the frames of an
//! animation, one under the other, in `<scene>-<theme>-strip.png`.
//!
//! `cargo run -p kou-thread-view --features capture --example thread_capture -- <directory> [scene ...]`
//!
//! With `--zoom 0.6,1,1.5` each scene is drawn instead as it is on a canvas
//! at each of those zooms: the window is the 900 by 700 pane times the zoom
//! and the view follows the zoom. The captures go side by side, at their own
//! sizes, in `<scene>-<theme>-zoom.png`, and scaled to the first zoom's size
//! in `<scene>-<theme>-zoom-same.png`, where they should look alike.
//!
//! `--wrap-report` prints, for a few paragraphs, how many lines they take at
//! several zooms when set at the zoomed size, as GPUI's text sets them, and
//! when laid out at the unscaled size, as the zoomed view sets them.
//!
//! `--sweep` zooms one pane from 0.6 to 4 as a pinch would, a frame per 0.2%,
//! and prints the process's memory footprint before and after: what the
//! glyphs of every size the view was drawn at cost.
//!
//! `--pace` zooms a pane the way a canvas zooms one of its nodes, as the app's
//! `motion-canvas-thread-zoom` scene does: from 60% to 160% and back, 1.2% a
//! frame (`--rate 1.003` is the same speed at 240 frames a second), the node
//! centred in the window and the view not told of the zoom.
//! It does so for a short conversation and for a long one (`--entries N`, 300
//! by default) and prints how long each frame took to draw, and what GPUI did
//! in it. With `KOU_ZOOM_TRACE` set the view prints its own share of every
//! frame too (see the crate's `trace` module).
//!
//! `--field` draws instead the prompt field standing alone, as the app's empty
//! screen holds it ([`kou_thread_view::PromptField`]): empty and with a
//! prompt and a picture in it, at 320, 560 and 900 pixels, with a button of
//! the app's in its foot, in `field-<theme>-<width>.png`.
//!
//! The pane works in this repository, so `$` lists its files; pictures are
//! dropped on the field from the repository's own screenshots, never put on
//! the system clipboard.

use gpui::{
    App, AppContext as _, AsyncApp, Bounds, ExternalPaths, FileDropEvent, Focusable as _,
    IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, PlatformInput, WindowBounds, WindowOptions,
    point, px, size,
};
use kou_thread::{
    BackendEnd, Block, Capabilities, Choice, Level, Location, Offers, PermissionKind,
    PermissionOption, PermissionRequest, PlanStatus, PlanStep, Stop, ToolCall, ToolContent,
    ToolKind, ToolStatus, Update, Usage,
};
use kou_thread_view::{Beside, Header, Look, Nested, NestedState, ThreadView};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};

/// What to do to the window after the script ran.
enum Step {
    /// Press a key, as a key binding or as text.
    Key(&'static str),
    /// Type text into whatever is focused.
    Type(&'static str),
    /// Press and release a key: what activates a focused control.
    Press(&'static str),
    /// Leave the header to the pane the view would sit in.
    HideHeader,
    /// Let this many milliseconds pass: a listing, a picture being read.
    Wait(u64),
    /// Drop these files of the repository on the field.
    Drop(&'static [&'static str]),
    /// The backend says more: what arrives while the pane is watched.
    Say(fn() -> Vec<Update>),
}

/// Frames of what the last step sets moving: how many, how far apart, and
/// how much of the window's bottom each keeps.
struct Strip {
    frames: usize,
    every_ms: u64,
    keep: f32,
}

struct Scene {
    name: &'static str,
    script: fn() -> Vec<Update>,
    steps: &'static [Step],
    strip: Option<Strip>,
}

const SCENES: &[Scene] = &[
    Scene {
        name: "starting",
        script: Vec::new,
        steps: &[],
        strip: None,
    },
    Scene {
        name: "bare",
        script: bare,
        steps: &[Step::Type("И ещё вопрос")],
        strip: None,
    },
    Scene {
        name: "empty",
        script: || vec![ready()],
        steps: &[],
        strip: None,
    },
    Scene {
        name: "answer",
        script: answer,
        // Shift+Tab from the field lands on the last control above it: a link.
        steps: &[Step::Key("shift-tab")],
        strip: None,
    },
    Scene {
        name: "tools",
        script: tools,
        steps: &[Step::Key("shift-tab")],
        strip: None,
    },
    // Agents the agent started by itself, as Claude Code does: one that has
    // reported, one at work, one left to run in the background. Each is a
    // card with how it goes and its latest.
    Scene {
        name: "agents",
        script: agents_at_work,
        steps: &[],
        strip: None,
    },
    // The agent is idle, and a command it left running in the background
    // runs on: its card says so, and so does the line over the field.
    Scene {
        name: "background",
        script: background_command,
        steps: &[],
        strip: None,
    },
    // The same with the card of the one at work open: what it was asked,
    // what it did and said so far.
    Scene {
        name: "agents-open",
        script: agents_at_work,
        steps: &[
            Step::Key("shift-tab"),
            Step::Key("shift-tab"),
            Step::Key("shift-tab"),
            Step::Press("enter"),
            Step::Wait(600),
        ],
        strip: None,
    },
    Scene {
        name: "scrolled",
        script: answer,
        steps: &[Step::Key("pageup")],
        strip: None,
    },
    Scene {
        name: "permission",
        script: permission,
        steps: &[Step::Key("shift-tab")],
        strip: None,
    },
    Scene {
        name: "working",
        script: working,
        steps: &[
            Step::Type("Когда закончишь, прогони ещё clippy"),
            Step::Key("shift-enter"),
            Step::Type("и покажи предупреждения по kou-thread-view, если они есть."),
            Step::Key("shift-left"),
            Step::Key("shift-left"),
            Step::Key("shift-left"),
            Step::Key("shift-left"),
            Step::Key("shift-left"),
        ],
        strip: None,
    },
    Scene {
        name: "failed",
        script: failed,
        steps: &[],
        strip: None,
    },
    Scene {
        name: "embedded",
        script: tools,
        steps: &[Step::HideHeader, Step::Key("home")],
        strip: None,
    },
    Scene {
        name: "closed",
        script: closed,
        steps: &[],
        strip: None,
    },
    Scene {
        name: "slash",
        script: || vec![ready(), commands()],
        steps: &[Step::Type("/re"), Step::Key("down")],
        strip: None,
    },
    Scene {
        name: "picker",
        script: || vec![kou_ready(), kou_providers(), Update::Usage(usage())],
        // Past the attach button to the model's control: the providers, closed.
        steps: &[Step::Key("tab"), Step::Key("tab"), Step::Press("enter")],
        strip: Some(Strip {
            frames: 6,
            every_ms: 22,
            keep: 0.75,
        }),
    },
    Scene {
        name: "picker-open",
        script: || vec![kou_ready(), kou_providers(), Update::Usage(usage())],
        // A provider opens on its models, sliding in.
        steps: &[
            Step::Key("tab"),
            Step::Key("tab"),
            Step::Press("enter"),
            Step::Wait(200),
            Step::Key("down"),
            Step::Key("right"),
        ],
        strip: Some(Strip {
            frames: 9,
            every_ms: 40,
            keep: 0.75,
        }),
    },
    Scene {
        name: "picker-search",
        script: || vec![kou_ready(), kou_providers(), Update::Usage(usage())],
        // Typing searches every provider's models at once.
        steps: &[
            Step::Key("tab"),
            Step::Key("tab"),
            Step::Press("enter"),
            Step::Wait(200),
            Step::Type("gpt"),
        ],
        strip: None,
    },
    Scene {
        name: "picker-flat",
        script: || vec![ready(), Update::Usage(usage())],
        // An agent whose models show no provider: one flat list.
        steps: &[Step::Key("tab"), Step::Key("tab"), Step::Press("enter")],
        strip: None,
    },
    Scene {
        name: "streaming",
        script: streaming,
        // The answer arrives: it rises into place, its text fades in, a caret ends it.
        steps: &[Step::Say(streamed)],
        strip: Some(Strip {
            frames: 10,
            every_ms: 45,
            keep: 1.,
        }),
    },
    Scene {
        name: "thinking",
        script: streaming,
        // Reasoning being written: its label shimmers.
        steps: &[Step::Say(thought)],
        strip: Some(Strip {
            frames: 8,
            every_ms: 220,
            keep: 1.,
        }),
    },
    Scene {
        name: "running",
        script: running,
        // A call that runs: a light goes round its card, its mark blinks.
        steps: &[Step::Wait(100)],
        strip: Some(Strip {
            frames: 8,
            every_ms: 300,
            keep: 1.,
        }),
    },
    Scene {
        name: "menu",
        script: || vec![ready(), Update::Usage(usage())],
        // Past the attach button and the model, to the mode's menu.
        steps: &[
            Step::Key("tab"),
            Step::Key("tab"),
            Step::Key("tab"),
            Step::Press("enter"),
            Step::Key("down"),
        ],
        strip: None,
    },
    Scene {
        name: "expanded",
        script: expanded,
        steps: &[],
        strip: None,
    },
    Scene {
        name: "files",
        script: answer,
        steps: &[
            Step::Type("Сравни с $thread-view/src/comp"),
            Step::Wait(900),
        ],
        strip: None,
    },
    Scene {
        name: "files-open",
        script: answer,
        steps: &[
            Step::Type("Сравни с "),
            Step::Wait(900),
            Step::Type("$composer"),
        ],
        strip: Some(Strip {
            frames: 8,
            every_ms: 28,
            keep: 0.62,
        }),
    },
    Scene {
        name: "links",
        script: answer,
        steps: &[
            Step::Type("Сравни $desktop/crates/kou-thread-view/src/composer.rs:120-160 с "),
            Step::Type("$desktop/README.md и $desktop/crates/ "),
            Step::Wait(500),
        ],
        strip: None,
    },
    Scene {
        name: "links-arrive",
        script: answer,
        steps: &[
            Step::Type("Сравни $desktop/README.md"),
            Step::Wait(300),
            Step::Type(" "),
        ],
        strip: Some(Strip {
            frames: 8,
            every_ms: 30,
            keep: 0.3,
        }),
    },
    Scene {
        name: "commands",
        script: || vec![ready(), commands(), offers()],
        steps: &[Step::Type("/")],
        strip: Some(Strip {
            frames: 8,
            every_ms: 28,
            keep: 0.62,
        }),
    },
    Scene {
        name: "command-args",
        script: || vec![ready(), commands(), offers()],
        steps: &[Step::Type("/model "), Step::Key("down")],
        strip: Some(Strip {
            frames: 6,
            every_ms: 30,
            keep: 0.5,
        }),
    },
    Scene {
        name: "pictures",
        script: || vec![ready_with_pictures()],
        steps: &[
            Step::Drop(&["cockpit-glow-up-live.png", "code-syntax-dark.png"]),
            Step::Wait(1500),
            Step::Type("почему на втором экране подсветка другая?"),
        ],
        strip: None,
    },
    Scene {
        name: "pictures-arrive",
        script: || vec![ready_with_pictures()],
        steps: &[Step::Type("вот: "), Step::Drop(&["code-syntax-light.png"])],
        strip: Some(Strip {
            frames: 8,
            every_ms: 40,
            keep: 0.32,
        }),
    },
    Scene {
        name: "preview",
        script: || vec![ready_with_pictures()],
        steps: &[
            Step::Drop(&["cockpit-glow-up-empty.png"]),
            Step::Wait(1200),
            Step::Key("left"),
        ],
        strip: Some(Strip {
            frames: 6,
            every_ms: 35,
            keep: 1.,
        }),
    },
    Scene {
        name: "limits",
        script: || vec![ready()],
        steps: &[Step::Drop(&["code-syntax-dark.png"]), Step::Wait(100)],
        strip: None,
    },
    Scene {
        name: "queue",
        script: queued,
        steps: &[
            Step::Type("Потом прогони clippy по всем крейтам"),
            Step::Key("enter"),
            Step::Type("И обнови README, если поменялся API"),
            Step::Key("enter"),
            Step::Type("Проверь ещё и тесты kou-acp"),
            Step::Key(FORCE),
            Step::Type("И не забудь capture"),
        ],
        strip: None,
    },
    Scene {
        name: "queue-reorder",
        script: queued,
        steps: &[
            Step::Type("первое"),
            Step::Key("enter"),
            Step::Type("второе"),
            Step::Key("enter"),
            Step::Type("третье"),
            Step::Key("enter"),
            Step::Wait(400),
            Step::Key("up"),
            Step::Wait(200),
            Step::Key("alt-up"),
        ],
        strip: Some(Strip {
            frames: 8,
            every_ms: 30,
            keep: 0.32,
        }),
    },
    Scene {
        name: "send-turns",
        script: || vec![ready()],
        steps: &[Step::Type("x")],
        strip: Some(Strip {
            frames: 6,
            every_ms: 30,
            keep: 0.14,
        }),
    },
    Scene {
        name: "history",
        script: || vec![ready()],
        steps: &[Step::Type("retry"), Step::Wait(300), Step::Key("ctrl-r")],
        strip: None,
    },
    Scene {
        name: "help",
        script: || vec![ready(), commands(), offers()],
        steps: &[Step::Type("/help"), Step::Key("enter")],
        strip: None,
    },
    Scene {
        name: "transcript",
        script: transcript,
        steps: &[Step::Wait(800)],
        strip: None,
    },
    Scene {
        name: "cleared",
        script: tools,
        steps: &[Step::Type("/clear"), Step::Key("enter")],
        strip: None,
    },
    // A prompt edited where it stands: its editor, what it replaces faded.
    Scene {
        name: "edit",
        script: edit,
        steps: &[
            Step::Type("/edit 1"),
            Step::Key("enter"),
            Step::Type(" И что будет с метриками?"),
        ],
        strip: None,
    },
    // The editor arriving and what it replaces fading, frame by frame.
    Scene {
        name: "edit-fade",
        script: edit,
        steps: &[Step::Type("/edit 1"), Step::Key("enter")],
        strip: Some(Strip {
            frames: 7,
            every_ms: 30,
            keep: 1.,
        }),
    },
    Scene {
        name: "everything",
        script: everything,
        // Two prompts wait in the queue, and a slash opens the list of commands,
        // short enough to leave the queue in view.
        steps: &[
            Step::Type("Потом прогони clippy по всем крейтам"),
            Step::Key("enter"),
            Step::Type("И обнови README, если поменялся API"),
            Step::Key("enter"),
            Step::Wait(300),
            Step::Type("/rev"),
        ],
        strip: None,
    },
];

/// Whether the studio scenes are drawn at one pixel to the point, as on the
/// user's display, rather than at the capturing display's own scale.
static ONE: AtomicBool = AtomicBool::new(false);

/// The reference picture's window, in points, under its title bar: what the
/// `-picture` captures are drawn at, to lie over the picture at its scale.
const PICTURE: (f32, f32) = (1506., 818.);

/// A scene with the agents its agent started and what stands beside it.
struct StudioScene {
    name: &'static str,
    look: Look,
    script: fn() -> Vec<Update>,
    nested: fn(SystemTime) -> Vec<Nested>,
    waiting: &'static [&'static str],
    beside: fn() -> Option<Beside>,
    steps: &'static [Step],
}

const STUDIO_SCENES: &[StudioScene] = &[
    // The reference picture's conversation: work folded, agents' cards
    // between the answers, the line that waits on two of them.
    StudioScene {
        name: "studio-picture",
        look: Look::Studio,
        script: picture_script,
        nested: picture_nested,
        waiting: &["sessions", "lite-measure"],
        beside: picture_beside,
        steps: &[],
    },
    // The same in the cockpit's manner.
    StudioScene {
        name: "nested-cockpit",
        look: Look::Cockpit,
        script: picture_script,
        nested: picture_nested,
        waiting: &["sessions", "lite-measure"],
        beside: picture_beside,
        steps: &[],
    },
    // Every entry kind in the quieter look, its work opened.
    StudioScene {
        name: "studio-everything",
        look: Look::Studio,
        script: everything,
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[Step::Type("/expand"), Step::Key("enter"), Step::Wait(200)],
    },
    // A turn at work in the quieter look: its steps as they come.
    StudioScene {
        name: "studio-working",
        look: Look::Studio,
        script: working,
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[],
    },
    // A question for the user, in the quieter look.
    StudioScene {
        name: "studio-permission",
        look: Look::Studio,
        script: permission,
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[],
    },
    // A prompt edited where it stands, in the quieter look.
    StudioScene {
        name: "studio-edit",
        look: Look::Studio,
        script: edit,
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[
            Step::Type("/edit 1"),
            Step::Key("enter"),
            Step::Type(" И что будет с метриками?"),
        ],
    },
    // The list of commands over the field, and the queue, in the quieter look.
    StudioScene {
        name: "studio-menu",
        look: Look::Studio,
        script: everything,
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[
            Step::Type("Потом прогони clippy по всем крейтам"),
            Step::Key("enter"),
            Step::Wait(300),
            Step::Type("/rev"),
        ],
    },
    // What the field says over its text, in the quieter look: a notice (the
    // queue's command with nothing queued says how the queue is used).
    StudioScene {
        name: "studio-notice",
        look: Look::Studio,
        script: everything,
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[Step::Type("/queue"), Step::Key("enter"), Step::Wait(300)],
    },
    // A queued message taken back into the field to edit it, while the
    // agent works: the banner over the text.
    StudioScene {
        name: "studio-banner",
        look: Look::Studio,
        script: working,
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[
            Step::Type("Потом прогони clippy по всем крейтам"),
            Step::Key("enter"),
            Step::Wait(300),
            Step::Key("up"),
            Step::Key("enter"),
            Step::Wait(300),
        ],
    },
    // Two messages waiting while the agent works, in the quieter look: the
    // queue between the line that says it works and the field.
    StudioScene {
        name: "studio-queue",
        look: Look::Studio,
        script: working,
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[
            Step::Type("Потом прогони clippy по всем крейтам"),
            Step::Key("enter"),
            Step::Type("И обнови README"),
            Step::Key("enter"),
            Step::Wait(400),
        ],
    },
    // A command an agent ran, opened, in the quieter look: the cockpit's
    // card, with what the command printed (handed over as one fenced block)
    // as its output.
    StudioScene {
        name: "studio-tool",
        look: Look::Studio,
        script: ran_a_command,
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[Step::Type("/expand"), Step::Key("enter"), Step::Wait(400)],
    },
    // A picture a tool gave back (an image file the agent read), in its
    // opened card.
    StudioScene {
        name: "studio-tool-picture",
        look: Look::Studio,
        script: read_a_picture,
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[Step::Type("/expand"), Step::Key("enter"), Step::Wait(600)],
    },
    // The agent handed a task to a subagent of its own, which is at work:
    // its card says who was asked what and what it is doing now.
    StudioScene {
        name: "studio-subagent",
        look: Look::Studio,
        script: || delegated(false),
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[],
    },
    // The subagent has reported and the turn is over: its card, opened,
    // holds what it was asked, what it did and what it reported.
    StudioScene {
        name: "studio-subagent-done",
        look: Look::Studio,
        script: || delegated(true),
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[Step::Type("/expand"), Step::Key("enter"), Step::Wait(500)],
    },
    // A new session in the quieter look.
    StudioScene {
        name: "studio-empty",
        look: Look::Studio,
        script: || vec![ready()],
        nested: |_| Vec::new(),
        waiting: &[],
        beside: || None,
        steps: &[],
    },
];

/// The pinned wall clock of the studio scenes.
fn picture_now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_793_000_000)
}

/// The earlier turns of the picture's conversation: short ones, scrolled
/// out of sight, which give the ticks down the left edge.
const EARLIER: usize = 9;

fn picture_script() -> Vec<Update> {
    let spawn = |id: &str, title: &str| {
        let mut call = tool(id, title, ToolKind::Execute, ToolStatus::Done);
        call.content = vec![ToolContent::Text("started".into())];
        Update::Tool(call)
    };
    let fable = Update::Ready {
        session: "0198f2c4".into(),
        capabilities: Capabilities::default(),
        modes: vec![
            choice("low", "Low", ""),
            choice("medium", "Medium", ""),
            choice("high", "High", ""),
        ],
        mode: Some("high".into()),
        models: vec![choice("fable-5.2", "Fable 5.2", "")],
        model: Some("fable-5.2".into()),
    };
    let mut updates = vec![
        fable,
        Update::Providers(vec![kou_thread::Provider {
            id: "anthropic".into(),
            name: "Anthropic".into(),
            models: vec!["fable-5.2".into()],
        }]),
    ];
    for turn in 0..EARLIER {
        updates.extend([
            user(&format!("Шаг {}: что дальше по плану?", turn + 1)),
            Update::TurnStarted,
            Update::AssistantChunk(format!("Шаг {} сделан, перехожу к следующему.", turn + 1)),
            Update::TurnEnded(Stop::Finished),
        ]);
    }
    updates.extend([
        user("Оцени проект и выбери дальнейший путь: что мерить, что чинить первым."),
        Update::TurnStarted,
        Update::ThoughtChunk(
            "**Planning the measurements.** Three workers, then the forks.".into(),
        ),
        spawn("s1", "Run kou pane spawn claude --prompt sessions"),
        Update::AssistantChunk(
            "Запустил воркер `sessions`: он разберёт HTTP-сессии в jw-solver.".into(),
        ),
        Update::TurnEnded(Stop::Finished),
        Update::TurnStarted,
        Update::ThoughtChunk("**Queueing the next item.**".into()),
        spawn("s2", "Run kou pane spawn claude --prompt lite-measure"),
        spawn("s3", "Run kou pane wait lite-measure --timeout 600"),
        Update::AssistantChunk(
            "Сообщение от воркера `nested-marker` — промежуточное, не отчёт: у него ещё идёт \
             первая сборка и тесты, результат будет позже.\n\n\
             Пока он собирается, я запустил следующий пункт очереди:\n\
             lite-measure: jw cost breakdown and Lightpanda. Он измерит на записанных страницах, \
             офлайн:\n\n\
             - **куда у jw уходят CPU и память** — скрипты страницы, стили, раскладка и шрифты, \
             декодирование, сеть;\n\
             - **jw, Firefox 160 и Lightpanda на одних и тех же страницах**, включая то, сколько \
             работы страницы каждый реально выполнил;\n\
             - **верхнюю границу выигрыша от лёгкого режима** — сколько исчезло бы, если бы стили, \
             раскладка и шрифты ничего не делали.\n\n\
             Сейчас работают три воркера: сессии, селекторы и этот замер. Форки ещё не начаты. \
             Вопрос про повторные загрузки сайтов, где jw проиграл, по-прежнему ждёт твоего ответа."
                .into(),
        ),
        Update::TurnEnded(Stop::Finished),
        Update::TurnStarted,
        Update::AssistantChunk(
            "Ещё одно промежуточное сообщение от воркера `nested-marker`: идёт сборка пробы и \
             повторный прогон FingerprintJS, отчёта пока нет. Жду."
                .into(),
        ),
        Update::TurnEnded(Stop::Finished),
        Update::TurnStarted,
        Update::AssistantChunk(
            "Третье промежуточное сообщение от `nested-marker`: на его текущей голове \
             `f8ba4f7c8` идут проверки и сравнение на записанных страницах. Отчёта ещё нет, жду."
                .into(),
        ),
        Update::TurnEnded(Stop::Finished),
        Update::Usage(Usage {
            context_used: Some(361_000),
            context_limit: Some(1_000_000),
            input_tokens: None,
            output_tokens: None,
            cost_usd: None,
        }),
    ]);
    updates
}

fn picture_nested(now: SystemTime) -> Vec<Nested> {
    // Where each was started: after this many entries of the main turn.
    let at = |entries: usize| Some(EARLIER * 2 + entries);
    let agent = |id: &str, title: &str, state, detail: &str, ago: u64, after| Nested {
        id: id.into(),
        title: title.into(),
        harness: "claude".into(),
        code: "CC".into(),
        state,
        detail: detail.into(),
        since: now - Duration::from_secs(ago),
        after,
    };
    let marker = "nested-marker: ::before::marker as Firefox 160";
    vec![
        agent(
            "sessions",
            "sessions: HTTP sessions in jw-solver",
            NestedState::Running,
            "",
            1290,
            at(3),
        ),
        agent(
            "lite-measure",
            "lite-measure: jw cost breakdown and Lightpanda",
            NestedState::Running,
            "",
            800,
            at(7),
        ),
        agent(
            "marker-1",
            marker,
            NestedState::Finished,
            "The release probe build and the FingerprintJS reruns are running in the background. \
             I'll compare the results when they land.",
            1500,
            at(8),
        ),
        agent(
            "marker-2",
            marker,
            NestedState::Finished,
            "The gates and the replay bench are running in the background on head f8ba4f7c8. \
             I'll write the report when they finish.",
            1180,
            at(9),
        ),
        agent(
            "marker-3",
            marker,
            NestedState::Finished,
            "The gates and the replay bench are running in the background on head f8ba4f7c8. \
             I'll write the report when they finish.",
            1140,
            at(9),
        ),
    ]
}

fn picture_beside() -> Option<Beside> {
    Some(Beside {
        project: "neko-mini".into(),
        path: home().join("Projects/neko-mini"),
        branch: Some("master".into()),
        changes: Some((0, 0)),
        parent: Some(Nested {
            id: "parent".into(),
            title: "Проектный путь и навигация".into(),
            harness: "claude".into(),
            code: "CC".into(),
            state: NestedState::Finished,
            detail: String::new(),
            since: picture_now() - Duration::from_secs(4000),
            after: None,
        }),
        earlier: 3,
    })
}

/// A studio scene in a window `width` by `height`, or at `zoom` in the
/// 900 by 700 pane times the zoom.
async fn capture_studio(
    scene: &StudioScene,
    dark: bool,
    width: f32,
    height: f32,
    zoom: Option<f32>,
    cx: &mut AsyncApp,
) -> anyhow::Result<image::RgbaImage> {
    let (backend, end) = kou_thread::channel();
    let times = zoom.unwrap_or(1.);
    let window = cx.update(|cx| {
        cx.set_global(kou_ui::Theme::new(dark));
        let bounds = Bounds::centered(None, size(px(width * times), px(height * times)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                show: false,
                focus: false,
                ..Default::default()
            },
            |window, cx| {
                if ONE.load(Ordering::Relaxed) {
                    window.set_scale_factor(1.);
                }
                let header = Header {
                    agent: "Claude Code".into(),
                    code: "CC".into(),
                    cwd: home().join("Projects/neko-mini"),
                };
                let view = cx.new(|cx| {
                    let mut view = ThreadView::new(backend, header, window, cx);
                    view.set_history_file(Some(history_file()), cx);
                    view.set_app_commands(true, cx);
                    view.set_header_visible(false, cx);
                    view.set_look(scene.look, cx);
                    view.pin_clock(Some(picture_now()), cx);
                    if let Some(zoom) = zoom {
                        view.follow_zoom(Some(Rc::new(Cell::new(zoom))), cx);
                    }
                    view
                });
                view.focus_handle(cx).focus(window, cx);
                view
            },
        )
    })?;
    feed(&end, (scene.script)());
    settle(cx).await;
    window.update(cx, |view, _, cx| {
        view.set_nested((scene.nested)(picture_now()), cx);
        view.set_waiting_on(
            scene.waiting.iter().map(|id| (*id).to_owned()).collect(),
            cx,
        );
        view.set_beside((scene.beside)(), cx);
    })?;
    // The last turn still runs, waiting on the agents it started.
    if !scene.waiting.is_empty() {
        feed(&end, vec![Update::TurnStarted]);
    }
    settle(cx).await;
    for step in scene.steps {
        if let Step::Wait(ms) = step {
            run_frames(window.into(), *ms, cx).await?;
            continue;
        }
        cx.update_window(window.into(), |_, window, cx| match step {
            Step::Key(key) => {
                window.dispatch_keystroke(Keystroke::parse(key).expect("a key"), cx);
            }
            Step::Type(text) => {
                for ch in text.chars() {
                    let key = match ch {
                        ' ' => "space".to_owned(),
                        ch => ch.to_string(),
                    };
                    let os = cx.platform_info().os;
                    let keystroke = Keystroke::parse_with_os(&key, os).expect("a character");
                    window.dispatch_keystroke(keystroke, cx);
                }
            }
            _ => {}
        })?;
        run_frames(window.into(), 60, cx).await?;
    }
    // The pointer rests in the bottom left corner, over nothing: a hidden
    // window otherwise takes it where the screen's pointer happens to be.
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_event(
            PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: point(px(1.), px(height * times - 1.)),
                pressed_button: None,
                modifiers: gpui::Modifiers::default(),
            }),
            cx,
        );
    })?;
    run_frames(window.into(), 400, cx).await?;
    for _ in 0..8 {
        cx.update_window(window.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })?;
        settle(cx).await;
    }
    let image = cx.update_window(window.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
        window.render_to_image()
    })??;
    cx.update_window(window.into(), |_, window, _| window.remove_window())?;
    drop(end);
    // At one pixel to the point the frame fills the corner of a drawable
    // that is still the display's size.
    if ONE.load(Ordering::Relaxed) {
        let (w, h) = ((width * times) as u32, (height * times) as u32);
        let (w, h) = (w.min(image.width()), h.min(image.height()));
        return Ok(image::imageops::crop_imm(&image, 0, 0, w, h).to_image());
    }
    Ok(image)
}

/// The key that forces a message into the running turn.
const FORCE: &str = if cfg!(target_os = "macos") {
    "cmd-enter"
} else {
    "ctrl-enter"
};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(directory) = args.next().map(PathBuf::from) else {
        eprintln!(
            "usage: thread_capture <directory> [--zoom z,z,...] [--wrap-report] [--one] [scene ...]"
        );
        std::process::exit(2);
    };
    let mut only: Vec<String> = Vec::new();
    let mut zooms: Vec<f32> = Vec::new();
    let mut wrap_report = false;
    let mut field = false;
    let mut sweep = false;
    let mut pace = false;
    let mut entries = 300;
    let mut rate = 1.012f32;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--zoom" => {
                zooms = args
                    .next()
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|zoom| zoom.trim().parse().ok())
                    .collect();
            }
            "--wrap-report" => wrap_report = true,
            "--one" => ONE.store(true, Ordering::Relaxed),
            "--field" => field = true,
            "--sweep" => sweep = true,
            "--pace" => pace = true,
            "--rate" => {
                rate = args
                    .next()
                    .and_then(|rate| rate.parse().ok())
                    .filter(|rate: &f32| *rate > 1.)
                    .unwrap_or(rate);
            }
            "--entries" => {
                entries = args
                    .next()
                    .and_then(|count| count.parse().ok())
                    .unwrap_or(entries);
            }
            _ => only.push(arg),
        }
    }
    std::fs::create_dir_all(&directory).expect("create the output directory");

    gpui_platform::application()
        .with_assets(kou_ui::Assets)
        .run(move |cx: &mut App| {
            kou_ui::init(cx);
            cx.spawn(async move |cx| {
                if wrap_report {
                    if let Err(error) = report_wrapping(cx) {
                        eprintln!("wrap report: {error:#}");
                        std::process::exit(1);
                    }
                    cx.update(|cx| cx.quit());
                    return;
                }
                if pace {
                    if let Err(error) = pace_zoom(entries, rate, cx).await {
                        eprintln!("pace: {error:#}");
                        std::process::exit(1);
                    }
                    cx.update(|cx| cx.quit());
                    return;
                }
                if field {
                    for dark in [true, false] {
                        let theme = if dark { "dark" } else { "light" };
                        let path = directory.join(format!("field-{theme}.png"));
                        match capture_field(dark, cx).await {
                            Ok(image) => {
                                if let Err(error) = image.save(&path) {
                                    eprintln!("{}: {error:#}", path.display());
                                    std::process::exit(1);
                                }
                                eprintln!("saved {}", path.display());
                            }
                            Err(error) => {
                                eprintln!("{}: {error:#}", path.display());
                                std::process::exit(1);
                            }
                        }
                    }
                    cx.update(|cx| cx.quit());
                    return;
                }
                if sweep {
                    if let Err(error) = sweep_zoom(cx).await {
                        eprintln!("sweep: {error:#}");
                        std::process::exit(1);
                    }
                    cx.update(|cx| cx.quit());
                    return;
                }
                for scene in STUDIO_SCENES {
                    if !only
                        .iter()
                        .any(|name| name == scene.name || name == "studio")
                    {
                        continue;
                    }
                    for dark in [true, false] {
                        let theme = if dark { "dark" } else { "light" };
                        let mut sizes = vec![
                            (900., 700., "wide"),
                            (420., 700., "narrow"),
                            (PICTURE.0, PICTURE.1, "picture"),
                        ];
                        if !zooms.is_empty() {
                            sizes.clear();
                        }
                        for (width, height, label) in sizes {
                            let path =
                                directory.join(format!("{}-{theme}-{label}.png", scene.name));
                            let shot = capture_studio(scene, dark, width, height, None, cx).await;
                            if let Err(error) = shot.and_then(|image| Ok(image.save(&path)?)) {
                                eprintln!("{}: {error:#}", path.display());
                                std::process::exit(1);
                            }
                            eprintln!("saved {}", path.display());
                        }
                        for zoom in &zooms {
                            let path =
                                directory.join(format!("{}-{theme}-zoom-{zoom}.png", scene.name));
                            let shot =
                                capture_studio(scene, dark, 900., 700., Some(*zoom), cx).await;
                            if let Err(error) = shot.and_then(|image| Ok(image.save(&path)?)) {
                                eprintln!("{}: {error:#}", path.display());
                                std::process::exit(1);
                            }
                            eprintln!("saved {}", path.display());
                        }
                    }
                }
                for scene in SCENES {
                    if !only.is_empty() && !only.iter().any(|name| name == scene.name) {
                        continue;
                    }
                    for dark in [true, false] {
                        let theme = if dark { "dark" } else { "light" };
                        if !zooms.is_empty() {
                            let path = directory.join(format!("{}-{theme}-zoom.png", scene.name));
                            if let Err(error) = capture_zooms(scene, dark, &zooms, &path, cx).await
                            {
                                eprintln!("{}: {error:#}", path.display());
                                std::process::exit(1);
                            }
                            eprintln!("saved {}", path.display());
                            continue;
                        }
                        for (width, label) in [(900., "wide"), (420., "narrow")] {
                            let path =
                                directory.join(format!("{}-{theme}-{label}.png", scene.name));
                            let shot = capture(scene, dark, width, None, None, cx).await;
                            if let Err(error) = shot.and_then(|image| Ok(image.save(&path)?)) {
                                eprintln!("{}: {error:#}", path.display());
                                std::process::exit(1);
                            }
                            eprintln!("saved {}", path.display());
                        }
                        if let Some(strip) = &scene.strip {
                            let path = directory.join(format!("{}-{theme}-strip.png", scene.name));
                            let shot = capture(scene, dark, 900., None, Some(strip), cx).await;
                            if let Err(error) = shot.and_then(|image| Ok(image.save(&path)?)) {
                                eprintln!("{}: {error:#}", path.display());
                                std::process::exit(1);
                            }
                            eprintln!("saved {}", path.display());
                        }
                    }
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
}

/// The scene at each zoom, side by side at their own sizes in `path`, and
/// scaled to the first zoom's size next to it in `<path>-same.png`.
async fn capture_zooms(
    scene: &Scene,
    dark: bool,
    zooms: &[f32],
    path: &Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let mut shots = Vec::new();
    for zoom in zooms {
        shots.push(capture(scene, dark, 900., Some(*zoom), None, cx).await?);
    }
    let gap = 24;
    let grey = image::Rgba([128, 128, 128, 255]);
    let width = shots.iter().map(|shot| shot.width() + gap).sum::<u32>() - gap;
    let height = shots.iter().map(|shot| shot.height()).max().unwrap_or(0);
    let mut tiled = image::RgbaImage::from_pixel(width, height, grey);
    let mut x = 0;
    for shot in &shots {
        image::imageops::replace(&mut tiled, shot, x as i64, 0);
        x += shot.width() + gap;
    }
    tiled.save(path)?;

    let (same_w, same_h) = shots[0].dimensions();
    let mut same =
        image::RgbaImage::from_pixel((same_w + gap) * shots.len() as u32 - gap, same_h, grey);
    for (at, shot) in shots.iter().enumerate() {
        let scaled =
            image::imageops::resize(shot, same_w, same_h, image::imageops::FilterType::Triangle);
        image::imageops::replace(&mut same, &scaled, (at as u32 * (same_w + gap)) as i64, 0);
    }
    same.save(path.with_file_name(format!(
        "{}-same.png",
        path.file_stem().unwrap_or_default().to_string_lossy()
    )))?;
    Ok(())
}

/// A pane at a zoom inside a window that holds it at its largest.
struct Zoomed {
    view: gpui::Entity<ThreadView>,
    zoom: Rc<Cell<f32>>,
    size: gpui::Size<f32>,
}

impl gpui::Render for Zoomed {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        let zoom = self.zoom.get();
        gpui::div().size_full().child(
            gpui::div()
                .w(px(self.size.width * zoom))
                .h(px(self.size.height * zoom))
                .child(self.view.clone()),
        )
    }
}

/// The process's memory footprint and the part of it that is the GPU's, in
/// bytes, as `footprint` reports them.
fn footprint() -> Option<(f64, f64)> {
    let output = std::process::Command::new("/usr/bin/footprint")
        .arg(std::process::id().to_string())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let bytes = |amount: &str, unit: &str| -> f64 {
        let amount: f64 = amount.parse().unwrap_or(0.);
        match unit {
            "B" => amount,
            "KB" => amount * 1024.,
            "MB" => amount * 1024. * 1024.,
            "GB" => amount * 1024. * 1024. * 1024.,
            _ => 0.,
        }
    };
    let mut total = None;
    let mut gpu = 0.;
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if let Some(at) = words.iter().position(|word| *word == "Footprint:") {
            total = Some(bytes(words.get(at + 1)?, words.get(at + 2)?));
        } else if line.contains("IOAccelerator") || line.contains("IOSurface") {
            // Dirty, then clean: the first amount is what the category holds.
            gpu += bytes(words.first()?, words.get(1)?);
        }
    }
    Some((total?, gpu))
}

/// Zooms the `everything` scene from 0.6 to 4, a frame per 0.2% as a pinch at
/// 240 Hz goes, and prints the footprint before and after and the factors drawn.
async fn sweep_zoom(cx: &mut AsyncApp) -> anyhow::Result<()> {
    let pane = gpui::size(420f32, 320.);
    let (backend, end) = kou_thread::channel();
    let zoom = Rc::new(Cell::new(0.6f32));
    let window = cx.update(|cx| {
        cx.set_global(kou_ui::Theme::new(true));
        let bounds = Bounds::centered(None, size(px(pane.width * 4.), px(pane.height * 4.)), cx);
        let handle = zoom.clone();
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                show: false,
                focus: false,
                ..Default::default()
            },
            move |window, cx| {
                let header = Header {
                    agent: "kou".into(),
                    code: "K".into(),
                    cwd: home().join("Projects/Go/kou-conveyor"),
                };
                let view = cx.new(|cx| {
                    let mut view = ThreadView::new(backend, header, window, cx);
                    view.set_history_file(None, cx);
                    view.follow_zoom(Some(handle.clone()), cx);
                    view
                });
                cx.new(|_| Zoomed {
                    view,
                    zoom: handle,
                    size: pane,
                })
            },
        )
    })?;
    feed(&end, everything());
    settle(cx).await;
    let draw = |cx: &mut AsyncApp| {
        cx.update_window(window.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
    };
    for _ in 0..4 {
        draw(cx)?;
        settle(cx).await;
    }
    let before = footprint();
    let (mut frames, mut factors) = (0, Vec::new());
    let mut value = 0.6f32;
    while value <= 4. {
        zoom.set(value);
        window.update(cx, |zoomed, _, cx| {
            cx.notify();
            zoomed.view.update(cx, |_, cx| cx.notify());
        })?;
        draw(cx)?;
        let factor = kou_ui::scale::quantize(value);
        if factors.last() != Some(&factor) {
            factors.push(factor);
        }
        frames += 1;
        value *= 1.002;
    }
    settle(cx).await;
    let after = footprint();
    println!(
        "{frames} frames from 0.6 to 4, drawn at {} factors",
        factors.len()
    );
    fn mb(bytes: f64) -> f64 {
        bytes / (1024. * 1024.)
    }
    match (before, after) {
        (Some((total, gpu)), Some((total_after, gpu_after))) => println!(
            "footprint {:.1} MB -> {:.1} MB (+{:.1}); of it the GPU's {:.1} MB -> {:.1} MB (+{:.1})",
            mb(total),
            mb(total_after),
            mb(total_after - total),
            mb(gpu),
            mb(gpu_after),
            mb(gpu_after - gpu),
        ),
        _ => println!("footprint could not be read"),
    }
    window.update(cx, |_, window, _| window.remove_window())?;
    drop(end);

    // The same sweep over text alone, drawn at every frame's zoom and at its
    // step, each in a window of its own: what the steps save.
    for exact in [true, false] {
        let zoom = Rc::new(Cell::new(0.6f32));
        let handle = zoom.clone();
        let window = cx.update(|cx| {
            let bounds = Bounds::centered(None, size(px(1680.), px(1280.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    show: false,
                    focus: false,
                    ..Default::default()
                },
                move |_, cx| {
                    cx.new(|_| Prose {
                        zoom: handle,
                        exact,
                    })
                },
            )
        })?;
        let draw = |cx: &mut AsyncApp| {
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
        };
        draw(cx)?;
        settle(cx).await;
        let before = footprint();
        let mut value = 0.6f32;
        let mut sizes = Vec::new();
        while value <= 4. {
            zoom.set(value);
            let drawn = if exact {
                value
            } else {
                kou_ui::scale::quantize(value)
            };
            if sizes.last() != Some(&drawn) {
                sizes.push(drawn);
            }
            draw(cx)?;
            value *= 1.002;
        }
        settle(cx).await;
        let after = footprint();
        if let (Some((_, gpu)), Some((_, gpu_after))) = (before, after) {
            println!(
                "text alone, drawn at {} sizes ({}): the GPU's footprint +{:.1} MB",
                sizes.len(),
                if exact {
                    "every frame's zoom"
                } else {
                    "the steps"
                },
                mb(gpu_after - gpu)
            );
        }
        window.update(cx, |_, window, _| window.remove_window())?;
    }
    Ok(())
}

/// A node of a canvas, as the canvas draws one: the pane at the zoom, centred
/// in the window, with the view in it built as a child the host does not
/// notify. The host is notified on every frame, as the canvas is.
struct Board {
    view: gpui::Entity<ThreadView>,
    zoom: Rc<Cell<f32>>,
    pane: gpui::Size<f32>,
    window: gpui::Size<f32>,
}

impl gpui::Render for Board {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        let zoom = self.zoom.get();
        let (width, height) = (self.pane.width * zoom, self.pane.height * zoom);
        gpui::div().size_full().relative().child(
            gpui::div()
                .absolute()
                .left(px((self.window.width - width) / 2.))
                .top(px((self.window.height - height) / 2.))
                .w(px(width))
                .h(px(height))
                .overflow_hidden()
                .child(self.view.clone()),
        )
    }
}

/// Zooms a pane on a [`Board`] from 0.6 to 1.6 and back, by `rate` a frame,
/// with a short conversation and then with one of `entries` entries, and
/// prints what the frames took.
async fn pace_zoom(entries: usize, rate: f32, cx: &mut AsyncApp) -> anyhow::Result<()> {
    let window_size = gpui::size(1300f32, 820.);
    let pane = gpui::size(640f32, 560.);
    for (name, script) in [
        ("short", short_conversation()),
        ("long", long_conversation(entries)),
    ] {
        let (backend, end) = kou_thread::channel();
        let zoom = Rc::new(Cell::new(0.6f32));
        let handle = zoom.clone();
        let window = cx.update(|cx| {
            cx.set_global(kou_ui::Theme::new(true));
            let bounds = Bounds::centered(
                None,
                size(px(window_size.width), px(window_size.height)),
                cx,
            );
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    show: false,
                    focus: false,
                    ..Default::default()
                },
                move |window, cx| {
                    let header = Header {
                        agent: "kou".into(),
                        code: "K".into(),
                        cwd: home().join("Projects/Go/kou-conveyor"),
                    };
                    let view = cx.new(|cx| {
                        let mut view = ThreadView::new(backend, header, window, cx);
                        view.set_history_file(None, cx);
                        view.follow_zoom(Some(handle.clone()), cx);
                        view
                    });
                    cx.new(|_| Board {
                        view,
                        zoom: handle,
                        pane,
                        window: window_size,
                    })
                },
            )
        })?;
        feed(&end, script);
        for _ in 0..6 {
            settle(cx).await;
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })?;
        }
        let mut values = Vec::new();
        let mut value = 0.6f32;
        while value < 1.6 {
            value *= rate;
            values.push(value);
        }
        let back: Vec<f32> = values.iter().rev().skip(1).copied().collect();
        values.extend(back);
        let mut times = Vec::new();
        let mut stats = Vec::new();
        let mut glyphs = Vec::new();
        kou_ui::scale::count_text_work(true);
        kou_ui::scale::take_text_places();
        for value in &values {
            zoom.set(*value);
            let frame = cx.update_window(window.into(), |root, window, cx| {
                // As the canvas: notified for its new camera, the views in
                // its nodes left to find their new bounds.
                cx.notify(root.entity_id());
                window.reset_layout_stats();
                let started = std::time::Instant::now();
                window.draw(cx).clear(cx);
                let took = started.elapsed();
                let places = kou_ui::scale::take_text_places()
                    .into_iter()
                    .map(|bounds| (f32::from(bounds.origin.x), f32::from(bounds.origin.y)))
                    .collect::<Vec<_>>();
                (took, window.layout_stats(), places)
            })?;
            times.push(frame.0);
            stats.push(frame.1);
            glyphs.push(frame.2);
        }
        let rows = window.update(cx, |board, _, cx| {
            board.view.read(cx).thread().entries().len()
        })?;
        report_pace(name, rows, &values, &times, &stats);
        report_places(&values, &glyphs, pane, window_size);
        window.update(cx, |_, window, _| window.remove_window())?;
        drop(end);
        settle(cx).await;
    }
    Ok(())
}

/// Prints the frames of one run of [`pace_zoom`]: the time each took to draw,
/// as the median, the 90th percentile and the worst, for the frames up and
/// the frames back down (whose glyph sizes were all drawn on the way up), and
/// what GPUI counted in them.
fn report_pace(
    name: &str,
    rows: usize,
    values: &[f32],
    times: &[Duration],
    stats: &[gpui::LayoutStats],
) {
    let ms = |duration: Duration| duration.as_secs_f64() * 1000.;
    let summary = |frames: &[Duration]| {
        let mut sorted: Vec<f64> = frames.iter().map(|time| ms(*time)).collect();
        sorted.sort_by(f64::total_cmp);
        let at = |share: f64| sorted[((sorted.len() - 1) as f64 * share).round() as usize];
        format!(
            "median {:.2} ms, p90 {:.2} ms, worst {:.2} ms",
            at(0.5),
            at(0.9),
            at(1.)
        )
    };
    let half = values.len().div_ceil(2);
    let budget = |frames: &[Duration]| {
        frames
            .iter()
            .filter(|time| time.as_secs_f64() * 1000. > 1000. / 240.)
            .count()
    };
    println!(
        "{name}: {rows} entries, {} frames, {} of them over the 4.17 ms of a frame at 240 Hz",
        values.len(),
        budget(times)
    );
    println!("  up   {}", summary(&times[..half]));
    println!("  down {}", summary(&times[half..]));
    let median = |pick: &dyn Fn(&gpui::LayoutStats) -> f64| {
        let mut values: Vec<f64> = stats.iter().map(pick).collect();
        values.sort_by(f64::total_cmp);
        values[values.len() / 2]
    };
    println!(
        "  median per frame: build {:.2} ms, prepaint {:.2} ms (layout {:.2}, measuring {:.2}), \
         paint {:.2} ms; {} measurements, {} lines shaped ({:.2} ms), {} views built",
        median(&|stats| ms(stats.build_time)),
        median(&|stats| ms(stats.prepaint_time)),
        median(&|stats| ms(stats.compute_layout_time)),
        median(&|stats| ms(stats.measure_time)),
        median(&|stats| ms(stats.paint_time)),
        median(&|stats| stats.measure_calls as f64),
        median(&|stats| stats.lines_shaped as f64),
        median(&|stats| ms(stats.shape_time)),
        median(&|stats| stats.views_built as f64),
    );
    for (at, ((value, time), stats)) in values.iter().zip(times).zip(stats).enumerate() {
        println!(
            "  frame {at:3} zoom {value:.4} draw {:6.2} ms build {:5.2} prepaint {:5.2} \
             paint {:5.2} shaped {:3} measured {:4}",
            ms(*time),
            ms(stats.build_time),
            ms(stats.prepaint_time),
            ms(stats.paint_time),
            stats.lines_shaped,
            stats.measure_calls,
        );
    }
}

/// How far from where the zoom puts it each text was placed, frame by frame.
/// The texts are matched from the last placed, which stay on screen at every
/// zoom while the list follows its end. A text's distance from the pane's
/// left edge and from its bottom (which the board places exactly) is its
/// distance at 1 times the zoom, plus the borders on the way, which do not
/// scale: a straight line in the zoom. Prints, in pixels, how far the texts
/// are from the lines that fit them best, at the 95th percentile and at
/// worst, across and down; the worst spread of those distances in one frame;
/// and how often a text moved more than a pixel off its line from one frame
/// to the next. Rounded once, a place is never more than half a pixel off.
fn report_places(
    values: &[f32],
    places: &[Vec<(f32, f32)>],
    pane: gpui::Size<f32>,
    window: gpui::Size<f32>,
) {
    // Frames drawn at 1 have no scaled texts, and a frame whose texts are
    // not the same as most frames' (a row coming into sight at the top) is
    // left out: the texts are told apart by their order.
    let mut counts: Vec<usize> = places
        .iter()
        .map(Vec::len)
        .filter(|count| *count > 0)
        .collect();
    counts.sort_unstable();
    let Some(count) = counts
        .iter()
        .copied()
        .max_by_key(|count| counts.iter().filter(|other| *other == count).count())
    else {
        println!("  no texts placed");
        return;
    };
    let kept: Vec<usize> = (0..places.len())
        .filter(|frame| places[*frame].len() == count)
        .collect();
    let values: Vec<f32> = kept.iter().map(|frame| values[*frame]).collect();
    // Each text's distances from the pane's left edge and bottom, frame by frame.
    let distances: Vec<Vec<(f32, f32)>> = kept
        .iter()
        .zip(&values)
        .map(|(frame, zoom)| {
            let left = (window.width - pane.width * zoom) / 2.;
            let bottom = (window.height + pane.height * zoom) / 2.;
            places[*frame]
                .iter()
                .map(|(x, y)| (x - left, bottom - y))
                .collect()
        })
        .collect();
    // How far each of a text's distances is from the straight line in the
    // zoom that fits them best.
    let zooms = &values;
    let fit = |series: &[f32]| -> Vec<f32> {
        let frames = series.len() as f32;
        let mean_z = zooms.iter().sum::<f32>() / frames;
        let mean_v = series.iter().sum::<f32>() / frames;
        let slope = zooms
            .iter()
            .zip(series)
            .map(|(z, v)| (z - mean_z) * (v - mean_v))
            .sum::<f32>()
            / zooms.iter().map(|z| (z - mean_z).powi(2)).sum::<f32>();
        zooms
            .iter()
            .zip(series)
            .map(|(z, v)| v - (mean_v + slope * (z - mean_z)))
            .collect()
    };
    let mut across = Vec::new();
    let mut down: Vec<Vec<f32>> = Vec::new();
    for text in 0..count {
        let xs: Vec<f32> = distances.iter().map(|frame| frame[text].0).collect();
        let ys: Vec<f32> = distances.iter().map(|frame| frame[text].1).collect();
        across.extend(fit(&xs).into_iter().map(f32::abs));
        down.push(fit(&ys));
    }
    let spread = (0..values.len())
        .map(|frame| {
            let (low, high) = down.iter().fold((f32::MAX, f32::MIN), |(low, high), text| {
                (low.min(text[frame]), high.max(text[frame]))
            });
            high - low
        })
        .fold(0., f32::max);
    let steps: Vec<f32> = down
        .iter()
        .flat_map(|text| text.windows(2).map(|pair| (pair[1] - pair[0]).abs()))
        .collect();
    let jumps = steps.iter().filter(|step| **step > 1.01).count();
    let sorted = |mut values: Vec<f32>| {
        values.sort_by(f32::total_cmp);
        values
    };
    let down = sorted(down.into_iter().flatten().map(f32::abs).collect());
    let across = sorted(across);
    let at =
        |values: &[f32], share: f32| values[((values.len() - 1) as f32 * share).round() as usize];
    println!(
        "  {count} texts placed, off their lines: down p95 {:.2} px, worst {:.2} px; across \
         p95 {:.2} px, worst {:.2} px; worst spread in a frame {spread:.2} px; more than a \
         pixel off from one frame to the next {jumps} times in {}",
        at(&down, 0.95),
        at(&down, 1.),
        at(&across, 0.95),
        at(&across, 1.),
        steps.len(),
    );
}

/// The app's `motion-canvas-thread-zoom` scene's conversation: a short
/// exchange with prose, a list, code and a tool call in it.
fn short_conversation() -> Vec<Update> {
    vec![
        ready(),
        user("Why does the retry queue back off twice as long as the config says?"),
        Update::TurnStarted,
        Update::Tool(tool(
            "t1",
            "Read retry/backoff.go",
            ToolKind::Read,
            ToolStatus::Done,
        )),
        Update::AssistantChunk(
            "The delay is doubled in two places:\n\n- `backoff()` shifts by the attempt \
             number\n- the caller multiplies by `factor` again\n\n```go\nfunc backoff(attempt \
             int) time.Duration {\n    return base << attempt\n}\n```\n\nDropping the second \
             multiplication gives the configured curve."
                .into(),
        ),
        Update::TurnEnded(Stop::Finished),
    ]
}

/// A long working session: `entries` entries or a few more, in turns of a
/// prompt, reasoning, three tool calls (an edit with its change, a test run
/// that sometimes fails and stays open) and an answer with a heading, a list,
/// a quote, code and a table.
fn long_conversation(entries: usize) -> Vec<Update> {
    let mut updates = vec![ready(), commands()];
    let Some(Update::AssistantChunk(answer)) = answer().into_iter().nth(6) else {
        unreachable!("the answer scene answers");
    };
    let mut turn = 0;
    while turn * 6 < entries {
        let mut read = tool(
            &format!("r{turn}"),
            "Read internal/runner/retry.go",
            ToolKind::Read,
            ToolStatus::Done,
        );
        read.content = vec![ToolContent::Text("84 lines".into())];
        let mut edit = tool(
            &format!("e{turn}"),
            "Edit retry.go",
            ToolKind::Edit,
            ToolStatus::Done,
        );
        edit.content = vec![ToolContent::Diff {
            path: repo("internal/runner/retry.go"),
            old: Some(OLD_RETRY.into()),
            new: NEW_RETRY.into(),
        }];
        let failed = turn % 4 == 3;
        let mut test = tool(
            &format!("x{turn}"),
            "Run go test ./internal/runner/...",
            ToolKind::Execute,
            if failed {
                ToolStatus::Failed
            } else {
                ToolStatus::Done
            },
        );
        test.content = vec![ToolContent::Terminal {
            id: format!("term-{turn}"),
            output: "=== RUN   TestBackoffGrows\n--- PASS: TestBackoffGrows (0.00s)\n\
                     === RUN   TestRequeueSchedulesLater\n    retry_test.go:58: delay for \
                     attempt 3 = 9.4s, want between 4s and 8s\n--- FAIL: \
                     TestRequeueSchedulesLater (0.01s)\nFAIL\n"
                .into(),
            exit: Some(i32::from(failed)),
        }];
        updates.extend([
            user(&format!(
                "Шаг {turn}: проверь очередь повторов ещё раз и объясни, что изменилось."
            )),
            Update::TurnStarted,
            Update::ThoughtChunk(
                "**Looking at the retry path.** The queue lives in `internal/runner/retry.go`; \
                 the lease TTL is the key detail."
                    .into(),
            ),
            Update::Tool(read),
            Update::Tool(edit),
            Update::Tool(test),
            Update::AssistantChunk(answer.clone()),
            Update::TurnEnded(Stop::Finished),
        ]);
        turn += 1;
    }
    updates
}

/// Two paragraphs of the interface font and one of the monospace, at 13 pixels
/// times the zoom, or times its step.
struct Prose {
    zoom: Rc<Cell<f32>>,
    exact: bool,
}

impl gpui::Render for Prose {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        let theme = kou_ui::theme(cx);
        let zoom = self.zoom.get();
        let factor = if self.exact {
            zoom
        } else {
            kou_ui::scale::quantize(zoom)
        };
        gpui::div()
            .w(px(420. * factor))
            .font_family(theme.ui_font.clone())
            .flex()
            .flex_col()
            .text_size(px(13. * factor))
            .line_height(px(20. * factor))
            .text_color(gpui::white())
            .child(
                "Задача не удаляется из очереди, пока воркер её не подтвердил. Вместо этого он \
                 берёт аренду на 30 секунд и продлевает её, пока работает.",
            )
            .child(
                "The task stays in the queue until a worker acknowledges it; if the worker dies, \
                 the lease runs out and the next worker takes it. 0123456789",
            )
            .child(gpui::div().font_family(theme.mono_font.clone()).child(
                "fn backoff(attempt: u32) -> Duration { Duration::from_secs(1 << attempt) }",
            ))
    }
}

/// Prints how many lines paragraphs take at several zooms, set at the zoomed
/// size (as GPUI's text sets them) and laid out at the unscaled size (as the
/// zoomed view sets them), in a 520 pixel column of the interface font.
fn report_wrapping(cx: &mut AsyncApp) -> anyhow::Result<()> {
    let window = cx.update(|cx| {
        let bounds = Bounds::centered(None, size(px(400.), px(300.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                show: false,
                focus: false,
                ..Default::default()
            },
            |_, cx| cx.new(|_| gpui::EmptyView),
        )
    })?;
    let paragraphs = [
        "Задача не удаляется из очереди, пока воркер её не подтвердил. Вместо этого он берёт \
         аренду на 30 секунд и продлевает её, пока работает. Если воркер упал, аренда просто \
         истекает, и задачу берёт следующий; повтор возможен, поэтому обработчики должны быть \
         идемпотентными.",
        "The task stays in the queue until a worker acknowledges it. Instead the worker takes \
         a lease of thirty seconds and renews it while it works. If the worker dies, the lease \
         simply runs out and the next worker takes the task; a retry can happen, so handlers \
         have to be idempotent.",
        "One test fails: the upper bound in `retry_test.go` was computed for the old formula, \
         so the expectation moves with `backoff` and the jitter it now adds to every delay.",
    ];
    let zooms = [0.6f32, 0.75, 1., 1.25, 1.5, 2., 2.5, 3., 4.];
    window.update(cx, |_, window, cx| {
        let theme = kou_ui::theme(cx);
        let font = gpui::font(theme.ui_font.clone());
        let mono = gpui::font(theme.mono_font.clone());
        println!("lines of each paragraph in a 520px column of 13px text, by zoom:");
        println!(
            "{:>9} {}",
            "zoom",
            zooms
                .iter()
                .map(|zoom| format!("{:>5.2}", kou_ui::scale::quantize(*zoom)))
                .collect::<String>()
        );
        for (index, text) in paragraphs.iter().enumerate() {
            let runs = |text: &str| {
                // Inline code in the monospace, as the transcript sets it.
                let mut runs = Vec::new();
                for (at, piece) in text.split('`').enumerate() {
                    if piece.is_empty() {
                        continue;
                    }
                    runs.push(gpui::TextRun {
                        len: piece.len(),
                        font: if at % 2 == 1 {
                            mono.clone()
                        } else {
                            font.clone()
                        },
                        color: gpui::black(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    });
                }
                runs
            };
            let text: String = text.replace('`', "");
            let runs = runs(paragraphs[index]);
            let lines = |font_size: f32, width: f32| -> usize {
                window
                    .text_system()
                    .shape_text(
                        text.clone().into(),
                        px(font_size),
                        &runs,
                        Some(px(width)),
                        None,
                    )
                    .map(|lines| {
                        lines
                            .iter()
                            .map(|line| line.wrap_boundaries().len() + 1)
                            .sum()
                    })
                    .unwrap_or(0)
            };
            let mut zoomed = String::new();
            let mut unscaled = String::new();
            for zoom in zooms {
                let factor = kou_ui::scale::quantize(zoom);
                zoomed.push_str(&format!("{:>5}", lines(13. * factor, 520. * factor)));
                unscaled.push_str(&format!("{:>5}", lines(13., 520.)));
            }
            println!(
                "{:>9} {zoomed}   paragraph {index}, set at the zoomed size",
                ""
            );
            println!(
                "{:>9} {unscaled}   paragraph {index}, laid out unscaled",
                ""
            );
        }
    })?;
    window.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

/// The scene drawn in a window `width` wide, or at `zoom` in a window the
/// size of the pane times the zoom, or the frames of `strip`.
async fn capture(
    scene: &Scene,
    dark: bool,
    width: f32,
    zoom: Option<f32>,
    strip: Option<&Strip>,
    cx: &mut AsyncApp,
) -> anyhow::Result<image::RgbaImage> {
    let (backend, end) = kou_thread::channel();
    let times = zoom.unwrap_or(1.);
    let window = cx.update(|cx| {
        cx.set_global(kou_ui::Theme::new(dark));
        let bounds = Bounds::centered(None, size(px(width * times), px(700. * times)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                // Never on screen: the frame is rendered straight to an image.
                show: false,
                focus: false,
                ..Default::default()
            },
            |window, cx| {
                let header = Header {
                    agent: "kou".into(),
                    code: "K".into(),
                    cwd: home().join("Projects/Go/kou-conveyor"),
                };
                let view = cx.new(|cx| {
                    let mut view = ThreadView::new(backend, header, window, cx);
                    // The repository's own history is the user's: these
                    // scenes keep one of their own.
                    view.set_history_file(Some(history_file()), cx);
                    view.set_app_commands(true, cx);
                    if let Some(zoom) = zoom {
                        view.follow_zoom(Some(Rc::new(Cell::new(zoom))), cx);
                    }
                    view
                });
                view.focus_handle(cx).focus(window, cx);
                view
            },
        )
    })?;

    feed(&end, (scene.script)());
    settle(cx).await;
    for (at, step) in scene.steps.iter().enumerate() {
        if matches!(step, Step::HideHeader) {
            window.update(cx, |view, _, cx| view.set_header_visible(false, cx))?;
        }
        if let Step::Say(more) = step {
            feed(&end, more());
        }
        // By the untyped handle: the typed one would hold the view while its own listeners run.
        if let Step::Wait(ms) = step {
            run_frames(window.into(), *ms, cx).await?;
            if strip.is_none() || at + 1 < scene.steps.len() {
                continue;
            }
        }
        cx.update_window(window.into(), |_, window, cx| match step {
            Step::HideHeader | Step::Wait(_) | Step::Say(_) => {}
            Step::Drop(files) => {
                let position = point(px(width * times / 2.), px(640. * times));
                let paths = ExternalPaths(files.iter().map(|file| repo(file)).collect());
                window.dispatch_event(
                    PlatformInput::FileDrop(FileDropEvent::Entered { position, paths }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::FileDrop(FileDropEvent::Submit { position }),
                    cx,
                );
            }
            Step::Key(key) => {
                window.dispatch_keystroke(Keystroke::parse(key).expect("a key"), cx);
            }
            Step::Type(text) => {
                for ch in text.chars() {
                    let key = match ch {
                        ' ' => "space".to_owned(),
                        ch => ch.to_string(),
                    };
                    let os = cx.platform_info().os;
                    let keystroke = Keystroke::parse_with_os(&key, os).expect("a character");
                    window.dispatch_keystroke(keystroke, cx);
                }
            }
            Step::Press(key) => {
                let keystroke = Keystroke::parse(key).expect("a key");
                window.dispatch_event(
                    PlatformInput::KeyDown(KeyDownEvent {
                        keystroke: keystroke.clone(),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
                window.dispatch_event(PlatformInput::KeyUp(KeyUpEvent { keystroke }), cx);
            }
        })?;
        if let Some(strip) = strip
            && at + 1 == scene.steps.len()
        {
            let image = film(window.into(), strip, cx).await?;
            cx.update_window(window.into(), |_, window, _| window.remove_window())?;
            drop(end);
            return Ok(image);
        }
        run_frames(window.into(), 60, cx).await?;
    }

    // Frames until what moves has settled: springs and entrances take a few
    // hundred milliseconds, and a measured height needs a frame to be known.
    for _ in 0..8 {
        cx.update_window(window.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })?;
        settle(cx).await;
    }
    let image = cx.update_window(window.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
        window.render_to_image()
    })??;
    cx.update_window(window.into(), |_, window, _| window.remove_window())?;
    // The backend's end lived until here: dropping it earlier would close the session.
    drop(end);
    Ok(image)
}

/// An empty screen with the prompt field in its middle, as the app has it.
struct Clean {
    field: gpui::Entity<kou_thread_view::PromptField>,
    width: f32,
}

impl gpui::Render for Clean {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        let theme = kou_ui::theme(cx);
        gpui::div()
            .size_full()
            .bg(theme.pane)
            .flex()
            .items_center()
            .justify_center()
            .child(gpui::div().w(px(self.width)).child(self.field.clone()))
    }
}

/// The app's own button in the field's foot: who gets the prompt.
fn recipient(_: &mut gpui::Window, cx: &mut App) -> gpui::AnyElement {
    use gpui::{ParentElement as _, Styled as _};
    let theme = kou_ui::theme(cx);
    gpui::div()
        .h(kou_ui::scale::px(28.))
        .px(kou_ui::scale::px(8.))
        .flex()
        .items_center()
        .gap(kou_ui::scale::px(6.))
        .rounded(kou_ui::scale::px(6.))
        .border_1()
        .border_color(theme.line)
        .child(kou_ui::monogram("ko", theme.accent, theme))
        .child(kou_ui::scale::text("kou"))
        .child(kou_ui::icon(kou_ui::Icon::ChevronDown, theme.fg_muted).size(kou_ui::scale::px(11.)))
        .into_any_element()
}

/// The prompt field alone, at three widths, empty and with a prompt in it,
/// tiled under one another.
async fn capture_field(dark: bool, cx: &mut AsyncApp) -> anyhow::Result<image::RgbaImage> {
    let mut shots = Vec::new();
    for width in [320f32, 560., 900.] {
        for typed in [false, true] {
            let window = cx.update(|cx| {
                cx.set_global(kou_ui::Theme::new(dark));
                let bounds = Bounds::centered(None, size(px(width + 80.), px(260.)), cx);
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: None,
                        show: false,
                        focus: false,
                        ..Default::default()
                    },
                    |window, cx| {
                        let field = cx.new(|cx| {
                            let mut field = kou_thread_view::PromptField::new(window, cx);
                            field.set_placeholder("What should be done?", cx);
                            field.set_foot(Some(recipient), cx);
                            field
                        });
                        field.update(cx, |field, cx| field.focus(window, cx));
                        cx.new(|_| Clean { field, width })
                    },
                )
            })?;
            settle(cx).await;
            if typed {
                cx.update_window(window.into(), |_, window, cx| {
                    for ch in "Сравни подсветку на двух экранах и почини светлую тему".chars()
                    {
                        let key = match ch {
                            ' ' => "space".to_owned(),
                            ch => ch.to_string(),
                        };
                        let os = cx.platform_info().os;
                        let keystroke = Keystroke::parse_with_os(&key, os).expect("a character");
                        window.dispatch_keystroke(keystroke, cx);
                    }
                })?;
                run_frames(window.into(), 200, cx).await?;
                cx.update_window(window.into(), |_, window, cx| {
                    let position = point(px((width + 80.) / 2.), px(130.));
                    let paths =
                        ExternalPaths(std::iter::once(repo("code-syntax-dark.png")).collect());
                    window.dispatch_event(
                        PlatformInput::FileDrop(FileDropEvent::Entered { position, paths }),
                        cx,
                    );
                    window.dispatch_event(
                        PlatformInput::FileDrop(FileDropEvent::Submit { position }),
                        cx,
                    );
                })?;
                run_frames(window.into(), 1200, cx).await?;
            }
            for _ in 0..6 {
                cx.update_window(window.into(), |_, window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                })?;
                settle(cx).await;
            }
            let image = cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
                window.render_to_image()
            })??;
            cx.update_window(window.into(), |_, window, _| window.remove_window())?;
            shots.push(image);
        }
    }
    let gap = 8;
    let width = shots.iter().map(|shot| shot.width()).max().unwrap_or(0);
    let height = shots.iter().map(|shot| shot.height() + gap).sum::<u32>();
    let mut tiled = image::RgbaImage::from_pixel(width, height, image::Rgba([128, 128, 128, 255]));
    let mut y = 0;
    for shot in &shots {
        image::imageops::replace(&mut tiled, shot, 0, y as i64);
        y += shot.height() + gap;
    }
    Ok(tiled)
}

/// The frames of an animation, as the display gives them, one under the
/// other: the bottom of the window, where the field is.
async fn film(
    window: gpui::AnyWindowHandle,
    strip: &Strip,
    cx: &mut AsyncApp,
) -> anyhow::Result<image::RgbaImage> {
    let mut frames = Vec::new();
    for _ in 0..strip.frames {
        let frame = cx.update_window(window, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            window.render_to_image()
        })??;
        frames.push(frame);
        cx.background_executor()
            .timer(Duration::from_millis(strip.every_ms))
            .await;
    }
    let (width, height) = frames[0].dimensions();
    let keep = ((height as f32) * strip.keep).round() as u32;
    let gap = 6;
    let mut tiled = image::RgbaImage::from_pixel(
        width,
        (keep + gap) * frames.len() as u32 - gap,
        image::Rgba([128, 128, 128, 255]),
    );
    for (at, frame) in frames.iter().enumerate() {
        let part = image::imageops::crop_imm(frame, 0, height - keep, width, keep).to_image();
        image::imageops::replace(&mut tiled, &part, 0, (at as u32 * (keep + gap)) as i64);
    }
    Ok(tiled)
}

/// Draws frames for `ms` milliseconds, as a display would: a hidden window
/// gets none of its own, and what moves would otherwise start only at the
/// next capture.
async fn run_frames(
    window: gpui::AnyWindowHandle,
    ms: u64,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let until = std::time::Instant::now() + Duration::from_millis(ms);
    loop {
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })?;
        if std::time::Instant::now() >= until {
            return Ok(());
        }
        cx.background_executor()
            .timer(Duration::from_millis(16))
            .await;
    }
}

/// A history file of the scenes' own, with prompts sent in other cockpits.
fn history_file() -> PathBuf {
    let path = std::env::temp_dir().join("kou-thread-capture-history.json");
    let _ = std::fs::write(
        &path,
        r#"[{"Prompt":"Сделай задержку повтора экспоненциальной с джиттером","SessionID":"a","Title":"","Model":"","CreatedAt":"2026-11-10T10:00:00Z"},
            {"Prompt":"Почему retry_test падает после смены формулы?","SessionID":"a","Title":"","Model":"","CreatedAt":"2026-11-10T12:30:00Z"},
            {"Prompt":"Добавь метрику по истёкшим арендам в retry queue","SessionID":"b","Title":"","Model":"","CreatedAt":"2026-11-11T09:15:00Z"},
            {"Prompt":"Запусти тесты крейта и скажи, что упало.","SessionID":"c","Title":"","Model":"","CreatedAt":"2026-11-11T11:40:00Z"}]"#,
    );
    path
}

fn feed(end: &BackendEnd, updates: Vec<Update>) {
    for update in updates {
        end.updates.try_send(update).expect("the view is listening");
    }
}

/// Let the view's pump and a frame run.
async fn settle(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(60))
        .await;
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn repo(path: &str) -> PathBuf {
    home().join("Projects/Go/kou-conveyor").join(path)
}

fn choice(id: &str, name: &str, description: &str) -> Choice {
    Choice {
        id: id.into(),
        name: name.into(),
        description: (!description.is_empty()).then(|| description.to_owned()),
    }
}

/// kou's own agent, as `kou-native` says it is ready: efforts as modes, the
/// connection's models, then who serves them.
fn kou_ready() -> Update {
    let effort = |level: &str| {
        choice(
            level,
            &format!("Effort: {level}"),
            "How hard the model thinks before it answers",
        )
    };
    Update::Ready {
        session: "0198f2c4".into(),
        capabilities: Capabilities {
            images: true,
            embedded_files: false,
            resume: true,
        },
        modes: ["minimal", "low", "medium", "high", "xhigh"]
            .into_iter()
            .map(effort)
            .collect(),
        mode: Some("high".into()),
        models: [
            ("claude-opus-5-5", "Claude Opus 5.5"),
            ("claude-sonnet-5", "Claude Sonnet 5"),
            ("claude-haiku-5", "Claude Haiku 5"),
            ("claude-opus-5", "Claude Opus 5"),
            ("gpt-6.1-sol", "gpt-6.1-sol"),
            ("gpt-6.1-mini", "gpt-6.1-mini"),
            ("codex-6", "codex-6"),
            ("gemini-4-pro", "Gemini 4 Pro"),
            ("gemini-4-flash", "Gemini 4 Flash"),
            ("grok-4.7", "grok-4.7"),
            ("kimi-k3", "Kimi K3"),
            ("deepseek-v4", "DeepSeek V4"),
            ("gpt-image-2", "gpt-image-2"),
        ]
        .into_iter()
        .map(|(id, name)| Choice {
            id: id.into(),
            name: name.into(),
            description: None,
        })
        .collect(),
        model: Some("claude-opus-5-5".into()),
    }
}

/// Who serves kou's models, as `kou-native` groups them.
fn kou_providers() -> Update {
    let provider = |id: &str, name: &str, models: &[&str]| kou_thread::Provider {
        id: id.into(),
        name: name.into(),
        models: models.iter().map(|model| (*model).to_owned()).collect(),
    };
    Update::Providers(vec![
        provider(
            "anthropic",
            "Anthropic",
            &[
                "claude-opus-5-5",
                "claude-sonnet-5",
                "claude-haiku-5",
                "claude-opus-5",
            ],
        ),
        provider(
            "openai",
            "OpenAI",
            &["gpt-6.1-sol", "gpt-6.1-mini", "codex-6"],
        ),
        provider("google", "Google", &["gemini-4-pro", "gemini-4-flash"]),
        provider("xai", "xAI", &["grok-4.7"]),
        provider("moonshot", "Moonshot", &["kimi-k3"]),
        provider("deepseek", "DeepSeek", &["deepseek-v4"]),
    ])
}

/// A turn under way: a prompt, a call done, the agent about to answer.
fn streaming() -> Vec<Update> {
    vec![
        kou_ready(),
        kou_providers(),
        Update::UserChunk(Block::Text(
            "Why does the retry queue back off twice as long as the config says?".into(),
        )),
        Update::TurnStarted,
        Update::Model("claude-opus-5-5".into()),
        Update::Tool(ToolCall {
            id: "t1".into(),
            title: "Read internal/runner/retry.go".into(),
            kind: ToolKind::Read,
            status: ToolStatus::Done,
            ..ToolCall::default()
        }),
    ]
}

/// The answer, as it streams in.
fn streamed() -> Vec<Update> {
    vec![Update::AssistantChunk(
        "The delay is doubled in two places: `backoff` already returns the doubled \
             wait, and `requeue` doubles it again before it schedules the task."
            .into(),
    )]
}

/// Reasoning, as it streams in.
fn thought() -> Vec<Update> {
    vec![Update::ThoughtChunk(
        "**Tracing the backoff**\nThe config says 2s; the log shows 4s, then 8s.".into(),
    )]
}

/// A turn whose command runs.
fn running() -> Vec<Update> {
    vec![
        kou_ready(),
        kou_providers(),
        Update::UserChunk(Block::Text("Run the runner's tests.".into())),
        Update::TurnStarted,
        Update::Model("claude-opus-5-5".into()),
        Update::Tool(ToolCall {
            id: "t1".into(),
            title: "Run go test ./internal/runner/...".into(),
            kind: ToolKind::Execute,
            status: ToolStatus::Running,
            content: vec![ToolContent::Terminal {
                id: "term-1".into(),
                output: "=== RUN   TestBackoffGrows\n--- PASS: TestBackoffGrows (0.00s)\n=== RUN   TestRequeueSchedulesLater\n".into(),
                exit: None,
            }],
            ..ToolCall::default()
        }),
    ]
}

fn ready() -> Update {
    Update::Ready {
        session: "0198f2c4".into(),
        capabilities: Capabilities::default(),
        modes: vec![
            choice(
                "ask",
                "Ask before edits",
                "Every change waits for your answer",
            ),
            choice(
                "auto",
                "Auto-edit",
                "Files change without asking; commands still ask",
            ),
            choice("plan", "Plan only", "Reads and proposes, changes nothing"),
        ],
        mode: Some("ask".into()),
        models: vec![
            choice("sonnet", "Sonnet 4.5", "Fast, for everyday changes"),
            choice("opus", "Opus 4.5", "Slower, for hard problems"),
            choice("haiku", "Haiku 4.5", ""),
        ],
        model: Some("opus".into()),
    }
}

fn commands() -> Update {
    Update::Commands(vec![
        choice(
            "compact",
            "compact",
            "Summarise the conversation to free context",
        ),
        choice("review", "review", "Review the working tree against main"),
        choice("resume", "resume", "Pick up an earlier session"),
        choice("init", "init", "Write an AGENTS.md for this repository"),
    ])
}

fn offers() -> Update {
    Update::Offers(Offers {
        listening: false,
        steer: true,
        rewind: false,
        arguments: vec![("compact".into(), "[focus]".into())],
    })
}

fn ready_with_pictures() -> Update {
    let Update::Ready {
        session,
        modes,
        mode,
        models,
        model,
        ..
    } = ready()
    else {
        unreachable!("ready is ready");
    };
    Update::Ready {
        session,
        capabilities: Capabilities {
            images: true,
            ..Capabilities::default()
        },
        modes,
        mode,
        models,
        model,
    }
}

/// A turn at work, which takes messages while it runs.
fn queued() -> Vec<Update> {
    let mut updates = working();
    updates.insert(2, offers());
    updates
}

/// A prompt with pictures, and an answer with code in two languages.
fn transcript() -> Vec<Update> {
    let picture = |name: &str| Block::Image {
        mime: "image/png".into(),
        data: std::fs::read(repo(name)).unwrap_or_default().into(),
    };
    vec![
        ready_with_pictures(),
        user("Сравни подсветку на "),
        Update::UserChunk(picture("code-syntax-dark.png")),
        user(" и на "),
        Update::UserChunk(picture("code-syntax-light.png")),
        user(", и поправь backoff в $internal/runner/retry.go"),
        Update::TurnStarted,
        Update::AssistantChunk(
            "Разница в токенах ключевых слов. Вот исправленный `backoff`:\n\n\
             ```go\n\
             // backoff is the delay before a retry: exponential, capped, with jitter.\n\
             func backoff(attempt int) time.Duration {\n\
             \tbase := time.Second << min(attempt, 6)\n\
             \treturn base/2 + time.Duration(rand.Int63n(int64(base/2)))\n\
             }\n\
             ```\n\n\
             И проверка:\n\n\
             ```rust\n\
             #[test]\n\
             fn backoff_grows() {\n\
             \x20   let delays: Vec<u64> = (1..5).map(|n| backoff(n).as_secs()).collect();\n\
             \x20   assert!(delays.windows(2).all(|w| w[0] <= w[1]), \"{delays:?}\");\n\
             }\n\
             ```"
            .into(),
        ),
        Update::TurnEnded(Stop::Finished),
    ]
}

fn usage() -> Usage {
    Usage {
        context_used: Some(48_210),
        context_limit: Some(200_000),
        input_tokens: Some(183_400),
        output_tokens: Some(9_812),
        cost_usd: Some(1.37),
    }
}

fn user(text: &str) -> Update {
    Update::UserChunk(Block::Text(text.into()))
}

fn tool(id: &str, title: &str, kind: ToolKind, status: ToolStatus) -> ToolCall {
    ToolCall {
        id: id.into(),
        title: title.into(),
        kind,
        status,
        ..ToolCall::default()
    }
}

fn answer() -> Vec<Update> {
    vec![
        ready(),
        commands(),
        Update::Title("Очередь повторов".into()),
        user("Объясни, как устроена очередь повторов в runner и что будет, если воркер упадёт посреди задачи."),
        Update::TurnStarted,
        Update::ThoughtChunk(
            "**Looking at the retry path.** The queue lives in `internal/runner/retry.go`; \
             I should check how leases expire before answering.\n\nThe lease TTL is the key detail."
                .into(),
        ),
        Update::AssistantChunk(
            "## Очередь повторов\n\n\
             Задача не удаляется из очереди, пока воркер её **не подтвердил**. Вместо этого он берёт \
             *аренду* (`lease`) на 30 секунд и продлевает её, пока работает. Подробности в \
             [описании формата](https://example.com/kou/retry-queue) и в `internal/runner/retry.go`.\n\n\
             1. Воркер забирает задачу и ставит `leased_until`.\n\
             2. Каждые 10 секунд продлевает аренду.\n\
             3. По завершении пишет результат и удаляет запись.\n\
             \x20\x20\x20- при ошибке увеличивает `attempt`\n\
             \x20\x20\x20- после пятой попытки переносит в `dead`\n\n\
             > Если воркер упал, аренда просто истекает, и задачу берёт следующий. \
             Повтор возможен, поэтому обработчики должны быть идемпотентными.\n\n\
             ```go\n\
             func (q *Queue) Lease(ctx context.Context, worker string) (*Task, error) {\n\
             \treturn q.store.ClaimOldest(ctx, worker, time.Now().Add(q.leaseTTL), WithMaxAttempts(q.maxAttempts), WithBackoff(q.backoff))\n\
             }\n\
             ```\n\n\
             | Поле | Тип | Назначение в `Task` |\n\
             |:-----|:---:|:-----------|\n\
             | `attempt` | int | сколько раз задачу уже брали |\n\
             | `leased_until` | time | до какого момента она занята |\n\
             | `last_error` | text | чем закончилась прошлая попытка |\n\n\
             ---\n\n\
             - [x] аренда и продление\n\
             - [ ] метрика по истёкшим арендам, см. https://example.com/kou/issues/412\n"
                .into(),
        ),
        Update::Usage(usage()),
        Update::TurnEnded(Stop::Finished),
    ]
}

fn plan(states: [PlanStatus; 4]) -> Update {
    let texts = [
        "Найти, где считается задержка повтора",
        "Заменить линейную задержку на экспоненциальную с джиттером",
        "Обновить тесты очереди",
        "Прогнать go test ./internal/runner/...",
    ];
    Update::Plan(
        texts
            .into_iter()
            .zip(states)
            .map(|(text, status)| PlanStep {
                text: text.into(),
                status,
            })
            .collect(),
    )
}

const OLD_RETRY: &str = "package runner\n\nimport \"time\"\n\n// backoff is the delay before a retry.\nfunc backoff(attempt int) time.Duration {\n\treturn time.Duration(attempt) * time.Second\n}\n\nfunc (q *Queue) requeue(t *Task) {\n\tt.Attempt++\n\tt.RunAfter = time.Now().Add(backoff(t.Attempt))\n\tq.store.Put(t)\n}\n";
const NEW_RETRY: &str = "package runner\n\nimport (\n\t\"math/rand\"\n\t\"time\"\n)\n\n// backoff is the delay before a retry: exponential, capped, with jitter.\nfunc backoff(attempt int) time.Duration {\n\tbase := time.Second << min(attempt, 6)\n\treturn base/2 + time.Duration(rand.Int63n(int64(base/2)))\n}\n\nfunc (q *Queue) requeue(t *Task) {\n\tt.Attempt++\n\tt.RunAfter = time.Now().Add(backoff(t.Attempt))\n\tq.store.Put(t)\n}\n";

/// Two prompts, each named for an edit, from a backend that rewinds.
fn edit() -> Vec<Update> {
    let mut updates = answer();
    updates.insert(
        1,
        Update::Offers(Offers {
            listening: false,
            steer: true,
            rewind: true,
            arguments: Vec::new(),
        }),
    );
    let prompt = updates
        .iter()
        .position(|update| matches!(update, Update::UserChunk(_)))
        .expect("a prompt");
    updates.insert(prompt + 1, Update::PromptId("p1".into()));
    updates.extend([
        user("А если воркер не упал, а завис?"),
        Update::PromptId("p2".into()),
        Update::TurnStarted,
        Update::Tool(tool(
            "t9",
            "Read internal/runner/lease.go",
            ToolKind::Read,
            ToolStatus::Done,
        )),
        Update::AssistantChunk(
            "Тогда аренду продлевать некому: она истекает так же, как при падении.".into(),
        ),
        Update::TurnEnded(Stop::Finished),
    ]);
    updates
}

fn tools() -> Vec<Update> {
    let mut read = tool(
        "t1",
        "Read internal/runner/retry.go",
        ToolKind::Read,
        ToolStatus::Done,
    );
    read.locations = vec![Location {
        path: repo("internal/runner/retry.go"),
        line: Some(41),
    }];
    read.content = vec![ToolContent::Text("84 lines".into())];
    let mut search = tool(
        "t2",
        "Search \"backoff(\" in internal/",
        ToolKind::Search,
        ToolStatus::Done,
    );
    search.content = vec![ToolContent::Text("3 matches in 2 files".into())];
    let mut edit = tool("t3", "Edit retry.go", ToolKind::Edit, ToolStatus::Done);
    edit.locations = vec![Location {
        path: repo("internal/runner/retry.go"),
        line: None,
    }];
    edit.content = vec![ToolContent::Diff {
        path: repo("internal/runner/retry.go"),
        old: Some(OLD_RETRY.into()),
        new: NEW_RETRY.into(),
    }];
    let mut test = tool(
        "t4",
        "Run go test ./internal/runner/...",
        ToolKind::Execute,
        ToolStatus::Failed,
    );
    test.content = vec![ToolContent::Terminal {
        id: "term-1".into(),
        output: "\u{1b}[1m=== RUN   TestBackoffGrows\u{1b}[0m\n\
                 --- \u{1b}[32mPASS\u{1b}[0m: TestBackoffGrows (0.00s)\n\
                 \u{1b}[1m=== RUN   TestRequeueSchedulesLater\u{1b}[0m\n\
                 \u{20}   retry_test.go:58: delay for attempt 3 = 9.4s, want between 4s and 8s after the change to exponential backoff with jitter\n\
                 --- \u{1b}[31mFAIL\u{1b}[0m: TestRequeueSchedulesLater (0.01s)\n\
                 \u{1b}[31mFAIL\u{1b}[0m\n\
                 FAIL\tgithub.com/kou/conveyor/internal/runner\t0.412s\n"
            .into(),
        exit: Some(1),
    }];
    vec![
        ready(),
        user("Сделай задержку повтора экспоненциальной с джиттером и проверь тестами."),
        Update::TurnStarted,
        plan([
            PlanStatus::Done,
            PlanStatus::Done,
            PlanStatus::Running,
            PlanStatus::Pending,
        ]),
        Update::Tool(read),
        Update::Tool(search),
        Update::Tool(edit),
        Update::Tool(test),
        Update::Notice(Level::Info, "Context compacted: 31 earlier messages summarised.".into()),
        Update::Notice(
            Level::Warning,
            "go.sum changed outside this session; the test cache was dropped.".into(),
        ),
        Update::Notice(Level::Error, "MCP server \"github\" failed to start: exit status 127.".into()),
        Update::AssistantChunk("Один тест падает: верхняя граница в `retry_test.go` посчитана для старой формулы. Исправляю ожидание.".into()),
        Update::Usage(usage()),
    ]
}

fn permission() -> Vec<Update> {
    let mut edit = tool(
        "t3",
        "Edit internal/runner/retry.go",
        ToolKind::Edit,
        ToolStatus::Pending,
    );
    edit.locations = vec![Location {
        path: repo("internal/runner/retry.go"),
        line: Some(5),
    }];
    edit.content = vec![ToolContent::Diff {
        path: repo("internal/runner/retry.go"),
        old: Some(OLD_RETRY.into()),
        new: NEW_RETRY.into(),
    }];
    let mut read = tool(
        "t1",
        "Read internal/runner/retry.go",
        ToolKind::Read,
        ToolStatus::Done,
    );
    read.content = vec![ToolContent::Text("84 lines".into())];
    vec![
        ready(),
        user("Сделай задержку повтора экспоненциальной с джиттером."),
        Update::TurnStarted,
        Update::ThoughtChunk("The delay is computed in one place, `backoff`. A shift with a cap and half of it as jitter keeps retries from lining up.".into()),
        Update::Tool(read),
        Update::AssistantChunk("Задержка считается в одной функции. Меняю её и импорты:".into()),
        Update::Permission(PermissionRequest {
            questions: Vec::new(),
            id: 7,
            tool: edit,
            options: vec![
                PermissionOption {
                    id: "once".into(),
                    label: "Allow once".into(),
                    kind: PermissionKind::AllowOnce,
                },
                PermissionOption {
                    id: "always".into(),
                    label: "Always allow edits".into(),
                    kind: PermissionKind::AllowAlways,
                },
                PermissionOption {
                    id: "reject".into(),
                    label: "Reject".into(),
                    kind: PermissionKind::RejectOnce,
                },
            ],
        }),
        Update::Usage(usage()),
    ]
}

fn working() -> Vec<Update> {
    let mut run = tool(
        "t9",
        "Run cargo test -p kou-thread-view",
        ToolKind::Execute,
        ToolStatus::Running,
    );
    run.content = vec![ToolContent::Terminal {
        id: "term-2".into(),
        output: "\u{1b}[1m\u{1b}[32m   Compiling\u{1b}[0m kou-thread v0.1.0\n\
                 \u{1b}[1m\u{1b}[32m   Compiling\u{1b}[0m kou-ui v0.1.0\n\
                 \u{1b}[1m\u{1b}[32m   Compiling\u{1b}[0m kou-thread-view v0.1.0\n\
                 \u{1b}[1m\u{1b}[33mwarning\u{1b}[0m: unused variable: `window`\n\
                 \u{1b}[1m\u{1b}[32m    Finished\u{1b}[0m `test` profile [unoptimized + debuginfo] target(s) in 41.20s\n\
                 \u{1b}[1m\u{1b}[32m     Running\u{1b}[0m unittests src/lib.rs\n\n\
                 running 58 tests\n"
            .into(),
        exit: None,
    }];
    let fetch = tool(
        "t8",
        "Fetch docs.rs/pulldown-cmark",
        ToolKind::Fetch,
        ToolStatus::Cancelled,
    );
    vec![
        ready(),
        Update::Mode("auto".into()),
        user("Запусти тесты крейта и скажи, что упало."),
        Update::TurnStarted,
        Update::Tool(fetch),
        Update::Tool(run),
        Update::ThoughtChunk(
            "The build finished with one warning. While the tests run, the unused `window` \
             parameter is worth a look: it is part of the public constructor, so it stays.\n\n\
             Next: read the failures, if any."
                .into(),
        ),
        Update::Usage(Usage {
            context_used: Some(12_050),
            context_limit: None,
            input_tokens: None,
            output_tokens: Some(640),
            cost_usd: None,
        }),
    ]
}

/// What the zoomed captures compare: a prompt with a picture, an answer with
/// code, a tool call that ran, one that runs, with commands and a queue on offer.
/// A turn that is over in which the agent ran a command, started a server
/// and left it running in the background, and one it left earlier that has
/// ended since.
fn background_command() -> Vec<Update> {
    let command = |id: &str, title: &str, output: &str| {
        let mut call = tool(id, title, ToolKind::Execute, ToolStatus::Done);
        call.content = vec![ToolContent::Terminal {
            id: id.into(),
            output: output.into(),
            exit: None,
        }];
        call
    };
    let mut ended = command(
        "b0",
        "`cargo build --release`",
        "Command running in background with ID: bash_1. Output is being written to: /tmp/tasks/bash_1.output\n",
    );
    ended.work.background = true;
    ended.work.ended = true;
    let mut listed = command("c1", "`ls dist`", "index.html\nassets\n");
    listed.content = vec![ToolContent::Terminal {
        id: "c1".into(),
        output: "index.html\nassets\n".into(),
        exit: Some(0),
    }];
    let mut server = command(
        "b1",
        "`npm run dev -- --port 5173`",
        "Command running in background with ID: bash_2. Output is being written to: /tmp/tasks/bash_2.output\n",
    );
    server.work.background = true;
    vec![
        ready(),
        commands(),
        Update::Title("Дев-сервер".into()),
        user("Собери проект и подними дев-сервер, я посмотрю в браузере."),
        Update::TurnStarted,
        Update::Tool(ended),
        Update::Tool(listed),
        Update::Tool(server),
        Update::AssistantChunk(
            "Сборка прошла, сервер поднят на `localhost:5173` и работает в фоне.".into(),
        ),
        Update::Usage(usage()),
        Update::TurnEnded(Stop::Finished),
    ]
}

/// A turn in which the agent starts three agents of its own, as Claude Code
/// does: one that has reported, one at work, and one left to run in the
/// background, whose call is over while it goes on.
fn agents_at_work() -> Vec<Update> {
    let launch = |id: &str, title: &str, kind: &str, asked: &str, status| {
        let mut call = tool(id, title, ToolKind::Think, status);
        call.agent = Some(kind.into());
        call.raw_input = Some(serde_json::json!({
            "description": title,
            "subagent_type": kind,
            "prompt": asked,
        }));
        call.content = vec![ToolContent::Text(asked.into())];
        call
    };
    let step = |agent: &str, id: &str, title: &str, kind, status| {
        let mut call = tool(id, title, kind, status);
        call.parent = Some(agent.into());
        Update::Tool(call)
    };
    let said = |agent: &str, text: &str| Update::AgentSaid {
        call: agent.into(),
        text: text.into(),
    };
    let mut reported = launch(
        "a1",
        "Проверить тесты парсера",
        "Explore",
        "Прогони тесты крейта parser и скажи, какие падают и почему.",
        ToolStatus::Done,
    );
    reported.content = vec![ToolContent::Text(
        "Падает один тест из двенадцати: `parses_a_nested_list`. Отступ считается в байтах, а не в символах.".into(),
    )];
    reported.work.tokens = Some(48_211);
    let working = launch(
        "a2",
        "Найти, где считается отступ",
        "general-purpose",
        "Найди все места, где длина отступа берётся в байтах, и предложи правку.\nНичего не меняй: нужен только список мест.",
        ToolStatus::Running,
    );
    let mut behind = launch(
        "a3",
        "Следить за сборкой в CI",
        "general-purpose",
        "Дождись конца сборки ветки и скажи, чем она закончилась.",
        ToolStatus::Done,
    );
    behind.work.background = true;
    behind.content = vec![ToolContent::Text(
        "Async agent launched successfully. The agent is working in the background.".into(),
    )];
    vec![
        ready(),
        commands(),
        Update::Title("Отступы в парсере".into()),
        user("Разберись, почему падает тест вложенных списков, и присмотри за сборкой."),
        Update::TurnStarted,
        Update::AssistantChunk(
            "Разобью на три части: тесты, поиск мест с отступом и наблюдение за сборкой.".into(),
        ),
        Update::Tool(reported),
        step(
            "a1",
            "c1",
            "Read crates/parser/src/list.rs",
            ToolKind::Read,
            ToolStatus::Done,
        ),
        step(
            "a1",
            "c2",
            "`cargo test -p parser`",
            ToolKind::Execute,
            ToolStatus::Done,
        ),
        Update::Tool(working),
        step(
            "a2",
            "c3",
            "Search `indent` in crates/parser",
            ToolKind::Search,
            ToolStatus::Done,
        ),
        said(
            "a2",
            "Нашёл два места, где длина берётся в байтах: `list.rs` и `quote.rs`.",
        ),
        step(
            "a2",
            "c4",
            "Read crates/parser/src/quote.rs",
            ToolKind::Read,
            ToolStatus::Done,
        ),
        said(
            "a2",
            "Во втором месте то же самое. Проверяю, нет ли третьего в `indent.rs`.",
        ),
        step(
            "a2",
            "c5",
            "Read crates/parser/src/indent.rs",
            ToolKind::Read,
            ToolStatus::Running,
        ),
        Update::Tool(behind),
        step(
            "a3",
            "c6",
            "`gh run list --branch fix-indent`",
            ToolKind::Execute,
            ToolStatus::Done,
        ),
        step(
            "a3",
            "c7",
            "`gh run watch 9412`",
            ToolKind::Execute,
            ToolStatus::Running,
        ),
        Update::Tool(tool(
            "t9",
            "`git log --oneline -5`",
            ToolKind::Execute,
            ToolStatus::Done,
        )),
    ]
}

/// A turn in which the agent hands a task to a subagent of its own, as
/// Claude Code does; `done`, the subagent has reported and the turn is over.
fn delegated(done: bool) -> Vec<Update> {
    let mut launch = tool(
        "a1",
        "Проверить тесты парсера",
        ToolKind::Think,
        if done {
            ToolStatus::Done
        } else {
            ToolStatus::Running
        },
    );
    launch.agent = Some("Explore".into());
    let asked = "Прогони тесты крейта parser и скажи, какие падают и почему.\nНичего не меняй в коде: нужен только отчёт.";
    launch.raw_input = Some(serde_json::json!({
        "description": "Проверить тесты парсера",
        "subagent_type": "Explore",
        "prompt": asked,
    }));
    launch.content = vec![ToolContent::Text(if done {
        "Падает один тест из двенадцати: `parses_a_nested_list`. Причина в том, что отступ считается в байтах, а не в символах.".into()
    } else {
        asked.into()
    })];
    let child = |id: &str, title: &str, kind, status| {
        let mut call = tool(id, title, kind, status);
        call.parent = Some("a1".into());
        call
    };
    let mut updates = vec![
        ready(),
        Update::UserChunk(Block::Text("Тесты парсера в порядке?".into())),
        Update::TurnStarted,
        Update::AssistantChunk(
            "Поручу проверку субагенту, а сам посмотрю историю изменений.".into(),
        ),
        Update::Tool(launch),
        Update::Tool(child(
            "c1",
            "Read crates/parser/src/list.rs",
            ToolKind::Read,
            ToolStatus::Done,
        )),
        Update::Tool(child(
            "c2",
            "Search `indent` in crates/parser",
            ToolKind::Search,
            ToolStatus::Done,
        )),
        Update::Tool(child(
            "c3",
            "`cargo test -p parser`",
            ToolKind::Execute,
            if done {
                ToolStatus::Done
            } else {
                ToolStatus::Running
            },
        )),
        Update::Tool(tool(
            "t9",
            "`git log --oneline -5`",
            ToolKind::Execute,
            ToolStatus::Done,
        )),
    ];
    if done {
        updates.push(Update::AssistantChunk(
            "Один тест падает: отступ во вложенных списках считается в байтах.".into(),
        ));
        updates.push(Update::TurnEnded(Stop::Finished));
    }
    updates
}

/// A turn in which the agent read an image file.
fn read_a_picture() -> Vec<Update> {
    let mut read = tool(
        "t1",
        "Read tmp/shots/code-syntax-dark.png",
        ToolKind::Read,
        ToolStatus::Done,
    );
    read.content = vec![ToolContent::Image {
        mime: "image/png".into(),
        data: std::fs::read(repo("code-syntax-dark.png"))
            .expect("the picture of the repository")
            .into(),
    }];
    vec![
        ready(),
        Update::UserChunk(Block::Text("Посмотри на скриншот".into())),
        Update::TurnStarted,
        Update::Tool(read),
        Update::AssistantChunk("На скриншоте блок кода с подсветкой.".into()),
        Update::TurnEnded(Stop::Finished),
    ]
}

/// A turn in which the agent ran one long command and said what came of it.
fn ran_a_command() -> Vec<Update> {
    let mut run = tool(
        "t1",
        "`unset CARGO_MAKEFLAGS MAKEFLAGS MFLAGS; cargo test -p kou-thread-view -p kou-desktop --no-fail-fast`",
        ToolKind::Execute,
        ToolStatus::Done,
    );
    run.content = vec![ToolContent::Text(
        "```console\ntest shell::tests::the_keys_switch_layouts_open_the_picker_and_escape_closes_it ... ok\ntest state::tests::a_branch_is_read_from_the_checkout_or_from_a_worktree_pointer ... ok\ntest state::tests::a_chosen_accent_is_kept_and_read_back_as_a_colour ... ok\ntest result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\nrunning 186 tests\ntest buffer::tests::deleting_to_the_line_start_stops_at_the_line_break ... ok\ntest result: ok. 186 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n```"
            .into(),
    )];
    vec![
        ready(),
        Update::UserChunk(Block::Text("Прогони тесты".into())),
        Update::TurnStarted,
        Update::Tool(run),
        Update::AssistantChunk("Все тесты проходят: 42 в приложении и 186 в чате.".into()),
        Update::TurnEnded(Stop::Finished),
    ]
}

fn everything() -> Vec<Update> {
    let picture = Block::Image {
        mime: "image/png".into(),
        data: std::fs::read(repo("code-syntax-dark.png"))
            .unwrap_or_default()
            .into(),
    };
    let mut read = tool(
        "t1",
        "Read internal/runner/retry.go",
        ToolKind::Read,
        ToolStatus::Done,
    );
    read.locations = vec![Location {
        path: repo("internal/runner/retry.go"),
        line: Some(41),
    }];
    read.content = vec![ToolContent::Text("84 lines".into())];
    let mut run = tool(
        "t2",
        "Run go test ./internal/runner/...",
        ToolKind::Execute,
        ToolStatus::Running,
    );
    run.content = vec![ToolContent::Terminal {
        id: "term-1".into(),
        output: "\u{1b}[1m=== RUN   TestBackoffGrows\u{1b}[0m\n\
                 --- \u{1b}[32mPASS\u{1b}[0m: TestBackoffGrows (0.00s)\n\
                 \u{1b}[1m=== RUN   TestRequeueSchedulesLater\u{1b}[0m\n"
            .into(),
        exit: None,
    }];
    vec![
        ready_with_pictures(),
        commands(),
        offers(),
        user("Почему на этом экране подсветка другая? "),
        Update::UserChunk(picture),
        user(" Поправь заодно backoff в retry.go."),
        Update::TurnStarted,
        Update::Tool(read),
        Update::AssistantChunk(
            "Подсветка берёт цвета из темы терминала, а не из темы приложения, поэтому \
             ключевые слова на светлом фоне темнее. Задержку делаю экспоненциальной, с \
             потолком и джиттером:\n\n\
             ```go\n\
             func backoff(attempt int) time.Duration {\n\
             \tbase := time.Second << min(attempt, 6)\n\
             \treturn base/2 + time.Duration(rand.Int63n(int64(base/2)))\n\
             }\n\
             ```"
            .into(),
        ),
        Update::Tool(run),
        Update::Usage(usage()),
    ]
}

/// A backend that offers no modes, models or commands and reports no usage.
fn bare() -> Vec<Update> {
    vec![
        Update::Ready {
            session: "s".into(),
            capabilities: Capabilities::default(),
            modes: Vec::new(),
            mode: None,
            models: Vec::new(),
            model: None,
        },
        user("Что делает флаг --resume?"),
        Update::TurnStarted,
        Update::AssistantChunk(
            "Продолжает прошлую сессию: читает её журнал и восстанавливает контекст.".into(),
        ),
        Update::TurnEnded(Stop::Cancelled),
    ]
}

fn failed() -> Vec<Update> {
    vec![
        ready(),
        user("Обнови зависимости и собери проект."),
        Update::TurnStarted,
        Update::AssistantChunk("Смотрю `go.mod` и список устаревших модулей.".into()),
        Update::Tool(tool(
            "t1",
            "Run go list -m -u all",
            ToolKind::Execute,
            ToolStatus::Running,
        )),
        Update::TurnEnded(Stop::Failed(
            "provider returned 529: overloaded, retried 3 times over 42s".into(),
        )),
    ]
}

fn closed() -> Vec<Update> {
    vec![
        ready(),
        user("Продолжай с того места, где остановились."),
        Update::TurnStarted,
        Update::AssistantChunk("Открываю прошлую сессию и план работ.".into()),
        Update::Closed(Some(
            "kou runner exited with status 2: open /Users/kou/.kou/sessions/0198f2c4.jsonl: permission denied".into(),
        )),
    ]
}

/// A turn whose finished calls were left open by their failure or by having no
/// content of their own, to show what an opened call holds.
fn expanded() -> Vec<Update> {
    let mut write = tool(
        "t1",
        "Write docs/retry.md",
        ToolKind::Edit,
        ToolStatus::Failed,
    );
    write.content = vec![
        ToolContent::Text("The file exists and was **not** overwritten. Its first lines:".into()),
        ToolContent::Diff {
            path: repo("docs/retry.md"),
            old: None,
            new: "# Retry queue\n\nTasks are leased, not removed.\n".into(),
        },
    ];
    let mut call = tool(
        "t2",
        "github: create_issue",
        ToolKind::Other,
        ToolStatus::Failed,
    );
    call.raw_input = Some(serde_json::json!({
        "repo": "kou/conveyor",
        "title": "Count expired leases",
        "labels": ["runner", "metrics"],
    }));
    call.raw_output = Some(serde_json::json!({ "error": "401 Unauthorized" }));
    vec![
        ready(),
        user("Запиши это в docs/retry.md и заведи задачу на метрику."),
        Update::UserChunk(Block::File {
            path: repo("internal/runner/retry.go"),
            text: None,
        }),
        Update::UserChunk(Block::Image {
            mime: "image/png".into(),
            data: vec![0; 48_211].into(),
        }),
        Update::TurnStarted,
        Update::Tool(write),
        Update::Tool(call),
        Update::AssistantChunk(
            "Файл уже есть, а для GitHub нужен токен. Что из этого сделать иначе?".into(),
        ),
        Update::TurnEnded(Stop::Limit),
    ]
}
