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
        Replace,
        Indent,
        Outdent,
        ToggleComment,
        MoveLineUp,
        MoveLineDown,
        DuplicateLineUp,
        DuplicateLineDown,
        AddNextOccurrence,
        SelectAllOccurrences,
        AddCursorAbove,
        AddCursorBelow,
        JumpToBracket,
        ToggleWhitespace,
        OpenSettings,
        ToggleWordWrap
    ]
);

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = edit, no_json)]
pub struct SetEncoding {
    pub label: String,
    pub bom: bool,
}
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = edit, no_json)]
pub struct SetLineEnding {
    pub ending: String,
}

#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(namespace = edit, no_json)]
pub struct ReopenEncoding {
    pub label: String,
}
