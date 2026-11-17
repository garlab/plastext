use plastext_syntax::{Highlight, SyntaxTree, detect, language_for_label};
use std::path::Path;

const SOURCE: &str = "# café 🎲\n@mode enumerate\nfn roll(x: int): int {\n let r ~ 2d6\n report \"λ {x + 1}\"\n r\n}\nreport '1h 30m'\n";

#[test]
fn files_fences_colors_and_folds_share_the_grammar() {
    let language = detect(Path::new("model.test.PROBL"), b"").unwrap();
    assert_eq!(language.name, "Probl");
    for label in ["probl", "Probl", "PROBL"] {
        assert!(std::ptr::eq(language, language_for_label(label).unwrap()));
    }
    let tree = SyntaxTree::parse(language, SOURCE.as_bytes(), || false).unwrap();
    let spans = tree.highlights(SOURCE.as_bytes(), 0..SOURCE.len());
    for (token, expected) in [
        ("# café", Highlight::Comment),
        ("mode", Highlight::Attribute),
        ("roll", Highlight::Function),
        ("int", Highlight::Type),
        ("2d6", Highlight::Number),
        ("report", Highlight::Keyword),
        ("λ", Highlight::String),
        ("+", Highlight::Operator),
        ("'1h 30m'", Highlight::Constant),
    ] {
        let byte = SOURCE.find(token).unwrap();
        assert_eq!(
            spans
                .iter()
                .find(|(range, _)| range.contains(&byte))
                .map(|(_, kind)| *kind),
            Some(expected),
            "{token}"
        );
    }
    assert!(
        spans
            .iter()
            .all(|(range, _)| SOURCE.get(range.clone()).is_some())
    );
    assert!(
        spans
            .windows(2)
            .all(|pair| pair[0].0.end <= pair[1].0.start)
    );
    let folds = tree.folds(SOURCE.as_bytes()).unwrap();
    assert_eq!(folds.len(), 1);
    assert!(SOURCE[folds[0].hidden.start as usize..folds[0].hidden.end as usize].contains("let r"));
    assert!(tree.symbols(SOURCE.as_bytes()).is_empty());
}

#[test]
fn edits_undo_and_initial_parse_keep_highlights() {
    let language = language_for_label("probl").unwrap();
    let mut before = SOURCE.to_owned();
    let mut tree = SyntaxTree::parse(language, before.as_bytes(), || false).unwrap();
    for after in [
        SOURCE.replace("2d6", "3d6"),
        SOURCE.replace("{x + 1}", "{roll(2)}"),
        SOURCE.replace("\"λ {x + 1}\"", "\"unfinished"),
        SOURCE.to_owned(),
    ] {
        tree = tree
            .reparse(before.as_bytes(), after.as_bytes(), || false)
            .unwrap();
        let fresh = SyntaxTree::parse(language, after.as_bytes(), || false).unwrap();
        assert_eq!(
            tree.highlights(after.as_bytes(), 0..after.len()),
            fresh.highlights(after.as_bytes(), 0..after.len())
        );
        assert_eq!(tree.folds(after.as_bytes()), fresh.folds(after.as_bytes()));
        before = after;
    }
    let large = SOURCE.repeat(2000);
    let initial = SyntaxTree::parse_initial(language, large.as_bytes(), || false).unwrap();
    let full = SyntaxTree::parse(language, large.as_bytes(), || false).unwrap();
    assert_eq!(
        initial.highlights(large.as_bytes(), 0..SOURCE.len()),
        full.highlights(large.as_bytes(), 0..SOURCE.len())
    );
    assert!(SyntaxTree::parse(language, large.as_bytes(), || true).is_none());
}

#[test]
fn line_local_fallback_handles_large_files_and_unfinished_strings() {
    let lexical = language_for_label("probl").unwrap().lexical;
    let line = "let roll: int = 2d6 # café 🎲";
    let spans = lexical.highlights(line.as_bytes());
    for (token, expected) in [
        ("let", Highlight::Keyword),
        ("int", Highlight::Type),
        ("2d6", Highlight::Number),
        ("# café 🎲", Highlight::Comment),
    ] {
        assert!(
            spans
                .iter()
                .any(|(range, kind)| &line[range.clone()] == token && *kind == expected)
        );
    }
    let line = "report \"unfinished 🎲";
    assert!(
        lexical
            .highlights(line.as_bytes())
            .iter()
            .all(|(range, _)| line.get(range.clone()).is_some())
    );
}
