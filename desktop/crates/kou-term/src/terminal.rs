//! The terminal entity: a child process on a pseudo-terminal and the emulator fed by it.
//!
//! Reading and parsing run on a thread of their own. This entity lives on the interface
//! thread and only looks at the emulator under a short lock.

use crate::vt::{Modes, Vt};
use crate::{
    keys::{self, KeyMode},
    osc::OscEvent,
    style::TerminalStyle,
};
use gpui::{App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, Keystroke, Task};
use parking_lot::{Mutex, MutexGuard};
use std::{
    os::fd::OwnedFd,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// The size a terminal has until a view measures its own.
const INITIAL_COLUMNS: u16 = 80;
const INITIAL_LINES: u16 = 24;

/// How long [`Terminal::submit`] waits between the pasted text and Enter. Programs that
/// guess a paste from how fast keys arrive would otherwise take the Enter for part of it
/// and insert a newline instead of sending.
const SUBMIT_DELAY: Duration = Duration::from_millis(50);

/// The longest a redraw is held back after the previous one: one frame at 60 Hz. A view
/// lowers it to its display's refresh interval.
const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

/// The longest a program may hold the screen still (synchronized output, mode 2026)
/// before it is drawn anyway.
const RENDER_HOLD_LIMIT: Duration = Duration::from_secs(1);

/// What to run in a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    /// The program: a path, or a name looked up in `PATH`.
    pub program: PathBuf,
    /// Its arguments, without the program name.
    pub args: Vec<String>,
    /// Variables set on top of the inherited environment. `TERM` and `COLORTERM` are set
    /// by the terminal and may be overridden here.
    pub env: Vec<(String, String)>,
    /// The directory the program starts in. Spawning fails when it does not exist.
    pub cwd: PathBuf,
}

/// Something a [`Terminal`] tells its subscribers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    /// The screen changed. Coalesced: at most one per frame however much the program writes.
    Wakeup,
    /// The program set the window title (OSC 0 or 2). Empty when it reset the title.
    Title(String),
    /// The program rang the bell.
    Bell,
    /// The program exited, with its status when known. A program killed by signal `n`
    /// reports `128 + n`, as a shell would. [`Terminal::exit`] says which it was and how
    /// long the program ran.
    Exited(Option<i32>),
    /// The program asked for a desktop notification (OSC 9, OSC 777 `notify`, OSC 99).
    Notification {
        /// The headline, when the sequence carries one.
        title: Option<String>,
        /// The message.
        body: String,
    },
    /// Task progress in percent (OSC 9;4). `None` clears it; a program that is busy
    /// without knowing how far along it is reports `Some(0)`.
    Progress(Option<u8>),
    /// The program's working directory changed (OSC 7).
    Cwd(PathBuf),
}

/// How a program ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    /// The code it exited with, when it exited by itself.
    pub code: Option<i32>,
    /// The signal that ended it, when one did.
    pub signal: Option<i32>,
    /// How long it ran, from being started to having exited.
    pub ran_for: Duration,
}

impl Exit {
    /// The status a shell would report: the code, or `128 + n` for signal `n`.
    pub fn status(&self) -> Option<i32> {
        self.code.or_else(|| self.signal.map(|signal| 128 + signal))
    }

    /// Whether the program exited by itself with status zero.
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }
}

/// Messages from the reader thread to the entity.
pub(crate) enum Internal {
    Title(Option<String>),
    Bell,
    Clipboard(String),
    Osc(OscEvent),
    /// The child exited and its output has been read; the status is in [`Shared`].
    Exited,
}

/// The grid size and the size of a cell in device pixels, as the program is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GridPixels {
    pub columns: u16,
    pub lines: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

/// State shared with the reader thread, which must never wait for the interface thread.
pub(crate) struct Shared {
    started: Instant,
    /// Nanoseconds from `started` to the last read, plus one; zero before any output.
    last_output: AtomicU64,
    bytes_received: AtomicU64,
    exited: AtomicBool,
    exit: Mutex<Option<Exit>>,
    pub(crate) size: Mutex<GridPixels>,
    /// Whether the style's background is dark, for colour scheme queries.
    pub(crate) dark: AtomicBool,
    /// When the program started holding the screen still, while it does.
    hold: Mutex<Option<Instant>>,
    frame_micros: AtomicU64,
    child: AtomicU32,
    hang_up_sent: AtomicBool,
    /// The program was handed to another terminal: this one neither ends it
    /// nor waits for it any more.
    released: AtomicBool,
    /// Tells the reader to stop even though the child's side is still open.
    pub(crate) shutdown: AtomicBool,
    /// The reader has seen the end of the output.
    pub(crate) drained: AtomicBool,
}

