mod accounts;
mod appearance;
// (Under `script/dev` the tool that runs the app makes its bundle.)
#[cfg(all(target_os = "macos", not(feature = "hot")))]
mod bundle;
mod chat_faces;
#[cfg(target_os = "macos")]
mod finder_env;
mod foreground;
mod hot;
mod integration;
mod notify;
#[cfg(any(test, feature = "hot"))]
mod outline;
mod panes;
mod plot;
mod plugin_view;
mod plugins;
mod rail;
mod scenes;
mod screens;
mod shell;
mod state;
mod surfaces;
mod usage;
mod wires;
mod workspace;

use gpui::{
    AnyView, AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, IntoElement, Render,
    TitlebarOptions, Window, WindowBounds, WindowOptions, point, px, size,
};
use shell::Shell;
use state::Store;
use std::{path::PathBuf, time::Duration};

const USAGE: &str = "\
kou-desktop                      open the app
kou-desktop --open DIR           open the app with DIR as a space, added if it is new
kou-desktop mcp                             the app's tools for an agent in a pane, as an MCP server on stdio
kou-desktop --layout panes|canvas
                                 show the open session that way
kou-desktop --theme-link URL      preview a shared kou-theme:// theme
kou-desktop --home DIR           keep its state in DIR instead of the user's data directory
kou-desktop --hidden             run without showing the window (scripted checks)
kou-desktop --capture FILE --after MS
                                 write a picture of the window after MS milliseconds, then quit
kou-desktop --scene NAME [--time MS] [--light] [--width PX] [--height PX] [--capture FILE]
                                 show a scripted state of the interface
            ... --capture FILE --strip N [--every MS]
                                 N pictures MS apart after the scene's action: FILE-0.png, ...
";

struct Args {
    capture: Option<PathBuf>,
    scene: Option<String>,
    home: Option<PathBuf>,
    open: Option<PathBuf>,
    layout: Option<String>,
    theme_link: Option<String>,
    strip: usize,
    every: u64,
    time: f32,
    after: u64,
    light: bool,
    hidden: bool,
    width: f32,
    height: f32,
}

fn args(args: &[String]) -> Args {
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let number = |name: &str, default: f32| {
        value(name)
            .and_then(|text| text.parse().ok())
            .unwrap_or(default)
    };
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    Args {
        capture: value("--capture").map(PathBuf::from),
        scene: value("--scene"),
        home: value("--home").map(PathBuf::from),
        open: value("--open").map(PathBuf::from),
        layout: value("--layout"),
        theme_link: value("--theme-link").or_else(|| {
            args.iter()
                .find(|arg| arg.starts_with("kou-theme://"))
                .cloned()
        }),
        strip: number("--strip", 0.) as usize,
        every: number("--every", 24.) as u64,
        time: number("--time", 0.),
        after: number("--after", 600.) as u64,
        light: flag("--light"),
        hidden: flag("--hidden"),
        width: number("--width", 1280.),
        height: number("--height", 800.),
    }
}

/// The window's root while a scene is shown.
struct Stage(AnyView);

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

async fn capture(window: AnyWindowHandle, path: PathBuf, cx: &mut AsyncApp) {
    let image = cx.update_window(window, |_, window, cx| {
        // Twice: a view shown for the first time measures itself on one frame
        // and lays out on the next.
        for _ in 0..2 {
            window.refresh();
            window.draw(cx).clear(cx);
        }
        window.render_to_image()
    });
    match image {
        Ok(Ok(image)) => {
            image.save(&path).expect("write the capture");
            eprintln!("saved {}", path.display());
        }
        other => {
            eprintln!("capture failed: {:?}", other.map(|image| image.map(|_| ())));
            std::process::exit(2);
        }
    }
}

/// The name a Linux desktop knows the app by: what its windows call
/// themselves, and so which desktop entry and which icon are theirs (the
/// files in `resources/linux`, which `script/icons` names the same).
const APP_ID: &str = "kou-desktop";

/// The app's icon for a window to carry. Only X11 asks a window for one:
/// elsewhere it comes with the program, its bundle or its desktop entry.
fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    if !cfg!(any(target_os = "linux", target_os = "freebsd")) {
        return None;
    }
    let png = include_bytes!("../resources/linux/icons/hicolor/128x128/apps/kou-desktop.png");
    let picture = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    Some(std::sync::Arc::new(picture.into_rgba8()))
}

