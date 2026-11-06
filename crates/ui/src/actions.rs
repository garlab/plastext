//! Commands exposed to the application menu bar.
pub use crate::workspace::{
    CloseTab, CloseWindow, FileIconTheme, GoBack, GoForward, GoToLine, GoToSymbol, ImportIconTheme,
    NewFile, OpenCommandPalette, OpenQuickOpen, ResetZoom, SearchProject, ShowFiles, SplitDown,
    SplitRight, ToggleOutline, ZoomIn, ZoomOut,
};
gpui::actions!(preview, [TogglePreview]);
