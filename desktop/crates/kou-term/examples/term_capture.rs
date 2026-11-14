//! Renders a terminal running a program to a PNG without showing a window, or measures how
//! the interface thread fares while a program floods the terminal.
//!
//! ```text
//! cargo run -p kou-term --features capture --example term_capture -- \
//!     --out shot.png [--size 900x520] [--settle 400] [--timeout 8000] [--focused] [--light] \
//!     [--type 'text typed after the first settle'] [--select-all] [--flight 30] \
//!     [--compose 'text an input method is composing'] [--text] [--zoom 0.5] \
//!     -- /bin/sh -c 'ls -la'
//!
//! With `--zoom` the terminal sits in a node of `--size` times the zoom and follows the
//! zoom, the way the canvas shows it: the program gets the grid `--size` holds, and the
//! window, and so the capture, is the zoomed size.
//!
//! cargo run -p kou-term --features capture --example term_capture -- \
//!     --bench -- /bin/cat big-file
//! ```

use gpui::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity, Focusable as _, Hsla,
    IntoElement, ParentElement as _, Render, Styled as _, Window, WindowBounds, WindowOptions, div,
    px, rgb, size,
};
use kou_term::{Spawn, Terminal, TerminalEvent, TerminalStyle, TerminalView};
use std::{
    cell::Cell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

struct Options {
    out: Option<PathBuf>,
    width: f32,
    height: f32,
    settle: Duration,
    timeout: Duration,
    focused: bool,
    light: bool,
    typed: Option<String>,
    select_all: bool,
    composing: Option<String>,
    print_text: bool,
    bench: bool,
    /// Capture this long after the cursor starts moving, to see it in flight.
    flight: Option<Duration>,
    /// Show the terminal the way a zoomed canvas does.
    zoom: Option<f32>,
    /// Draw this many frames while the zoom sweeps from 0.1 to 4, and report their cost.
    zoom_sweep: Option<u32>,
    command: Vec<String>,
}

/// The window's content: the terminal in a node of the base size times the zoom.
struct Stage {
    view: Entity<TerminalView>,
    width: f32,
    height: f32,
    zoom: Rc<Cell<f32>>,
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let zoom = self.zoom.get();
        div().size_full().child(
            div()
                .w(px(self.width * zoom))
                .h(px(self.height * zoom))
                .child(self.view.clone()),
        )
    }
}

fn parse_options() -> Options {
    let mut options = Options {
        out: None,
        width: 900.,
        height: 520.,
        settle: Duration::from_millis(400),
        timeout: Duration::from_secs(8),
        focused: false,
        light: false,
        typed: None,
        select_all: false,
        composing: None,
        print_text: false,
        bench: false,
        flight: None,
        zoom: None,
        zoom_sweep: None,
        command: Vec::new(),
    };
    let mut arguments = std::env::args().skip(1);
    let millis = |value: Option<String>, flag: &str| {
        let value = value.unwrap_or_else(|| panic!("{flag} needs a number of milliseconds"));
        Duration::from_millis(
            value
                .parse()
                .unwrap_or_else(|_| panic!("{flag}: not a number")),
        )
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--out" => options.out = arguments.next().map(PathBuf::from),
            "--size" => {
                let value = arguments.next().expect("--size needs WIDTHxHEIGHT");
                let (width, height) = value.split_once('x').expect("--size needs WIDTHxHEIGHT");
                options.width = width.parse().expect("--size: width is not a number");
                options.height = height.parse().expect("--size: height is not a number");
            }
            "--settle" => options.settle = millis(arguments.next(), "--settle"),
            "--timeout" => options.timeout = millis(arguments.next(), "--timeout"),
            "--focused" => options.focused = true,
            "--light" => options.light = true,
            "--type" => options.typed = arguments.next(),
            "--select-all" => options.select_all = true,
            "--compose" => options.composing = arguments.next(),
            "--text" => options.print_text = true,
            "--bench" => options.bench = true,
            "--flight" => options.flight = Some(millis(arguments.next(), "--flight")),
            "--zoom-sweep" => {
                let value = arguments.next().expect("--zoom-sweep needs a frame count");
                options.zoom_sweep = Some(value.parse().expect("--zoom-sweep: not a number"));
            }
            "--zoom" => {
                let value = arguments.next().expect("--zoom needs a factor");
                options.zoom = Some(value.parse().expect("--zoom: not a number"));
            }
            "--" => {
                options.command = arguments.collect();
                break;
            }
            other => panic!("unknown argument {other}"),
        }
    }
    assert!(
        !options.command.is_empty(),
        "give the program to run after --"
    );
    assert!(
        options.bench || options.zoom_sweep.is_some() || options.out.is_some(),
        "give --out FILE.png, --bench or --zoom-sweep"
    );
    options
}