fn window_options(args: &Args, cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(args.width), px(args.height)), cx);
    let visible = args.capture.is_none() && !args.hidden;
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // The tab strip and the rail's top are the title bar.
        titlebar: Some(TitlebarOptions {
            title: Some("kou".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.), px(12.))),
        }),
        window_min_size: Some(size(px(720.), px(440.))),
        app_id: Some(APP_ID.into()),
        icon: window_icon(),
        show: visible,
        focus: visible,
        ..Default::default()
    }
}

fn run_scene(name: String, args: Args, cx: &mut App) {
    // A capture is compared with earlier ones: not the system's theme of the day.
    cx.set_global(kou_ui::Theme::new(!args.light));
    let Some(scene) = scenes::build(&name, cx) else {
        eprintln!(
            "no scene named {name}; one of: {}",
            scenes::NAMES.join(", ")
        );
        std::process::exit(2);
    };
    let view = scene.view.clone();
    let window: AnyWindowHandle = cx
        .open_window(window_options(&args, cx), move |_, cx| {
            cx.new(|_| Stage(view))
        })
        .expect("open the window")
        .into();
    let (act, describe, time, path) = (scene.act, scene.describe, args.time, args.capture);
    let (strip, every) = (args.strip, args.every);
    cx.spawn(async move |cx| {
        let executor = cx.background_executor().clone();
        // The first frame measures; the second draws everything in place.
        executor.timer(Duration::from_millis(250)).await;
        let _ = cx.update_window(window, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
            act(time, window, cx);
        });
        let Some(path) = path else {
            return;
        };
        // A strip: frames a few milliseconds apart, to see what moves on its way.
        for frame in 0..strip {
            executor.timer(Duration::from_millis(every)).await;
            let name = format!(
                "{}-{frame}.png",
                path.file_stem().unwrap_or_default().to_string_lossy()
            );
            capture(window, path.with_file_name(name), cx).await;
        }
        if strip > 0 {
            cx.update(|cx| cx.quit());
            return;
        }
        executor.timer(Duration::from_millis(150)).await;
        capture(window, path, cx).await;
        cx.update(|cx| {
            for line in describe(cx) {
                eprintln!("{line}");
            }
            cx.quit();
        });
    })
    .detach();
}

/// What the window's content is built from: at launch, and again when a
/// patch changed the shape of what was alive (see `hot.rs`).
struct Mount {
    store: Store,
    /// Where the store is saved; `None` saves nowhere.
    path: Option<PathBuf>,
    home: PathBuf,
    /// Why the store could not be read, to say so in the window.
    problem: Option<String>,
    /// A directory to add as a space and show.
    open: Option<PathBuf>,
    /// How the tab in front and new tabs are shown, when the command line said.
    layout: Option<state::Layout>,
    settings_open: bool,
    usage_open: bool,
    idle_open: bool,
    /// Whether the window answers agents on its socket and looks for
    /// harnesses. Off where nothing outside the process may be touched.
    live: bool,
    /// Whether the user is told of agents that finish or ask: not by a
    /// window nobody can see, which is a check being run.
    notifies: bool,
}