impl Shared {
    pub(crate) fn new(size: GridPixels) -> Self {
        Self {
            started: Instant::now(),
            last_output: AtomicU64::new(0),
            bytes_received: AtomicU64::new(0),
            exited: AtomicBool::new(false),
            exit: Mutex::new(None),
            size: Mutex::new(size),
            dark: AtomicBool::new(true),
            hold: Mutex::new(None),
            frame_micros: AtomicU64::new(DEFAULT_FRAME_INTERVAL.as_micros() as u64),
            child: AtomicU32::new(0),
            hang_up_sent: AtomicBool::new(false),
            released: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            drained: AtomicBool::new(false),
        }
    }

    /// Records that the child wrote `count` bytes just now.
    pub(crate) fn note_output(&self, count: usize) {
        let elapsed = self.started.elapsed().as_nanos() as u64;
        self.last_output.store(elapsed + 1, Ordering::Relaxed);
        self.bytes_received
            .fetch_add(count as u64, Ordering::Relaxed);
    }

    pub(crate) fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    pub(crate) fn set_child(&self, pid: u32) {
        self.child.store(pid, Ordering::Release);
    }

    /// The child's process id; 0 before there is one.
    pub(crate) fn child(&self) -> u32 {
        self.child.load(Ordering::Acquire)
    }

    /// Gives the program up to another terminal: from here on this one will
    /// not hang it up, whatever becomes of it.
    pub(crate) fn disown(&self) {
        self.hang_up_sent.store(true, Ordering::Release);
        self.released.store(true, Ordering::Release);
    }

    /// Whether the program was handed to another terminal.
    pub(crate) fn released(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }

    /// Records how the child ended.
    pub(crate) fn set_exit(&self, exit: Exit) {
        *self.exit.lock() = Some(exit);
        self.exited.store(true, Ordering::Release);
    }

    pub(crate) fn started(&self) -> Instant {
        self.started
    }

    pub(crate) fn set_render_hold(&self, held: bool) {
        *self.hold.lock() = held.then(Instant::now);
    }

    /// Whether redraws are held back because the program is in the middle of drawing.
    pub(crate) fn render_held(&self) -> bool {
        self.hold
            .lock()
            .is_some_and(|since| since.elapsed() < RENDER_HOLD_LIMIT)
    }

    /// The child to signal, once: `None` when it is gone or was already told to go.
    pub(crate) fn take_child_to_hang_up(&self) -> Option<u32> {
        if self.has_exited() || self.hang_up_sent.swap(true, Ordering::AcqRel) {
            return None;
        }
        match self.child.load(Ordering::Acquire) {
            // Process group 0 or 1 would be this application or everything.
            0 | 1 => None,
            pid => Some(pid),
        }
    }
}

/// Bytes for the child, in the order they were handed in.
pub(crate) enum Input {
    Bytes(Vec<u8>),
    /// Wait before writing what follows.
    Pause(Duration),
}

/// A child process on a pseudo-terminal, and the screen it has drawn.
///
/// A GPUI entity: subscribe to it for [`TerminalEvent`]s, show it with a
/// [`TerminalView`](crate::TerminalView), or drive it without one through
/// [`Terminal::text`] and [`Terminal::submit`].
pub struct Terminal {
    vt: Arc<Mutex<Vt>>,
    shared: Arc<Shared>,
    input: mpsc::Sender<Input>,
    /// The parent's side of the pseudo-terminal, for resizing it.
    master: Option<OwnedFd>,
    title: Option<String>,
    cwd: Option<PathBuf>,
    exit: Option<Option<i32>>,
    // How many commands the program has said are over.
    finished: u64,
    _tasks: [Task<()>; 2],
}

/// A running child with its emulator, not yet tied to an entity.
struct Started {
    vt: Arc<Mutex<Vt>>,
    shared: Arc<Shared>,
    input: mpsc::Sender<Input>,
    master: Option<OwnedFd>,
    internal: async_channel::Receiver<Internal>,
    woken: async_channel::Receiver<()>,
}

