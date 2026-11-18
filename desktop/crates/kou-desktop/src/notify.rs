//! Telling the user that an agent wants them while they look elsewhere: a
//! notification from the system when an agent they started has finished, and
//! when any agent stops to ask something. Both can be turned off in the
//! settings.
//!
//! On macOS they are the system's own notifications, posted in the app's
//! name: its entry in the system's notification settings, and a click on
//! one brings the app to the front on the pane it is about. That takes the
//! app running from its bundle (see `bundle.rs`); as a bare program it posts
//! nothing, rather than posting in some other program's name. Elsewhere
//! `notify-send` is used when it is there. No notification makes a sound.

use kou_ui::Mark;

/// Why the user is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Occasion {
    /// An agent finished its turn.
    Done,
    /// An agent waits for an answer: a question, a permission.
    Asks,
}

/// Whether a change of an agent's state is worth telling the user about.
/// `watched` when the user is looking at its pane in the active window: then
/// they have seen it. `worker` when another agent started it: its finishing
/// is that agent's business, its questions are still the user's.
pub(crate) fn occasion(
    was: Mark,
    now: Mark,
    watched: bool,
    worker: bool,
    tell_done: bool,
    tell_asks: bool,
) -> Option<Occasion> {
    if watched || was == now {
        return None;
    }
    match (was, now) {
        (_, Mark::Blocked) if tell_asks => Some(Occasion::Asks),
        (Mark::Working, Mark::Idle) if tell_done && !worker => Some(Occasion::Done),
        _ => None,
    }
}

/// The words of a notification: what happened to whom, and where.
pub(crate) fn words(occasion: Occasion, agent: &str, place: &str) -> (String, String) {
    let title = match occasion {
        Occasion::Done => format!("{agent} is done"),
        Occasion::Asks => format!("{agent} needs you"),
    };
    (title, place.to_owned())
}

/// Whether the system lets the app post. The user decides it, in the
/// system's own question the first time the app asks and in the system's
/// settings ever after; a question left unanswered counts as a no.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Permission {
    /// Not known, or not something this system decides.
    #[default]
    Unknown,
    Allowed,
    Refused,
}

/// What the system tells the app about its notifications.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Heard {
    /// One was clicked: the `id` given to [`post`].
    Clicked(String),
    /// Whether the app may post, as the system has it now.
    Permission(Permission),
}

/// Where the system's news arrives, and the end it is sent from.
pub(crate) struct Listening {
    pub heard: async_channel::Receiver<Heard>,
    pub tell: async_channel::Sender<Heard>,
}

