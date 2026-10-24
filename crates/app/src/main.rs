//! The plastext application: startup, windows, single-instance server and session restore (spec §7.3).

mod windows;

use std::{ffi::OsString, io, iter, process::ExitCode};

use clap::Parser as _;
use futures::{StreamExt as _, channel::mpsc};
use gpui::{
    App, Application, Bounds, Focusable, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, actions, prelude::*, px, size,
};
use plastext_cli::Cli;
use plastext_platform::instance::{self, OpenRequest};
use plastext_ui::Workspace;

use crate::windows::{WindowRequest, window_requests};

actions!(plastext, [Quit]);

fn main() -> ExitCode {
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

        open(request, cx);
        cx.spawn(async move |cx| {
            while let Some(request) = requests.next().await {
                if cx.update(|cx| open(request, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    });
    ExitCode::SUCCESS
}

/// Opens what a launch asked for, reusing windows that show the same folder.
fn open(request: OpenRequest, cx: &mut App) {
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
                open_window(request, cx);
            }
            cx.activate(true);
        })
        .ok();
    })
    .detach();
}

fn open_window(request: WindowRequest, cx: &mut App) {
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
    let opened = cx.open_window(options, |window, cx| {
        let workspace = cx.new(|cx| {
            let mut workspace = Workspace::new(request.root, window, cx);
            for file in request.files {
                workspace.open_file(file.path, file.line.map(u64::from), window, cx);
            }
            workspace
        });
        workspace.focus_handle(cx).focus(window);
        workspace
    });
    if let Err(err) = opened {
        eprintln!("plastext: cannot open a window: {err:#}");
    }
}