/// The style the captures are judged in.
fn capture_style(light: bool) -> TerminalStyle {
    // The app's dark theme (`kou_ui::DARK`, `kou_ui::ANSI_DARK`), as the default is.
    let dark = TerminalStyle::default();
    if !light {
        return dark;
    }
    // The app's light theme (`kou_ui::LIGHT`, `kou_ui::ANSI_LIGHT`), to see that faint
    // text, the cursor and the selection are derived from the style and not from an
    // assumption that the background is dark.
    let ansi = [
        0x171717, 0xc0273d, 0x2a7d36, 0x8a5a00, 0x2757c4, 0x8a3fc0, 0x157a80, 0x6b6b6b, 0x454545,
        0xd9304a, 0x35913f, 0xa66d00, 0x3a6be0, 0xa052d8, 0x1c9198, 0x828282,
    ]
    .map(|hex| Hsla::from(rgb(hex)));
    TerminalStyle {
        foreground: rgb(0x0d0d0d).into(),
        background: rgb(0xffffff).into(),
        cursor: rgb(0x0d0d0d).into(),
        selection: Hsla::from(rgb(0xa83205)).opacity(0.2),
        ansi,
        ..dark
    }
}

fn main() {
    let mut options = parse_options();
    gpui_platform::application().run(move |cx: &mut App| {
        let spawn = Spawn {
            program: PathBuf::from(&options.command[0]),
            args: options.command[1..].to_vec(),
            env: Vec::new(),
            cwd: std::env::current_dir().expect("a current directory"),
        };
        let terminal = Terminal::open(spawn, cx).expect("start the program");
        if options.zoom_sweep.is_some() && options.zoom.is_none() {
            options.zoom = Some(1.);
        }
        let zoom = options.zoom.unwrap_or(1.);
        let handle = Rc::new(Cell::new(zoom));
        let bounds = Bounds::centered(
            None,
            size(px(options.width * zoom), px(options.height * zoom)),
            cx,
        );
        let focused = options.focused;
        let light = options.light;
        let follow = options.zoom.is_some();
        let (width, height) = (options.width, options.height);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    show: false,
                    focus: false,
                    ..Default::default()
                },
                |window, cx| {
                    let terminal = terminal.clone();
                    let view = cx.new(|cx| {
                        let mut view =
                            TerminalView::new(terminal, capture_style(light), window, cx);
                        if focused {
                            view.force_focused(Some(true));
                        }
                        if follow {
                            view.follow_zoom(Some(handle.clone()), cx);
                        }
                        view
                    });
                    cx.new(|_| Stage {
                        view,
                        width,
                        height,
                        zoom: handle.clone(),
                    })
                },
            )
            .expect("open the capture window");
        let stage = window.root(cx).expect("the window's view");
        let view = stage.read(cx).view.clone();
        let window: AnyWindowHandle = window.into();
        // A first frame gives the program its real size before it has drawn much.
        cx.update_window(window, |_, window, cx| {
            if focused {
                let handle = view.focus_handle(cx);
                window.focus(&handle, cx);
            }
            window.draw(cx).clear(cx);
        })
        .expect("draw the first frame");

        cx.spawn(async move |cx| {
            if let Some(frames) = options.zoom_sweep {
                settle(&terminal, options.settle, options.timeout, cx).await;
                zoom_sweep(frames, &stage, window, cx);
            } else if options.bench {
                bench(&terminal, window, cx).await;
            } else {
                capture(&options, &terminal, &view, window, cx).await;
            }
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

/// Waits until the program has been quiet for `settle`, or has exited, or `timeout` passed.
async fn settle(
    terminal: &Entity<Terminal>,
    settle: Duration,
    timeout: Duration,
    cx: &mut AsyncApp,
) {
    let started = Instant::now();
    loop {
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
        let (last_output, exited) = cx.update(|cx| {
            let terminal = terminal.read(cx);
            (terminal.last_output(), terminal.exit_status().is_some())
        });
        let quiet = last_output.is_some_and(|at| at.elapsed() >= settle);
        if quiet || exited || started.elapsed() >= timeout {
            return;
        }
    }
}

async fn capture(
    options: &Options,
    terminal: &Entity<Terminal>,
    view: &Entity<TerminalView>,
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
) {
    settle(terminal, options.settle, options.timeout, cx).await;
    let draw = |cx: &mut AsyncApp| {
        let _ = cx.update_window(window, |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
    };
    if let (Some(flight), Some(typed)) = (options.flight, options.typed.as_ref()) {
        // Draw the cursor where it rests, make the program move it, draw the frame
        // that sees the move (the flight starts there), and stop the clock a little later.
        draw(cx);
        let before = cx.update(|cx| terminal.read(cx).bytes_received());
        cx.update(|cx| terminal.read(cx).write(typed.as_bytes()));
        for _ in 0..500 {
            cx.background_executor()
                .timer(Duration::from_millis(2))
                .await;
            if cx.update(|cx| terminal.read(cx).bytes_received()) != before {
                break;
            }
        }
        cx.background_executor()
            .timer(Duration::from_millis(12))
            .await;
        draw(cx);
        // Frames at a 240 Hz display's pace while it flies, so the capture sees the cursor
        // where the display would.
        let flying = Instant::now();
        while flying.elapsed() < flight {
            cx.background_executor()
                .timer(Duration::from_millis(4))
                .await;
            draw(cx);
        }
    } else if let Some(typed) = options.typed.as_ref() {
        cx.update(|cx| terminal.read(cx).write(typed.as_bytes()));
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
        settle(terminal, options.settle, options.timeout, cx).await;
    }
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            if options.select_all {
                view.select_all(cx);
            }
            view.force_composition(options.composing.clone());
        });
    });
    if options.print_text {
        let text = cx.update(|cx| terminal.read(cx).text(None, false));
        println!("{text}");
    }
    let image = cx.update_window(window, |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
        window.render_to_image()
    });
    let out = options.out.as_ref().expect("checked when parsing");
    match image {
        Ok(Ok(image)) => {
            image.save(out).expect("write the capture");
            eprintln!(
                "saved {} ({}x{})",
                out.display(),
                image.width(),
                image.height()
            );
        }
        other => {
            eprintln!("capture failed: {other:?}");
            std::process::exit(2);
        }
    }
}

