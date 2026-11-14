//! Hot patching: working on the app while it runs.
//!
//! Built with the `hot` feature and started by `script/dev` (the Dioxus CLI's
//! `dx serve --hot-patch`), the app takes a change to its code as a patch.
//! The crates that changed are recompiled, linked into a small library and
//! swapped in under the running process, usually within a second or two.
//! Panes, programs and conversations stay as they are; the window is redrawn
//! by the new code.
//!
//! A patch replaces functions, not data, which decides what it can do:
//!
//! - Code reached through [`hot!`](crate::hot) runs in its newest version. The
//!   window's `render` goes through it, so everything that is drawn, and every
//!   listener made while drawing, is new on the next frame. A callback made
//!   before the patch (a subscription, a task) keeps its old body unless that
//!   body goes through `hot!` as well, which the app's own do.
//! - What is alive keeps its shape, so a patch that changes more than
//!   function bodies (a field added to `Shell`, a variant to an enum, a
//!   signature) cannot go in under it: new code would read old memory
//!   wrongly. Such a patch is told from the source (`outline.rs`), and the
//!   window's content is rebuilt around it: taken down by the code that made
//!   it, the patch applied, put up again by the new code from what the old
//!   kept as plain data (`take_down` and `put_up` in `main.rs`). The window
//!   stays open; its content blinks. [`fits`] is the second line: code that
//!   finds the sizes of what is alive other than its own runs as it was built.
//! - A patch gets fresh copies of the statics of the crates it recompiles: a
//!   cache starts empty again, a counter starts over. Nothing that has to
//!   outlive a patch is kept in one.
//! - A new dependency, a changed `Cargo.toml` or a changed build script is not
//!   a patch at all: press `r` in `script/dev` to rebuild and start again.
//!
//! Without the feature all of this compiles to plain calls.

/// Run an expression in its newest version, or as it was built when the
/// running app's state no longer fits the new code. The expression must be
/// one that can be evaluated twice in source (it runs once).
macro_rules! hot {
    ($run:expr) => {{
        #[cfg(feature = "hot")]
        {
            // The closure is what a patch replaces; the arm after it is this
            // build's own code, reached when the patch declines.
            match subsecond::call(|| $crate::hot::fits().then(|| $run)) {
                Some(done) => done,
                None => {
                    $crate::hot::declined();
                    $run
                }
            }
        }
        #[cfg(not(feature = "hot"))]
        {
            $run
        }
    }};
}
pub(crate) use hot;

#[cfg(feature = "hot")]
mod patching {
    use crate::{
        panes::{Running, Talking, Wait},
        shell::{HarnessInfo, Held, Live, Picker, Resuming, Shell},
        state::{Pane, Session, Space, Split, Store, Tab},
    };
    use std::{
        mem::{align_of, size_of},
        sync::atomic::{AtomicBool, Ordering},
    };

    // Where the shape the app was started with is kept: in the process's
    // environment, because a patch gets fresh copies of this crate's statics
    // and would otherwise compare its own shape with itself.
    const STARTED_WITH: &str = "KOU_HOT_SHAPE";
    // Set and read by the build the app was started from only (see `hot!`).
    static STALE: AtomicBool = AtomicBool::new(false);

    // What the app holds while it runs, as the code being compiled lays it
    // out: sizes and alignments, folded into one number. A field added,
    // removed or widened changes it; that is the change a patch cannot take.
    fn shape() -> u64 {
        macro_rules! of {
            ($($held:ty),* $(,)?) => {
                [$((size_of::<$held>() as u64) << 8 | align_of::<$held>() as u64),*]
            };
        }
        of![
            Shell,
            Live,
            Picker,
            Resuming,
            Held,
            HarnessInfo,
            Running,
            Talking,
            Wait,
            Store,
            Space,
            Session,
            Pane,
            Tab,
            Split,
        ]
        .into_iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, part| {
            (hash ^ part).wrapping_mul(0x0000_0100_0000_01b3)
        })
    }

    /// Whether the code calling this can run on the state the app holds.
    pub fn fits() -> bool {
        match std::env::var(STARTED_WITH) {
            Ok(started) => started.parse() == Ok(shape()),
            // Not listening for patches: there is only one build.
            Err(_) => true,
        }
    }

    /// A patch was declined: remember it, to say so in the window.
    pub fn declined() {
        STALE.store(true, Ordering::Relaxed);
    }

    /// Whether a patch was declined: the app runs older code than is on disk.
    pub fn stale() -> bool {
        STALE.load(Ordering::Relaxed)
    }

    /// Takes the windows' content down and returns what it needs to be put
    /// up again. A value, not a function called by name: it is made by the
    /// build that put the content up, and so is that build's code however
    /// many patches have come since.
    pub type TakeDown = Box<dyn FnMut(&mut gpui::App) -> Vec<u8>>;

    /// Say that what is alive from here on has the shape this build gives
    /// it: called by the build that puts the windows' content up.
    pub fn started() {
        // SAFETY: on the main thread, at launch before the app has threads
        // of its own, and later between two frames, where the only readers
        // of the environment are a pane's program being started, which
        // happens on this thread too.
        unsafe { std::env::set_var(STARTED_WITH, shape().to_string()) };
    }

    /// The outline of the workspace's source as it is on disk now (see
    /// `outline.rs`), `None` when it cannot be read: then every patch is
    /// taken for one that changed the shape of things.
    pub fn outline() -> Option<u64> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        crate::outline::outline(&root).ok()
    }

    /// Listen for patches: each is handed over as it arrives, to be applied
    /// with [`apply`] where the app is ready for it. Call it once, first
    /// thing, before the app starts any thread.
    pub fn listen() -> async_channel::Receiver<subsecond::JumpTable> {
        started();
        let (arrived, patches) = async_channel::unbounded();
        dioxus_devtools::connect(move |message| {
            if let dioxus_devtools::DevserverMsg::HotReload(reload) = message
                && let Some(table) = reload.jump_table
                // A patch built for another process of this app is not ours.
                && reload.for_pid == Some(std::process::id())
            {
                let _ = arrived.try_send(table);
            }
        });
        patches
    }

    /// Put a patch in. False when it could not be loaded: the code is then
    /// what it was.
    pub fn apply(patch: subsecond::JumpTable) -> bool {
        // SAFETY: called on the main thread between two frames, with nothing
        // of the app's on the stack that a replaced function could return
        // into differently; what is alive either fits the patch or has been
        // taken down by the caller.
        let applied = unsafe { subsecond::apply_patch(patch) };
        match applied {
            Ok(()) => {
                // Code that fits again clears the notice when it next runs.
                STALE.store(false, Ordering::Relaxed);
                true
            }
            Err(error) => {
                log::error!("a patch could not be applied: {error}");
                false
            }
        }
    }
}

#[cfg(feature = "hot")]
pub(crate) use patching::{TakeDown, apply, declined, fits, listen, outline, stale, started};

/// Whether a patch was declined. Never, in a build that takes none.
#[cfg(not(feature = "hot"))]
pub(crate) fn stale() -> bool {
    false
}