/// The window's content.
fn mount(mount: Mount, window: &mut Window, cx: &mut Context<Shell>) -> Shell {
    let Mount {
        store,
        path,
        home,
        problem,
        open,
        layout,
        settings_open,
        usage_open,
        idle_open,
        live,
        notifies,
    } = mount;
    let mut shell = Shell::new(store, path, cx);
    // The palette's accent gives way to the one the user chose.
    let dark = cx
        .try_global::<kou_ui::Theme>()
        .is_none_or(|theme| theme.dark);
    cx.set_global(shell.themed(dark));
    shell.home = home;
    shell.notifies = notifies;
    shell.probes = notifies;
    if notifies && let Some(listening) = notify::prepare() {
        shell.notice_tell = Some(listening.tell);
        let heard = listening.heard;
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(heard) = heard.recv().await {
                let told = this.update_in(cx, |shell, window, cx| shell.heard(heard, window, cx));
                if told.is_err() {
                    break;
                }
            }
        })
        .detach();
        // Back in the window, perhaps from the system's settings: what it
        // allows may have changed there.
        cx.observe_window_activation(window, |shell: &mut Shell, window, _| {
            if window.is_window_active() {
                shell.look_at_notice_permission();
            }
        })
        .detach();
    }
    shell.notice = problem.map(Into::into);
    shell.settings_open = settings_open;
    if live {
        // Its plugins are read from disk, and again as they change.
        shell.watch_plugins(cx);
    }
    if usage_open && !settings_open {
        shell.open_usage(cx);
    }
    shell.idle_open = idle_open;
    cx.observe_window_appearance(window, |shell: &mut Shell, window, cx| {
        let dark = kou_ui::Theme::for_appearance(window.appearance()).dark;
        cx.set_global(shell.themed(dark));
        shell.retheme(cx);
    })
    .detach();
    if live {
        match kou_ipc::Server::bind(&shell.socket) {
            Ok((server, requests)) => {
                shell.server = Some(server);
                // The socket is this window's: so are the Claude Codes kept
                // beside it. What was kept for panes that are no more is
                // let go.
                shell.keepers = Some(Shell::keepers_beside(&shell.socket));
                shell.sweep_keepers();
                // And what is kept for a pane that is still there is taken
                // up in it, without being asked.
                shell.revive_kept();
                if !shell.revive.is_empty() {
                    // Not left to the next frame alone: a window that is
                    // not being looked at draws none, and what was kept is
                    // to be taken up all the same, as soon as the harnesses
                    // have been found.
                    cx.spawn_in(window, async move |this, cx| {
                        for _ in 0..240 {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(250))
                                .await;
                            let waiting = this.update_in(cx, |shell, window, cx| {
                                shell.revive_ready(window, cx);
                                !shell.revive.is_empty()
                            });
                            if !matches!(waiting, Ok(true)) {
                                break;
                            }
                        }
                    })
                    .detach();
                }
                cx.spawn_in(window, async move |this, cx| {
                    while let Ok(request) = requests.recv().await {
                        // A request is taken apart by the handler, so it
                        // is handed over in a cell the handler empties.
                        let mut request = Some(request);
                        let handled = this.update_in(cx, |shell, window, cx| {
                            hot::hot!(if let Some(request) = request.take() {
                                shell.handle(request, window, cx)
                            });
                        });
                        if handled.is_err() {
                            break;
                        }
                    }
                })
                .detach();
            }
            Err(error) => {
                shell.notice.get_or_insert_with(|| {
                    format!("Agents cannot reach this window: {error:#}").into()
                });
            }
        }
        match kept_program(&shell.home) {
            Ok(bin) => shell.bin = Some(bin),
            Err(error) => log::warn!("the app's program was not copied for the panes: {error}"),
        }
        match keeper_program(&shell.home) {
            Ok(bin) => shell.keeper_bin = Some(bin),
            Err(error) => log::warn!("the app's program was not copied for its keepers: {error}"),
        }
        // What an agent started in a pane is given to work with the others:
        // written once under the app's home, named after its content.
        match kou_mcp::write_bundle(&shell.home.join("orchestration")) {
            Ok(bundle) => shell.guidance = Some(bundle),
            Err(error) => log::warn!("the orchestration guidance was not written: {error}"),
        }
        // A window nobody is looking at stays silent.
        shell.scan(cx);
    }
    if let Some(directory) = open.and_then(|directory| directory.canonicalize().ok()) {
        let space = shell.add_space_at(directory, cx);
        let session = shell
            .store
            .sessions_of(&space, false)
            .last()
            .map(|s| s.id.clone());
        if let Some(session) = session {
            shell.select_session(&session, cx);
        }
    }
    // The tab in front, and every tab made in this run.
    if let Some(layout) = layout {
        shell.new_tabs = Some(layout);
        shell.set_layout(layout, cx);
    }
    shell
}