impl Started {
    fn new(spawn: &Spawn) -> anyhow::Result<Self> {
        let size = GridPixels {
            columns: INITIAL_COLUMNS,
            lines: INITIAL_LINES,
            cell_width: 8,
            cell_height: 18,
        };
        let shared = Arc::new(Shared::new(size));
        let (events, internal) = async_channel::unbounded();
        let (wakeups, woken) = async_channel::bounded(1);
        let (input, queued) = mpsc::channel::<Input>();
        let vt = Vt::new(
            size.columns,
            size.lines,
            &TerminalStyle::default(),
            input.clone(),
            events.clone(),
            shared.clone(),
        )?;
        let vt = Arc::new(Mutex::new(vt));
        let master = start_child(
            spawn,
            size,
            vt.clone(),
            shared.clone(),
            events,
            wakeups,
            queued,
        )?;
        Ok(Self {
            vt,
            shared,
            input,
            master,
            internal,
            woken,
        })
    }
}

impl Started {
    /// A program another terminal let go of, carried on: an emulator drawn
    /// as the other's was, and threads of this terminal's own on the same
    /// pseudo-terminal.
    #[cfg(unix)]
    fn adopted(released: &Released) -> anyhow::Result<Self> {
        use std::os::fd::FromRawFd as _;
        let size = GridPixels {
            columns: released.columns.max(1),
            lines: released.lines.max(1),
            cell_width: released.cell_width,
            cell_height: released.cell_height,
        };
        let shared = Arc::new(Shared::new(size));
        let (events, internal) = async_channel::unbounded();
        let (wakeups, woken) = async_channel::bounded(1);
        let (input, queued) = mpsc::channel::<Input>();
        let mut vt = Vt::new(
            size.columns,
            size.lines,
            &TerminalStyle::default(),
            input.clone(),
            events.clone(),
            shared.clone(),
        )?;
        vt.resize(
            size.columns,
            size.lines,
            u32::from(size.cell_width),
            u32::from(size.cell_height),
        );
        // What was on the other's screen, and the modes the program had set.
        vt.write(&released.screen);
        let vt = Arc::new(Mutex::new(vt));
        // SAFETY: `Released` says the descriptor is open and is its holder's
        // to own; it becomes this terminal's here.
        let master = unsafe { OwnedFd::from_raw_fd(released.master) };
        let master = crate::pty::adopt(
            master,
            released.pid,
            vt.clone(),
            shared.clone(),
            events,
            wakeups,
            queued,
        )?;
        Ok(Self {
            vt,
            shared,
            input,
            master: Some(master),
            internal,
            woken,
        })
    }
}

/// A running program that one [`Terminal`] let go of for another to carry on
/// with: see [`Terminal::release`] and [`Terminal::adopt`]. Plain values, so
/// that it can be kept as data between the two, by code that knows nothing
/// of either terminal's insides.
///
/// Whoever holds one owns an open descriptor and a running program: hand it
/// to [`Terminal::adopt`], or end it with [`Released::discard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Released {
    /// The parent's side of the pseudo-terminal: an open descriptor.
    pub master: i32,
    /// The program's process id, which is also its process group.
    pub pid: u32,
    /// The grid, in cells.
    pub columns: u16,
    /// See `columns`.
    pub lines: u16,
    /// A cell's size in device pixels, as the program was told.
    pub cell_width: u16,
    /// See `cell_width`.
    pub cell_height: u16,
    /// The screen and its history as the escape sequences that draw them
    /// again, with the modes the program had set.
    pub screen: Vec<u8>,
    /// The title the program gave its window.
    pub title: Option<String>,
    /// The directory the program said it works in.
    pub cwd: Option<PathBuf>,
}

impl Released {
    /// End the program after all: nobody took it over. Its process group is
    /// hung up and the descriptor closed.
    pub fn discard(self) {
        #[cfg(unix)]
        {
            use std::os::fd::FromRawFd as _;
            if self.pid > 1 {
                // SAFETY: plain signal delivery to the process group this
                // value stands for.
                unsafe { libc::killpg(self.pid as libc::pid_t, libc::SIGHUP) };
            }
            // SAFETY: the descriptor is this value's to own, and is closed here.
            drop(unsafe { OwnedFd::from_raw_fd(self.master) });
        }
    }
}

