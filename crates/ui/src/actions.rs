//! Commands exposed to the application menu bar.
pub use crate::workspace::{
    CloseTab, CloseWindow, FileIconTheme, GoBack, GoForward, GoToLine, GoToSymbol, ImportIconTheme,
    NewFile, OpenCommandPalette, OpenQuickOpen, ResetZoom, SearchProject, ShowFiles, SplitDown,
    SplitRight, ToggleOutline, ZoomIn, ZoomOut,
};
gpui::actions!(preview, [TogglePreview]);
gpui::actions!(workspace, [OpenFile, OpenFolder, ClearRecentFolders]);

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = workspace, no_json)]
pub struct OpenRecentFolder {
    pub path: std::path::PathBuf,
}