/// Draws `frames` frames while the zoom sweeps from 0.1 to 4 and back, the way a pinch
/// gesture drives a canvas, and reports what a frame cost.
fn zoom_sweep(frames: u32, stage: &Entity<Stage>, window: AnyWindowHandle, cx: &mut AsyncApp) {
    let zoom = cx.update(|cx| stage.read(cx).zoom.clone());
    let mut times = Vec::with_capacity(frames as usize);
    for frame in 0..frames {
        // Geometric, so every part of the range gets its share of frames.
        let phase = frame as f32 / (frames.max(2) - 1) as f32;
        let there_and_back = 1. - (2. * phase - 1.).abs();
        zoom.set(0.1 * 40f32.powf(there_and_back));
        let started = Instant::now();
        cx.update_window(window, |_, window, cx| {
            stage.update(cx, |_, cx| cx.notify());
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .expect("draw a frame");
        times.push(started.elapsed());
    }
    times.sort();
    let mean = times.iter().sum::<Duration>() / frames.max(1);
    let at = |fraction: f32| times[((times.len() - 1) as f32 * fraction) as usize];
    println!("frames             {frames}");
    println!("frame, mean        {:.2} ms", mean.as_secs_f64() * 1000.);
    println!("frame, median      {:.2} ms", at(0.5).as_secs_f64() * 1000.);
    println!(
        "frame, 95th        {:.2} ms",
        at(0.95).as_secs_f64() * 1000.
    );
    println!("frame, longest     {:.2} ms", at(1.).as_secs_f64() * 1000.);
}

/// Runs the program to completion while drawing a frame for every wakeup, and reports how
/// long the interface thread was ever kept from running.
async fn bench(terminal: &Entity<Terminal>, window: AnyWindowHandle, cx: &mut AsyncApp) {
    let wakeups = Rc::new(Cell::new(0u32));
    let exited = Rc::new(Cell::new(false));
    let dirty = Rc::new(Cell::new(false));
    let subscription = cx.update(|cx| {
        let (wakeups, exited, dirty) = (wakeups.clone(), exited.clone(), dirty.clone());
        cx.subscribe(terminal, move |_, event, _| match event {
            TerminalEvent::Wakeup => {
                wakeups.set(wakeups.get() + 1);
                dirty.set(true);
            }
            TerminalEvent::Exited(_) => exited.set(true),
            _ => {}
        })
    });

    let started = Instant::now();
    let tick = Duration::from_millis(2);
    let mut last_tick = Instant::now();
    let mut longest_stall = Duration::ZERO;
    let mut longest_frame = Duration::ZERO;
    let mut frames = 0u32;
    let mut frame_time = Duration::ZERO;
    while !exited.get() && started.elapsed() < Duration::from_secs(120) {
        cx.background_executor().timer(tick).await;
        // How late this tick is, beyond the timer itself, is time the thread was held up.
        longest_stall = longest_stall.max(last_tick.elapsed().saturating_sub(tick));
        if dirty.replace(false) {
            let frame_started = Instant::now();
            cx.update_window(window, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .expect("draw a frame");
            let elapsed = frame_started.elapsed();
            longest_frame = longest_frame.max(elapsed);
            frame_time += elapsed;
            frames += 1;
        }
        last_tick = Instant::now();
    }
    drop(subscription);

    let elapsed = started.elapsed();
    let (bytes, size) = cx.update(|cx| {
        let terminal = terminal.read(cx);
        (terminal.bytes_received(), terminal.size())
    });
    let megabytes = bytes as f64 / (1024. * 1024.);
    println!("grid               {}x{}", size.0, size.1);
    println!("bytes              {bytes} ({megabytes:.2} MiB)");
    println!("elapsed            {:.3} s", elapsed.as_secs_f64());
    println!(
        "throughput         {:.1} MiB/s",
        megabytes / elapsed.as_secs_f64()
    );
    println!("wakeups            {}", wakeups.get());
    println!("frames drawn       {frames}");
    println!(
        "frame, mean        {:.2} ms",
        frame_time.as_secs_f64() * 1000. / f64::from(frames.max(1))
    );
    println!(
        "frame, longest     {:.2} ms",
        longest_frame.as_secs_f64() * 1000.
    );
    println!(
        "longest stall      {:.2} ms (between two 2 ms ticks of the interface thread)",
        longest_stall.as_secs_f64() * 1000.
    );
}