// The whole of a terminal as escape sequences: its history and its screen,
// and everything about its state the formatter can write (modes, margins,
// tab stops, the keyboard protocol, where the cursor is).
fn dump(vt: &Vt) -> Vec<u8> {
    use ghostty_vt_sys as sys;
    use std::{ffi::c_int, ptr};
    let mut formatter: sys::GhosttyFormatter = ptr::null_mut();
    // SAFETY: plain calls on a valid terminal; the formatter and the buffer
    // it returns are freed before returning.
    unsafe {
        if sys::ghostty_formatter_terminal_new(ptr::null(), &mut formatter, vt.raw())
            != sys::GHOSTTY_SUCCESS
            || formatter.is_null()
        {
            return Vec::new();
        }
        let format: c_int = sys::GHOSTTY_FORMATTER_FORMAT_VT;
        sys::ghostty_formatter_set(
            formatter,
            sys::GHOSTTY_FORMATTER_OPT_EMIT,
            (&raw const format).cast(),
        );
        let extras: u32 = sys::GHOSTTY_FORMATTER_EXTRAS_ALL;
        sys::ghostty_formatter_set(
            formatter,
            sys::GHOSTTY_FORMATTER_OPT_EXTRAS,
            (&raw const extras).cast(),
        );
        // Rows as they are: blanks at the end of a row are part of where
        // the cursor is.
        let trim = false;
        sys::ghostty_formatter_set(
            formatter,
            sys::GHOSTTY_FORMATTER_OPT_TRIM,
            (&raw const trim).cast(),
        );
        let mut bytes: *mut u8 = ptr::null_mut();
        let mut length = 0usize;
        let result =
            sys::ghostty_formatter_format_alloc(formatter, ptr::null(), &mut bytes, &mut length);
        let screen = if result == sys::GHOSTTY_SUCCESS && !bytes.is_null() {
            let screen = std::slice::from_raw_parts(bytes, length).to_vec();
            sys::ghostty_free(ptr::null(), bytes, length);
            screen
        } else {
            Vec::new()
        };
        sys::ghostty_formatter_free(formatter);
        screen
    }
}

impl EventEmitter<TerminalEvent> for Terminal {}

impl Terminal {
    /// Starts `spawn.program` on a new pseudo-terminal of 80 by 24 cells.
    ///
    /// Use inside `cx.new`; [`Terminal::open`] does that and hands back the error instead
    /// of an entity when the program cannot be started.
    pub fn spawn(spawn: Spawn, cx: &mut Context<Self>) -> anyhow::Result<Self> {
        Ok(Self::attach(Started::new(&spawn)?, cx))
    }

    /// Starts a program and returns the terminal entity, or why it could not be started.
    pub fn open(spawn: Spawn, cx: &mut App) -> anyhow::Result<Entity<Self>> {
        // The child is started before the entity exists, because an entity's constructor
        // has no way to fail.
        let started = Started::new(&spawn)?;
        Ok(cx.new(|cx| Self::attach(started, cx)))
    }

    /// Stop reading what the program writes; what it writes from here on
    /// waits in the pseudo-terminal. The first half of [`Terminal::release`],
    /// to call on every terminal before releasing any when there are many:
    /// each reader takes a moment to notice, and they notice together.
    pub fn stop_reading(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
    }

