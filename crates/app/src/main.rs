//! The plastext application: startup, windows, single-instance server and session restore (spec §7.3).

mod windows;

use std::path;

use clap::Parser as _;
use gpui::{
    App, Application, Bounds, Focusable, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, actions, prelude::*, px, size,
};
use plastext_cli::{Cli, Location};
use plastext_ui::Workspace;

use crate::windows::{WindowRequest, window_requests};

actions!(plastext, [Quit]);

fn main() {
    let cli = Cli::parse();
    let locations: Vec<Location> = cli
        .locations
        .into_iter()
        .map(|mut location| {
            if let Ok(path) = path::absolute(&location.path) {
                location.path = path;
            }
            location
        })
        .collect();

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
    });
}

fn open_window(request: WindowRequest, cx: &mut App) {
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