/// A copy of the running program under the app's home, for panes to call:
/// `<home>/bin/kou-desktop`. What runs in a pane calls the app on every hook
/// and every command, for as long as it lives; the program the app was
/// started from may be a build output that is replaced or removed meanwhile,
/// and every such call would then fail. The copy is replaced in one step
/// when the program has changed, so nothing ever finds half of it.
fn kept_program(home: &std::path::Path) -> std::io::Result<PathBuf> {
    let program = std::env::current_exe()?;
    let kept = home.join("bin").join("kou-desktop");
    if program == kept {
        return Ok(kept);
    }
    let stale = match (std::fs::metadata(&program), std::fs::metadata(&kept)) {
        (Ok(program), Ok(kept)) => {
            program.len() != kept.len()
                || match (program.modified(), kept.modified()) {
                    (Ok(program), Ok(kept)) => program > kept,
                    _ => true,
                }
        }
        _ => true,
    };
    if stale {
        std::fs::create_dir_all(home.join("bin"))?;
        let draft = home
            .join("bin")
            .join(format!("kou-desktop.{}", std::process::id()));
        std::fs::copy(&program, &draft)?;
        std::fs::rename(&draft, &kept)?;
    }
    Ok(kept)
}

/// A copy of the running program under the app's home for the keepers of
/// its Claude Codes to run from: `<home>/bin/claude-host-<size>-<time>`,
/// this build's alone. `bin/kou-desktop` will not do: it is whichever build
/// of the app last started under this home, and an older one (one that knows
/// no keepers) would leave each keeper started from it unable to start.
/// Copies of other builds are removed: a keeper that runs from one goes on
/// (a file removed stays for whoever has it open), and nothing new is
/// started from them.
fn keeper_program(home: &std::path::Path) -> std::io::Result<PathBuf> {
    keeper_copy(&std::env::current_exe()?, &home.join("bin"))
}