    /// Let go of the running program without ending it, for another
    /// terminal to carry on with ([`Terminal::adopt`]): the program does not
    /// notice. This terminal is done after that, and dropping it ends
    /// nothing. `None` when there is no running program to give, or its
    /// reader would not stop; the terminal is then as it was, except that it
    /// reads no more.
    ///
    /// For an application that rebuilds its interface while it runs. The
    /// program's exit status goes to the terminal that started it, so one
    /// that adopted it may only learn that it ended.
    pub fn release(&mut self) -> Option<Released> {
        if self.shared.has_exited() || self.shared.child() <= 1 {
            return None;
        }
        self.master.as_ref()?;
        self.stop_reading();
        // The reader looks up from an idle pseudo-terminal every tenth of a
        // second; until it has, a byte could still go to this emulator.
        let deadline = Instant::now() + Duration::from_secs(1);
        while !self.shared.drained.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if self.shared.has_exited() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::fd::IntoRawFd as _;
            let master = self.master.take()?;
            self.shared.disown();
            let size = *self.shared.size.lock();
            Some(Released {
                master: master.into_raw_fd(),
                pid: self.shared.child(),
                columns: size.columns,
                lines: size.lines,
                cell_width: size.cell_width,
                cell_height: size.cell_height,
                screen: dump(&self.vt.lock()),
                title: self.title.clone(),
                cwd: self.cwd.clone(),
            })
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Carry on with a program another terminal let go of: the same
    /// pseudo-terminal, the screen drawn as it was, the program none the
    /// wiser. On failure the program is ended, as nobody holds it then.
    pub fn adopt(released: Released, cx: &mut App) -> anyhow::Result<Entity<Self>> {
        #[cfg(unix)]
        {
            let (title, cwd) = (released.title.clone(), released.cwd.clone());
            match Started::adopted(&released) {
                Ok(started) => Ok(cx.new(|cx| {
                    let mut terminal = Self::attach(started, cx);
                    terminal.title = title;
                    terminal.cwd = cwd;
                    terminal
                })),
                Err(error) => {
                    // The descriptor went into the attempt; the program is
                    // still there to end.
                    if released.pid > 1 {
                        // SAFETY: plain signal delivery to that process group.
                        unsafe { libc::killpg(released.pid as libc::pid_t, libc::SIGHUP) };
                    }
                    Err(error)
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = cx;
            released.discard();
            anyhow::bail!("kou-term cannot adopt a program on this platform")
        }
    }

    /// Connects a running child to the interface thread.
    fn attach(started: Started, cx: &mut Context<Self>) -> Self {
        let Started {
            vt,
            shared,
            input,
            master,
            internal,
            woken,
        } = started;
        let events_task = cx.spawn(async move |this, cx| {
            while let Ok(event) = internal.recv().await {
                if this
                    .update(cx, |this, cx| this.handle_internal(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let frame = shared.clone();
        let wakeup_task = cx.spawn(async move |this, cx| {
            while woken.recv().await.is_ok() {
                let notified = this.update(cx, |_, cx| {
                    cx.emit(TerminalEvent::Wakeup);
                    cx.notify();
                });
                if notified.is_err() {
                    break;
                }
                // Whatever arrives during this pause collapses into the one wakeup
                // waiting in the channel.
                let interval = Duration::from_micros(frame.frame_micros.load(Ordering::Relaxed));
                cx.background_executor().timer(interval).await;
            }
        });
        Self {
            vt,
            shared,
            input,
            master,
            title: None,
            cwd: None,
            exit: None,
            finished: 0,
            _tasks: [events_task, wakeup_task],
        }
    }

    fn handle_internal(&mut self, event: Internal, cx: &mut Context<Self>) {
        match event {
            Internal::Title(title) => {
                self.title = title.clone();
                cx.emit(TerminalEvent::Title(title.unwrap_or_default()));
            }
            Internal::Bell => cx.emit(TerminalEvent::Bell),
            Internal::Clipboard(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Internal::Osc(OscEvent::Notification { title, body }) => {
                cx.emit(TerminalEvent::Notification { title, body });
            }
            Internal::Osc(OscEvent::Progress(progress)) => {
                cx.emit(TerminalEvent::Progress(progress));
            }
            Internal::Osc(OscEvent::Cwd(path)) => {
                if self.cwd.as_ref() != Some(&path) {
                    self.cwd = Some(path.clone());
                    cx.emit(TerminalEvent::Cwd(path));
                }
            }
            Internal::Osc(OscEvent::CommandFinished) => {
                self.finished += 1;
                // Whoever waits for the command looks again now.
                cx.emit(TerminalEvent::Wakeup);
            }
            Internal::Exited => {
                if self.exit.is_none() {
                    let status = self.shared.exit.lock().and_then(|exit| exit.status());
                    self.exit = Some(status);
                    cx.emit(TerminalEvent::Exited(status));
                    cx.notify();
                }
            }
        }
    }

    /// Writes raw bytes to the program, as if typed.
    pub fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() || self.shared.has_exited() {
            return;
        }
        let _ = self.input.send(Input::Bytes(bytes.to_vec()));
    }

    /// Whether the program asked for pasted text to be bracketed. A shell
    /// that edits its own command line (zsh, fish, bash 5) asks while it waits
    /// at its prompt and takes it back while a command runs, so for a shell
    /// this says "at the prompt".
    pub fn takes_bracketed_paste(&self) -> bool {
        self.modes().bracketed_paste
    }

    /// Paste text: bracketed when the program asked for it.
    pub fn paste(&self, text: &str) {
        let bracketed = self.modes().bracketed_paste;
        self.write(&paste_bytes(text, bracketed));
    }

    /// Pastes `text` and presses Enter: how another agent sends a message to the program
    /// in this pane. Enter follows after a short pause, so the program sees it as a key
    /// press of its own rather than as the end of the paste.
    pub fn submit(&self, text: &str) {
        if !text.is_empty() {
            self.paste(text);
            let _ = self.input.send(Input::Pause(SUBMIT_DELAY));
        }
        let enter = Keystroke {
            key: "enter".into(),
            ..Default::default()
        };
        if let Some(bytes) = keys::encode(&enter, self.key_mode(true)) {
            self.write(&bytes);
        }
    }

    /// The screen as text, one line per row, with trailing blanks and trailing empty rows
    /// removed. `scrollback` puts the history above the screen first; `lines` keeps only
    /// that many rows from the end. Rows are as displayed: a line the terminal wrapped is
    /// two rows. The result does not depend on where a view is scrolled to, and holds
    /// nothing of the pictures on screen.
    pub fn text(&self, lines: Option<usize>, scrollback: bool) -> String {
        self.vt.lock().text(lines, scrollback)
    }

    /// The title the program set, if it set one.
    pub fn title(&self) -> Option<String> {
        self.title.clone()
    }

    /// The working directory the program last reported (OSC 7), if it reported one.
    pub fn cwd(&self) -> Option<PathBuf> {
        self.cwd.clone()
    }

    /// `Some` once the program has exited; the inner value is its status when known.
    pub fn exit_status(&self) -> Option<Option<i32>> {
        self.exit
    }

    /// How the program ended, once it has: its exit code or the signal that ended it,
    /// and how long it ran. Set by the time [`TerminalEvent::Exited`] arrives.
    pub fn exit(&self) -> Option<Exit> {
        *self.shared.exit.lock()
    }

    /// How many commands a shell in it has said are over (OSC 133;D): none
    /// for a program that marks nothing. A shell that marks its prompts
    /// says so once before each prompt, the first included.
    pub fn commands_finished(&self) -> u64 {
        self.finished
    }

    /// When the program last wrote anything. For telling a busy program from an idle one.
    pub fn last_output(&self) -> Option<Instant> {
        match self.shared.last_output.load(Ordering::Relaxed) {
            0 => None,
            nanos => Some(self.shared.started + Duration::from_nanos(nanos - 1)),
        }
    }

    /// How many bytes the program has written so far.
    pub fn bytes_received(&self) -> u64 {
        self.shared.bytes_received.load(Ordering::Relaxed)
    }

    /// The child's process identifier, which is also its process group.
    pub fn pid(&self) -> Option<u32> {
        match self.shared.child.load(Ordering::Acquire) {
            0 => None,
            pid => Some(pid),
        }
    }

    /// The grid size as `(columns, lines)`.
    pub fn size(&self) -> (usize, usize) {
        self.vt.lock().size()
    }

    /// Resizes the grid and tells the program. A view does this from its own measurements;
    /// call it directly only for a terminal no view is showing.
    pub fn resize(&self, columns: usize, lines: usize) {
        let (cell_width, cell_height) = {
            let size = self.shared.size.lock();
            (size.cell_width, size.cell_height)
        };
        self.resize_cells(columns, lines, cell_width, cell_height);
    }

    /// Resizes the grid, recording the size of a cell in device pixels for pictures and
    /// for programs that ask. Does nothing when nothing changed, so the program is not
    /// signalled for nothing.
    pub(crate) fn resize_cells(
        &self,
        columns: usize,
        lines: usize,
        cell_width: u16,
        cell_height: u16,
    ) {
        let size = GridPixels {
            columns: columns.clamp(2, u16::MAX as usize) as u16,
            lines: lines.clamp(1, u16::MAX as usize) as u16,
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
        };
        {
            let mut current = self.shared.size.lock();
            if *current == size {
                return;
            }
            *current = size;
        }
        self.vt.lock().resize(
            size.columns,
            size.lines,
            u32::from(size.cell_width),
            u32::from(size.cell_height),
        );
        if let Some(master) = &self.master {
            set_window_size(master, size);
        }
    }

    /// The size the program was last told, with the cell size in device pixels.
    pub(crate) fn grid_pixels(&self) -> GridPixels {
        *self.shared.size.lock()
    }

    /// Ends the program: hangs up its process group, then kills it if it has not gone
    /// within a second. Returns at once; [`TerminalEvent::Exited`] follows.
    pub fn kill(&self) {
        hang_up(&self.shared);
    }

    /// The emulator, locked. Hold it briefly: the reader thread parses under this lock.
    pub(crate) fn lock(&self) -> MutexGuard<'_, Vt> {
        self.vt.lock()
    }

    /// The modes that decide how input is encoded.
    pub(crate) fn modes(&self) -> Modes {
        self.vt.lock().modes()
    }

    /// What decides how keys are encoded right now.
    pub(crate) fn key_mode(&self, alt_is_meta: bool) -> KeyMode {
        let modes = self.modes();
        KeyMode {
            app_cursor: modes.app_cursor,
            kitty: modes.kitty_keyboard != 0,
            kitty_all_keys: modes.kitty_keyboard & 8 != 0,
            alt_is_meta,
        }
    }

    /// The colours a program sees as the defaults and gets when it asks.
    pub(crate) fn set_style(&self, style: &TerminalStyle) {
        self.vt.lock().set_colors(style);
    }

    /// How often the screen can usefully be redrawn: the display's refresh interval.
    pub(crate) fn set_frame_interval(&self, interval: Duration) {
        let micros = interval.as_micros().clamp(2_000, 100_000) as u64;
        self.shared.frame_micros.store(micros, Ordering::Relaxed);
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // A pane that goes away takes its program with it.
        hang_up(&self.shared);
        self.shared.shutdown.store(true, Ordering::Release);
    }
}

/// The bytes a paste sends.
pub(crate) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        // An escape in the text could close the bracket early and smuggle in commands;
        // an interrupt would cancel the paste in a cooked-mode program.
        let text = text.replace(['\x1b', '\x03'], "");
        let mut bytes = Vec::with_capacity(text.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(unix)]
fn start_child(
    spawn: &Spawn,
    size: GridPixels,
    vt: Arc<Mutex<Vt>>,
    shared: Arc<Shared>,
    events: async_channel::Sender<Internal>,
    wakeups: async_channel::Sender<()>,
    input: mpsc::Receiver<Input>,
) -> anyhow::Result<Option<OwnedFd>> {
    crate::pty::start(spawn, size, vt, shared, events, wakeups, input).map(Some)
}

#[cfg(not(unix))]
fn start_child(
    _spawn: &Spawn,
    _size: GridPixels,
    _vt: Arc<Mutex<Vt>>,
    _shared: Arc<Shared>,
    _events: async_channel::Sender<Internal>,
    _wakeups: async_channel::Sender<()>,
    _input: mpsc::Receiver<Input>,
) -> anyhow::Result<Option<OwnedFd>> {
    anyhow::bail!("kou-term has no pseudo-terminal backend for this platform yet")
}

#[cfg(unix)]
fn hang_up(shared: &Arc<Shared>) {
    crate::pty::hang_up(shared);
}

#[cfg(not(unix))]
fn hang_up(_shared: &Arc<Shared>) {}

#[cfg(unix)]
fn set_window_size(master: &OwnedFd, size: GridPixels) {
    crate::pty::set_window_size(master, size);
}

#[cfg(not(unix))]
fn set_window_size(_master: &OwnedFd, _size: GridPixels) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracketed_paste_wraps_and_filters() {
        assert_eq!(
            paste_bytes("two\nlines", true),
            b"\x1b[200~two\nlines\x1b[201~"
        );
        assert_eq!(
            paste_bytes("evil\x1b[201~rm -rf\x03", true),
            b"\x1b[200~evil[201~rm -rf\x1b[201~"
        );
    }

    #[test]
    fn plain_paste_types_carriage_returns() {
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc");
    }

    #[test]
    fn an_exit_reports_its_status_like_a_shell() {
        let ran_for = Duration::from_millis(5);
        let code = Exit {
            code: Some(3),
            signal: None,
            ran_for,
        };
        assert_eq!(code.status(), Some(3));
        assert!(!code.succeeded());
        let killed = Exit {
            code: None,
            signal: Some(9),
            ran_for,
        };
        assert_eq!(killed.status(), Some(137));
    }
}
