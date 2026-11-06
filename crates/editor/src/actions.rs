//! Shared command identities for keyboard bindings and native menus. The focused
//! view supplies the implementation (editor, text field, preview or file tree).
gpui::actions!(
    edit,
    [
        Copy,
        Cut,
        Paste,
        SelectAll,
        Undo,
        Redo,
        Save,
        Find,
        FindNext,
        FindPrevious,
        ShowCharacterPalette,
        ToggleWordWrap
    ]
);