fn keeper_copy(program: &std::path::Path, folder: &std::path::Path) -> std::io::Result<PathBuf> {
    let metadata = std::fs::metadata(program)?;
    let time = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let name = format!("claude-host-{}-{time}", metadata.len());
    let kept = folder.join(&name);
    if !kept.exists() {
        std::fs::create_dir_all(folder)?;
        let draft = folder.join(format!(".{name}.{}", std::process::id()));
        std::fs::copy(program, &draft)?;
        std::fs::rename(&draft, &kept)?;
    }
    for entry in std::fs::read_dir(folder)?.flatten() {
        let other = entry.file_name();
        let other = other.to_string_lossy();
        if other.starts_with("claude-host-") && other != name {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(kept)
}

/// What stands in the window between its content being taken down and put
/// up again: nothing, for no longer than it takes to apply a patch.
#[cfg_attr(not(any(test, feature = "hot")), allow(dead_code))]
struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

/// Take the windows' content down, keeping the windows: each is left with
/// nothing in it, and what its content needs to be put up again is returned
/// as plain data. What the content held (its socket, what ran in the panes,
/// the canvases) is let go here and now; the views themselves go with the
/// next frame, which is the last thing that still knows them.
///
/// It is the build that put the content up that has to take it down, since
/// only it knows how the content is laid out; so this is handed on as a
/// value (see [`hot::TakeDown`]) and never called by name across a patch.
#[cfg_attr(not(any(test, feature = "hot")), allow(dead_code))]
fn take_down(cx: &mut App) -> Vec<u8> {
    let mut kept = serde_json::Value::Null;
    for window in cx.windows() {
        let _ = window.update(cx, |root, window, cx| {
            if let Ok(shell) = root.clone().downcast::<Shell>() {
                kept = shell.update(cx, |shell, cx| {
                    let mut kept = shell.kept();
                    // What runs in the panes is let go without being ended,
                    // for the content put up next to carry on with.
                    kept["panes"] = shell.release_panes(cx);
                    shell.let_go();
                    kept
                });
                window.replace_root(cx, |_, _| Blank);
            }
        });
    }
    serde_json::to_vec(&kept).unwrap_or_default()
}

/// Put the windows' content up again from what [`take_down`] kept, as this
/// build makes it. `live` as in [`Mount`]. False when nothing was kept, or no
/// window was waiting for content.
#[cfg_attr(not(any(test, feature = "hot")), allow(dead_code))]
fn put_up(kept: &[u8], live: bool, cx: &mut App) -> bool {
    let Ok(kept) = serde_json::from_slice::<serde_json::Value>(kept) else {
        return false;
    };
    if kept.is_null() {
        return false;
    }
    let read = |key: &str| kept.get(key).cloned().unwrap_or_default();
    // The store goes through what a file from an earlier build goes through:
    // read by this build's rules, then settled.
    let mut store: Store = serde_json::from_value(read("store")).unwrap_or_default();
    store.settle_tabs();
    let mut mounting = Some(Mount {
        store,
        path: serde_json::from_value(read("path")).unwrap_or_default(),
        home: serde_json::from_value(read("home")).unwrap_or_else(|_| state::home()),
        problem: None,
        open: None,
        layout: None,
        settings_open: read("settings_open").as_bool().unwrap_or_default(),
        usage_open: read("usage_open").as_bool().unwrap_or_default(),
        idle_open: read("idle_open").as_bool().unwrap_or_default(),
        live,
        notifies: read("notifies").as_bool().unwrap_or_default(),
    });
    let new_tabs: Option<state::Layout> =
        serde_json::from_value(read("new_tabs")).unwrap_or_default();
    // This build's own keys and colours: both may have changed with it.
    kou_ui::init(cx);
    shell::bind_keys(cx);
    let mut put = false;
    for window in cx.windows() {
        let _ = window.update(cx, |root, window, cx| {
            if root.downcast::<Blank>().is_ok()
                && let Some(mounting) = mounting.take()
            {
                let panes = read("panes");
                window.replace_root(cx, |window, cx| {
                    let mut shell = mount(mounting, window, cx);
                    shell.new_tabs = new_tabs;
                    shell.adopt_panes(&panes, window, cx);
                    shell
                });
                put = true;
            }
        });
    }
    if !put {
        // Nobody to carry on with what was let go: it is ended.
        for (_, released) in panes::released_terminals(&read("panes")) {
            released.discard();
        }
    }
    put
}

fn run_app(args: Args, cx: &mut App) {
    shell::bind_keys(cx);
    let home = args.home.clone().unwrap_or_else(state::home);
    let path = home.join("state.json");
    // A store that cannot be read is left alone: the app starts empty and saves nowhere.
    let (store, path, problem) = match Store::load(&path) {
        Ok(store) => (store, Some(path), None),
        Err(error) => (
            Store::default(),
            None,
            Some(format!(
                "Could not read {}: {error}. Nothing will be saved until it is fixed or removed.",
                path.display()
            )),
        ),
    };
    let layout = match args.layout.as_deref() {
        Some("canvas") => Some(state::Layout::Canvas),
        // "single" and "tiles" were layouts once; both are tabs with splits now.
        Some("panes" | "single" | "tiles") => Some(state::Layout::Panes),
        _ => None,
    };
    let mounting = Mount {
        store,
        path,
        home,
        problem,
        open: args.open.clone(),
        layout,
        settings_open: false,
        usage_open: false,
        idle_open: false,
        live: true,
        // (KOU_NOTIFY_TRACE makes a hidden window post too, to check the
        // system's side of it without putting a window on screen.)
        notifies: (args.capture.is_none() && !args.hidden)
            || std::env::var_os("KOU_NOTIFY_TRACE").is_some(),
    };
    let window = cx
        .open_window(window_options(&args, cx), move |window, cx| {
            cx.new(|cx| mount(mounting, window, cx))
        })
        .expect("open the window");
    if let Some(link) = args.theme_link {
        let _ = window.update(cx, |shell, _, cx| shell.import_theme_link(&link, cx));
    }
    if let Some(path) = args.capture {
        let after = args.after;
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(after))
                .await;
            capture(window.into(), path, cx).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    // The same binary is the keeper of a Claude Code that is to outlive the
    // app (`kou_claude::host`). It turns itself into a process of its own,
    // so this comes before anything else is opened or started here.
    if arguments.first().map(String::as_str) == Some("claude-host") {
        std::process::exit(kou_claude::host::run(&arguments[1..]));
    }
    if arguments.iter().any(|arg| arg == "--help" || arg == "-h") && arguments.len() == 1 {
        print!("{USAGE}\n{}", kou_ipc::cli::USAGE);
        return;
    }
    // The same binary is the MCP server a harness started in a pane is given:
    // the app's tools for an agent, over its standard streams. Nothing else
    // may be written to standard output in this mode.
    if arguments.first().map(String::as_str) == Some("mcp") {
        let code = kou_mcp::serve(
            std::io::stdin().lock(),
            std::io::stdout(),
            &kou_mcp::Env::from_process(),
        );
        std::process::exit(code);
    }
    // The same binary is the command line an agent in a pane talks to the app with.
    if let Some(code) = kou_ipc::cli::run(&arguments) {
        // Not a return: a hook may have left a thread reading a stdin its harness never closes.
        std::process::exit(code);
    }
    #[cfg(target_os = "macos")]
    if let Err(error) = finder_env::prepare() {
        eprintln!("kou-desktop: could not apply the Finder launch environment: {error}");
    }
    let args = args(&arguments);
    // On macOS the app proper runs from a bundle of its own, so that the
    // system knows it by name (see `bundle.rs`). Not a window made for a
    // check, and not under `script/dev`, whose tool makes a bundle itself.
    #[cfg(all(target_os = "macos", not(feature = "hot")))]
    if args.scene.is_none()
        && args.capture.is_none()
        && (!args.hidden || std::env::var_os("KOU_NOTIFY_TRACE").is_some())
    {
        let home = args.home.clone().unwrap_or_else(state::home);
        if let Err(error) = bundle::enter(&home, &arguments) {
            eprintln!("kou-desktop: running outside its bundle, without notifications: {error}");
        }
    }
    let (links, incoming_links) = async_channel::unbounded::<String>();
    let application = gpui_platform::application().with_assets(kou_ui::Assets);
    application.on_open_urls(move |urls| {
        for url in urls
            .into_iter()
            .filter(|url| url.starts_with("kou-theme://"))
        {
            let _ = links.try_send(url);
        }
    });
    application.run(move |cx: &mut App| {
        kou_ui::init(cx);
        #[cfg(feature = "hot")]
        {
            // A patch arrives on the listener's thread and is applied on
            // this one, between two frames: nothing is half-drawn by the
            // old code when the new comes in.
            let patches = hot::listen();
            // The build that put up what is in the window takes it down.
            let mut take: hot::TakeDown = Box::new(take_down);
            let mut outline = hot::outline();
            cx.spawn(async move |cx| {
                while let Ok(patch) = patches.recv().await {
                    // A patch that changed function bodies only goes in
                    // under what is alive. Anything else changed the shape
                    // of it: the window's content is taken down by the
                    // code that made it, and put up again by the new.
                    let now = hot::outline();
                    let reshaped = now != outline;
                    let kept = reshaped.then(|| cx.update(|cx| take(cx)));
                    // Back from that update, what was taken down is gone.
                    let applied = hot::apply(patch);
                    // For whoever runs `script/dev`: what became of the change.
                    eprintln!(
                        "kou-desktop: {}",
                        match (applied, reshaped) {
                            (false, _) => "a patch could not be applied; running as before",
                            (true, false) => "patched",
                            (true, true) => "patched; the window's content was rebuilt",
                        }
                    );
                    cx.update(|cx| {
                        if let Some(kept) = kept {
                            if applied {
                                // This closure is the patch's own from here on.
                                take = subsecond::call(|| -> hot::TakeDown {
                                    hot::started();
                                    put_up(&kept, true, cx);
                                    Box::new(take_down)
                                });
                                outline = now;
                            } else {
                                // Not applied: the code that took it down
                                // is still the code there is.
                                put_up(&kept, true, cx);
                            }
                        }
                        cx.refresh_windows();
                    });
                }
            })
            .detach();
        }
        shell::quit_without_a_window(cx);
        match args.scene.clone() {
            Some(scene) => run_scene(scene, args, cx),
            None => run_app(args, cx),
        }
        cx.spawn(async move |cx| {
            while let Ok(link) = incoming_links.recv().await {
                cx.update(|cx| {
                    for handle in cx.windows() {
                        let opened = handle
                            .update(cx, |root, window, cx| {
                                if let Ok(shell) = root.clone().downcast::<Shell>() {
                                    shell
                                        .update(cx, |shell, cx| shell.import_theme_link(&link, cx));
                                    window.activate_window();
                                    true
                                } else {
                                    false
                                }
                            })
                            .unwrap_or(false);
                        if opened {
                            break;
                        }
                    }
                });
            }
        })
        .detach();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};

    #[test]
    fn theme_links_can_be_opened_as_urls_or_with_an_explicit_option() {
        let link = kou_ui::themes::Preset::White.document().link().unwrap();
        assert_eq!(args(&[link.clone()]).theme_link, Some(link.clone()));
        assert_eq!(
            args(&["--theme-link".into(), link.clone()]).theme_link,
            Some(link)
        );
        assert_eq!(
            args(&["--home".into(), "/tmp/theme-test".into()]).theme_link,
            None
        );
    }

    #[test]
    fn the_keepers_run_from_a_copy_of_this_build_that_no_other_build_replaces() {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("kou-desktop");
        std::fs::write(&program, b"this build").unwrap();
        let folder = root.path().join("bin");
        std::fs::create_dir_all(&folder).unwrap();
        // What another build left: its own copy, and the panes' program.
        std::fs::write(folder.join("claude-host-5-1"), b"older").unwrap();
        std::fs::write(folder.join("kou-desktop"), b"older").unwrap();
        let kept = keeper_copy(&program, &folder).unwrap();
        let name = kept.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("claude-host-10-"), "{name}");
        assert_eq!(std::fs::read(&kept).unwrap(), b"this build");
        assert!(!folder.join("claude-host-5-1").exists());
        assert_eq!(std::fs::read(folder.join("kou-desktop")).unwrap(), b"older");
        // Found again as it is, the next time.
        assert_eq!(keeper_copy(&program, &folder).unwrap(), kept);
        let left: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left.len(), 2, "{left:?}");
    }

    #[gpui::test]
    fn a_windows_content_is_taken_down_and_put_up_again_in_the_same_window(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| cx.set_global(kou_ui::Theme::new(true)));
        let home = tempfile::tempdir().unwrap();
        let mut store = Store::default();
        let space = store.add_space("/fixture/atlas".into());
        let session = store.add_session(&space).unwrap();
        let mounting = Mount {
            store,
            path: Some(home.path().join("state.json")),
            home: home.path().to_owned(),
            problem: None,
            open: None,
            layout: None,
            settings_open: false,
            usage_open: false,
            idle_open: false,
            // Nothing outside the process: no socket, no search for harnesses.
            live: false,
            notifies: false,
        };
        let (shell, cx) = cx.add_window_view(|window, cx| mount(mounting, window, cx));
        let window = cx.update(|window, _| window.window_handle());
        // Something to remember: a renamed space, a screen that was open.
        shell.update(cx, |shell, cx| {
            shell.name_space(&space, "client work", cx);
            shell.settings_open = true;
        });
        // And something alive: a program in a pane, which answers what it is sent.
        cx.executor().allow_parking();
        let (pane, pid) = shell.update_in(cx, |shell, window, cx| {
            shell.harnesses = vec![shell::HarnessInfo {
                id: "shell".into(),
                name: "Shell".into(),
                code: "SH".into(),
                ready: true,
                version: None,
                message: None,
                native: false,
                adapter: None,
            }];
            let pane = shell
                .add_pane(
                    &session,
                    "shell",
                    None,
                    state::PaneKind::Terminal,
                    window,
                    cx,
                )
                .expect("a pane");
            let terminal = kou_term::Terminal::open(
                kou_term::Spawn {
                    program: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        r#"echo ready; while read line; do echo "got $line"; done"#.into(),
                    ],
                    env: Vec::new(),
                    cwd: std::env::temp_dir(),
                },
                cx,
            )
            .expect("a program");
            let saved = shell.store.pane(&pane).unwrap().clone();
            shell.install_terminal(&saved, terminal.clone(), false, None, false, window, cx);
            shell.set_mark(&pane, kou_ui::Mark::Working, window, cx);
            // (A new pane closes the settings; they are opened again.)
            shell.settings_open = true;
            (pane, terminal.read(cx).pid().expect("it runs"))
        });
        // Lets the program, its reader thread and the app run until `done` holds.
        fn wait(
            cx: &mut VisualTestContext,
            what: &str,
            mut done: impl FnMut(&mut VisualTestContext) -> bool,
        ) {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            loop {
                cx.run_until_parked();
                if done(cx) {
                    return;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "timed out waiting for {what}"
                );
                std::thread::sleep(Duration::from_millis(5));
                cx.executor().advance_clock(Duration::from_millis(17));
            }
        }
        let shown = |shell: &gpui::Entity<Shell>, cx: &mut VisualTestContext| {
            shell.read_with(cx, |shell, cx| {
                shell
                    .terminal_of(&pane)
                    .map(|terminal| terminal.read(cx).text(None, false))
                    .unwrap_or_default()
            })
        };
        wait(cx, "the program", |cx| shown(&shell, cx).contains("ready"));
        let old = shell.downgrade();
        drop(shell);

        // Taken down: the window is still there, with nothing in it. What
        // it held outside itself is let go at once; the view goes with the
        // next frame, the last thing that knew it.
        // (From outside the window's own turn, as the patch loop does it.)
        let kept = cx.cx.update(take_down);
        cx.run_until_parked();
        if let Some(old) = old.upgrade() {
            old.read_with(cx, |shell, _| {
                assert!(shell.server.is_none() && shell.running.is_empty());
            });
        }
        let root = |cx: &mut VisualTestContext| {
            window
                .update(cx, |root, _, _| root)
                .expect("the window is open")
        };
        assert!(root(cx).downcast::<Blank>().is_ok());
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        assert!(old.upgrade().is_none(), "the old content was dropped");

        // Put up again: the same window, new content, what it remembered.
        assert!(cx.cx.update(|cx| put_up(&kept, false, cx)));
        cx.run_until_parked();
        let shell = root(cx).downcast::<Shell>().expect("content again");
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.store.space(&space).unwrap().name, "client work");
            assert_eq!(shell.store.selected.as_deref(), Some(session.as_str()));
            assert!(shell.settings_open);
            assert_eq!(shell.home, home.path());
        });
        // The program in the pane is the same one, on a terminal of the new
        // content's making: its screen as it was, what it was doing, and it
        // answers on.
        let terminal = shell
            .read_with(cx, |shell, _| shell.terminal_of(&pane))
            .expect("the pane has its program");
        assert_eq!(
            terminal.read_with(cx, |terminal, _| terminal.pid()),
            Some(pid)
        );
        assert!(shown(&shell, cx).contains("ready"), "{}", shown(&shell, cx));
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.live[&pane].mark),
            kou_ui::Mark::Working
        );
        terminal.read_with(cx, |terminal, _| terminal.submit("hello"));
        wait(cx, "the answer", |cx| {
            shown(&shell, cx).contains("got hello")
        });
        // It saves where it saved: a change after the rebuild reaches the file.
        shell.update(cx, |shell, cx| shell.name_space(&space, "again", cx));
        let saved = std::fs::read_to_string(home.path().join("state.json")).unwrap();
        assert!(saved.contains("\"again\""), "{saved}");

        // Closing its pane ends it, as it would have before.
        shell.update_in(cx, |shell, window, cx| shell.close_pane(&pane, window, cx));
        let gone = std::time::Instant::now() + Duration::from_secs(5);
        // SAFETY: signal 0 delivers nothing; it asks whether the process is there.
        while unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 && std::time::Instant::now() < gone
        {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_ne!(
            unsafe { libc::kill(pid as libc::pid_t, 0) },
            0,
            "the program ended"
        );

        // Nothing kept, nothing put up; and content is not put up twice.
        assert!(!cx.cx.update(|cx| put_up(b"null", false, cx)));
        // (What was kept owns the programs it names, once: the second time
        // it is offered without them.)
        let mut again: serde_json::Value = serde_json::from_slice(&kept).unwrap();
        again.as_object_mut().unwrap().remove("panes");
        let again = serde_json::to_vec(&again).unwrap();
        assert!(!cx.cx.update(|cx| put_up(&again, false, cx)));
    }
}