/// Get ready to post: on macOS, ask the system once whether the app may,
/// and listen for clicks. Returns where the system's news arrives, or
/// `None` where notifications cannot be clicked or cannot be posted at all.
/// Call it on the main thread.
pub(crate) fn prepare() -> Option<Listening> {
    #[cfg(target_os = "macos")]
    {
        native::prepare()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Ask the system again whether the app may post (the user can change it
/// in the system's settings at any time); the answer arrives as
/// [`Heard::Permission`] through `tell`.
pub(crate) fn look(tell: &async_channel::Sender<Heard>) {
    #[cfg(target_os = "macos")]
    {
        native::look(tell.clone());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = tell;
    }
}

/// Open the system's settings where the app's notifications are allowed.
pub(crate) fn open_system_settings() {
    #[cfg(target_os = "macos")]
    {
        native::open_settings();
    }
}

/// Post a notification; `id` comes back from [`prepare`]'s channel when it
/// is clicked. Returns at once; a system that has no way to show one shows
/// nothing.
pub(crate) fn post(id: &str, title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        native::post(id, title, body);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = id;
        let arguments = vec!["--app-name=kou".to_owned(), one_line(title), one_line(body)];
        // The program is waited for on a thread of its own, so it leaves nothing behind.
        let spawned = std::thread::Builder::new()
            .name("kou notification".into())
            .spawn(move || {
                let _ = std::process::Command::new("notify-send")
                    .args(arguments)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            });
        if let Err(error) = spawned {
            log::warn!("a notification could not be posted: {error}");
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (id, title, body);
    }
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(target_os = "macos")]
mod native {
    use super::{Heard, Listening, Permission};
    use block2::RcBlock;
    use objc2::{
        AllocAnyThread, define_class, msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject, Bool, ProtocolObject},
    };
    use objc2_foundation::{
        NSBundle, NSError, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol,
        NSString,
    };
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
        UNNotification, UNNotificationPresentationOptions, UNNotificationRequest,
        UNNotificationResponse, UNNotificationSettings, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };
    use std::{ffi::CStr, ptr::NonNull};

    // The delegate's class, as the system knows it, and the name a click is
    // passed on under.
    const CLASS: &CStr = c"KouNotificationDelegate";
    const CLICKED: &str = "KouNotificationClicked";

    define_class!(
        // The delegate holds nothing of this program's: a click is passed on
        // through the process's own notification centre, under `CLICKED`,
        // with the notification's id as its object. That lets it outlive the
        // code that made it. While the app is worked on as it runs
        // (`hot.rs`), the window's content is rebuilt by newer code, which
        // cannot register this class a second time (the system refuses a
        // name it has) and must still hear the delegate the first code set.
        //
        // SAFETY: NSObject has no requirements of its subclasses, and the
        // class does not implement `Drop`.
        #[unsafe(super(NSObject))]
        #[name = "KouNotificationDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            // The app is told of a notification instead of the user when it
            // is in front. It is only posted when the user is not looking at
            // the pane it is about, so it is shown all the same.
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                done.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
            }

            // Clicked: which one goes to the app, which shows what it is about.
            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                done: &block2::DynBlock<dyn Fn()>,
            ) {
                let id = response.notification().request().identifier();
                let object: &AnyObject = &id;
                // SAFETY: the object is a string, which is what every
                // listener under this name takes it for.
                unsafe {
                    NSNotificationCenter::defaultCenter()
                        .postNotificationName_object(&NSString::from_str(CLICKED), Some(object));
                }
                done.call(());
            }
        }
    );

    impl Delegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            // SAFETY: NSObject's `init` takes no arguments and returns the object.
            unsafe { msg_send![super(this), init] }
        }
    }

    // Whether the system knows this program as an app: only then is there a
    // notification centre to ask for (asking without one raises an exception).
    fn bundled() -> bool {
        NSBundle::mainBundle().bundleIdentifier().is_some()
    }

    fn traced() -> bool {
        std::env::var_os("KOU_NOTIFY_TRACE").is_some()
    }

    pub(super) fn prepare() -> Option<Listening> {
        if !bundled() {
            return None;
        }
        let (tell, heard) = async_channel::unbounded();
        let center = UNUserNotificationCenter::currentNotificationCenter();
        // Once in the life of the process (see `Delegate`): a class of that
        // name means a delegate is set already, by this code or by the code
        // this one replaced.
        if AnyClass::get(CLASS).is_none() {
            let delegate = Delegate::new();
            center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            // The centre holds its delegate weakly; this one lives as long
            // as the app.
            std::mem::forget(delegate);
        }
        // Clicks reach whoever listens now. A listener left by content that
        // has been replaced tells a channel nobody reads, which costs nothing.
        let clicked = RcBlock::new({
            let tell = tell.clone();
            move |notification: NonNull<NSNotification>| {
                // SAFETY: the system hands over a valid notification for the call.
                let object = unsafe { notification.as_ref() }.object();
                if let Some(id) = object.and_then(|object| object.downcast::<NSString>().ok()) {
                    let _ = tell.try_send(Heard::Clicked(id.to_string()));
                }
            }
        });
        // SAFETY: no object to match and no queue: the block runs where the
        // click is passed on, and sends through a channel made for that.
        let listener = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(&NSString::from_str(CLICKED)),
                None,
                None,
                &clicked,
            )
        };
        std::mem::forget(listener);
        look(tell.clone());
        // `KOU_NOTIFY_TRACE=status` only looks: the user is asked nothing.
        if std::env::var("KOU_NOTIFY_TRACE").as_deref() == Ok("status") {
            return Some(Listening { heard, tell });
        }
        // The system asks the user the first time, and remembers the answer.
        let answered = RcBlock::new({
            let tell = tell.clone();
            move |granted: Bool, error: *mut NSError| {
                // SAFETY: the system hands over a valid error or null.
                let reason =
                    unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string());
                let permission = if granted.as_bool() {
                    Permission::Allowed
                } else {
                    log::warn!("the system does not let the app post notifications: {reason:?}");
                    Permission::Refused
                };
                // For whoever is finding out why none arrive: KOU_NOTIFY_TRACE=1.
                if traced() {
                    eprintln!(
                        "kou-desktop: asked to post notifications: {permission:?} ({reason:?})"
                    );
                }
                let _ = tell.try_send(Heard::Permission(permission));
            }
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert,
            &answered,
        );
        Some(Listening { heard, tell })
    }

    pub(super) fn look(tell: async_channel::Sender<Heard>) {
        if !bundled() {
            return;
        }
        let known = RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
            // SAFETY: the system hands over valid settings for the call.
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let permission = match status {
                UNAuthorizationStatus::Denied => Permission::Refused,
                UNAuthorizationStatus::NotDetermined => Permission::Unknown,
                _ => Permission::Allowed,
            };
            if traced() {
                eprintln!(
                    "kou-desktop: the system's permission to post notifications: {permission:?}"
                );
            }
            let _ = tell.try_send(Heard::Permission(permission));
        });
        UNUserNotificationCenter::currentNotificationCenter()
            .getNotificationSettingsWithCompletionHandler(&known);
    }

    pub(super) fn open_settings() {
        let Some(identifier) = NSBundle::mainBundle().bundleIdentifier() else {
            return;
        };
        let _ = std::process::Command::new("open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={identifier}"
            ))
            .spawn();
    }

    pub(super) fn post(id: &str, title: &str, body: &str) {
        if !bundled() {
            return;
        }
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(title));
        content.setBody(&NSString::from_str(body));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(id),
            &content,
            None,
        );
        let center = UNUserNotificationCenter::currentNotificationCenter();
        if !traced() {
            center.addNotificationRequest_withCompletionHandler(&request, None);
            return;
        }
        let id = id.to_owned();
        let posted = RcBlock::new(move |error: *mut NSError| {
            // SAFETY: the system hands over a valid error or null.
            match unsafe { error.as_ref() } {
                Some(error) => eprintln!(
                    "kou-desktop: notification {id} not posted ({})",
                    error.localizedDescription()
                ),
                None => eprintln!("kou-desktop: notification {id} posted"),
            }
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&posted));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Mark::{Blocked, Done, Failed, Idle, Working};

    #[test]
    fn the_user_is_told_of_what_they_did_not_see_and_asked_to_be_told_of() {
        let tell = |was, now| occasion(was, now, false, false, true, true);
        assert_eq!(tell(Working, Idle), Some(Occasion::Done));
        assert_eq!(tell(Working, Blocked), Some(Occasion::Asks));
        assert_eq!(tell(Idle, Blocked), Some(Occasion::Asks));
        // Nothing happened, or nothing that calls for them.
        for (was, now) in [
            (Idle, Idle),
            (Blocked, Blocked),
            (Idle, Working),
            (Blocked, Working),
            (Blocked, Idle),
            (Working, Failed),
            (Done, Idle),
        ] {
            assert_eq!(tell(was, now), None, "{was:?} to {now:?}");
        }
        // Looking at it is being told.
        assert_eq!(occasion(Working, Idle, true, false, true, true), None);
        assert_eq!(occasion(Working, Blocked, true, false, true, true), None);
        // A worker's end is for the agent that started it; its question is not.
        assert_eq!(occasion(Working, Idle, false, true, true, true), None);
        assert_eq!(
            occasion(Working, Blocked, false, true, true, true),
            Some(Occasion::Asks)
        );
        // Each can be turned off by itself.
        assert_eq!(occasion(Working, Idle, false, false, false, true), None);
        assert_eq!(occasion(Working, Blocked, false, false, true, false), None);
        assert_eq!(
            occasion(Working, Blocked, false, false, false, true),
            Some(Occasion::Asks)
        );
    }

    #[test]
    fn a_notification_says_who_and_where_and_cannot_break_out_of_its_script() {
        assert_eq!(
            words(Occasion::Done, "Claude Code", "migrations · atlas"),
            (
                "Claude Code is done".to_owned(),
                "migrations · atlas".to_owned()
            )
        );
        assert_eq!(words(Occasion::Asks, "Codex", "").0, "Codex needs you");
    }
}
