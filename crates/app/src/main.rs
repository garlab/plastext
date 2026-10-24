//! The plastext application: startup, windows, single-instance server and session restore (spec §7.3).

mod windows;

use std::{
    env,
    ffi::OsString,
    io, iter,
    process::ExitCode,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::Parser as _;
use futures::{StreamExt as _, channel::mpsc};
use gpui::{
    App, Application, Bounds, Focusable, Global, KeyBinding, Menu, MenuItem, SharedString,
    TitlebarOptions, WindowBounds, WindowHandle, WindowOptions, actions, prelude::*, px, size,
};
use plastext_cli::Cli;
use plastext_platform::instance::{self, OpenRequest};
use plastext_settings::Settings;
use plastext_ui::Workspace;

use crate::windows::{WindowRequest, window_requests};

actions!(plastext, [Quit]);

/// The settings every new window starts from.
struct AppSettings(Arc<Settings>);

impl Global for AppSettings {}

/// Set by `PLASTEXT_PROFILE_STARTUP`: report how long the first window took
/// to draw, then quit. `plastext-bench launch` runs the app this way.
struct StartupProfile {
    start: Instant,
    reported: bool,
}

impl Global for StartupProfile {}

fn main() -> ExitCode {
    let start = Instant::now();
    // Handles `--help` and `--version`, and rejects bad arguments.
    Cli::parse();
    let request = match OpenRequest::from_env() {
        Ok(request) => request,
        Err(err) => {
            eprintln!("plastext: cannot read the working directory: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Later launches hand their arguments over instead of starting another app.
    let (sender, mut requests) = mpsc::unbounded();
    let _server = match instance::listen(move |request| {
        sender.unbounded_send(request).ok();
    }) {
        Ok(server) => Some(server),
        Err(err) if err.kind() == io::ErrorKind::AddrInUse && instance::send(&request).is_ok() => {
            return ExitCode::SUCCESS;
        }
        Err(err) => {
            eprintln!("plastext: later launches will start separate apps: {err}");
            None
        }
    };

    Application::new().run(move |cx: &mut App| {
        if env::var_os("PLASTEXT_PROFILE_STARTUP").is_some() {
            cx.set_global(StartupProfile {
                start,
                reported: false,
            });
        }
        plastext_ui::init(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(vec![Menu {
            name: "plastext".into(),
            items: vec![MenuItem::action("Quit plastext", Quit)],
        }]);
        // Nothing can open a folder from the UI yet, so a windowless app is useless.
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let settings = cx.background_executor().spawn(async { Settings::load() });
        cx.spawn(async move |cx| {
            let (settings, notice) = match settings.await {
                Ok(settings) => (settings, None),
                Err(err) => (
                    Settings::default(),
                    Some(SharedString::from(err.to_string())),
                ),
            };
            let opened = cx.update(|cx| {
                cx.set_global(AppSettings(Arc::new(settings)));
                open(request, notice, cx);
            });
            if opened.is_err() {
                return;
            }
            while let Some(request) = requests.next().await {
                if cx.update(|cx| open(request, None, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    });
    ExitCode::SUCCESS
}

/// Opens what a launch asked for, reusing windows that show the same folder,
/// and shows `notice` in the windows it opens.
fn open(request: OpenRequest, notice: Option<SharedString>, cx: &mut App) {
    let args = iter::once(OsString::from("plastext")).chain(request.args.iter().cloned());
    let Ok(cli) = Cli::try_parse_from(args) else {
        return;
    };
    if cli.locations.is_empty() && !cx.windows().is_empty() {
        cx.activate(true);
        return;
    }
    let locations = cli
        .locations
        .into_iter()
        .map(|mut location| {
            location.path = request.resolve(&location.path);
            location
        })
        .collect();

    // Telling files from folders touches the disk, so it happens off the main thread.
    let requests = cx
        .background_executor()
        .spawn(async move { window_requests(locations) });
    cx.spawn(async move |cx| {
        let requests = requests.await;
        cx.update(|cx| {
            for request in requests {
                open_window(request, notice.clone(), cx);
            }
            cx.activate(true);
        })
        .ok();
    })
    .detach();
}

fn open_window(request: WindowRequest, notice: Option<SharedString>, cx: &mut App) {
    let existing = cx
        .windows()
        .into_iter()
        .filter_map(|window| window.downcast::<Workspace>())
        .find(|window| {
            window
                .read(cx)
                .is_ok_and(|workspace| workspace.root() == request.root.as_deref())
        });
    if let Some(window) = existing {
        window
            .update(cx, |workspace, window, cx| {
                for file in request.files {
                    workspace.open_file(file.path, file.line.map(u64::from), window, cx);
                }
                if let Some(notice) = notice {
                    workspace.show_notice(notice, cx);
                }
                window.activate_window();
            })
            .ok();
        return;
    }

    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(Workspace::title(request.root.as_deref())),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1200.), px(780.)),
            cx,
        ))),
        ..Default::default()
    };
    let settings = cx.global::<AppSettings>().0.clone();
    let opening = Instant::now();
    let opened = cx.open_window(options, |window, cx| {
        let workspace = cx.new(|cx| {
            let mut workspace = Workspace::new(request.root, settings, window, cx);
            for file in request.files {
                workspace.open_file(file.path, file.line.map(u64::from), window, cx);
            }
            if let Some(notice) = notice {
                workspace.show_notice(notice, cx);
            }
            workspace
        });
        workspace.focus_handle(cx).focus(window);
        workspace
    });
    match opened {
        Ok(window) => profile_first_frame(window, opening.elapsed(), cx),
        Err(err) => eprintln!("plastext: cannot open a window: {err:#}"),
    }
}

/// When profiling startup, reports once `window` has drawn its first frame, then quits.
fn profile_first_frame(window: WindowHandle<Workspace>, opening: Duration, cx: &mut App) {
    let Some(profile) = cx.try_global::<StartupProfile>() else {
        return;
    };
    if profile.reported {
        return;
    }
    let start = profile.start;
    cx.global_mut::<StartupProfile>().reported = true;
    let millis = |duration: Duration| duration.as_secs_f64() * 1000.;
    window
        .update(cx, |_, window, _| {
            // Frame callbacks run before drawing, so the second one runs after
            // the first frame: an upper bound within one display refresh.
            window.on_next_frame(move |window, _| {
                window.refresh();
                window.on_next_frame(move |_, cx| {
                    eprintln!(
                        "startup: window opened in {:.1} ms, first frame drawn by {:.1} ms",
                        millis(opening),
                        millis(start.elapsed())
                    );
                    cx.quit();
                });
            });
        })
        .ok();
}
