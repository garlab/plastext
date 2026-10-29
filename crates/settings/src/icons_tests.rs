use super::*;
use serde_json::json;

fn theme() -> IconTheme {
    let mut text = json!({
        "file":"file", "folder":"folder", "folderExpanded":"open",
        "rootFolder":"root", "rootFolderExpanded":"root-open",
        "folderNames":{"src":"named-folder"}, "folderNamesExpanded":{"src":"named-open"},
        "fileNames":{"src/thing.ts":"parent-name", "thing.ts":"name", "résumé.ts":"name", "hidden.ts":""},
        "fileExtensions":{"src/ts":"parent-ext", "d.ts":"compound", "ts":"ext"},
        "languageIds":{"rust":"lang"},
        "light":{"file":"light", "fileExtensions":{"ts":"light-ext"}},
        "highContrast":{"file":"contrast"}, "hidesExplorerArrows":true,
    });
    text["iconDefinitions"] = [
        "file",
        "folder",
        "open",
        "root",
        "root-open",
        "named-folder",
        "named-open",
        "parent-name",
        "name",
        "parent-ext",
        "compound",
        "ext",
        "lang",
        "light",
        "light-ext",
        "contrast",
    ]
    .into_iter()
    .map(|id| (id.to_owned(), json!({"iconPath":format!("{id}.svg")})))
    .collect::<serde_json::Map<_, _>>()
    .into();
    IconTheme::parse(&text.to_string()).unwrap()
}

#[test]
fn associations_follow_vscode_precedence_and_appearance_overrides() {
    let theme = theme();
    for (path, language, expected) in [
        ("SRC/THING.TS", None, Some("parent-name.svg")),
        ("other/thing.ts", None, Some("name.svg")),
        ("other/RÉSUMÉ.ts", None, Some("name.svg")),
        ("src/value.d.ts", None, Some("parent-ext.svg")),
        ("other/value.d.ts", None, Some("compound.svg")),
        ("other/value.ts", Some("rust"), Some("ext.svg")),
        ("other/value.unknown", Some("rust"), Some("lang.svg")),
        ("other/value.unknown", None, Some("file.svg")),
        ("hidden.ts", None, None),
    ] {
        assert_eq!(
            theme.resolve(
                Path::new(path),
                false,
                false,
                false,
                language,
                Appearance::Dark
            ),
            expected,
            "{path}"
        );
    }
    assert_eq!(
        theme.resolve(
            Path::new("other/thing.ts"),
            false,
            false,
            false,
            None,
            Appearance::Light
        ),
        Some("name.svg")
    );
    assert_eq!(
        theme.resolve(
            Path::new("other/value.ts"),
            false,
            false,
            false,
            None,
            Appearance::Light
        ),
        Some("light-ext.svg")
    );
    assert_eq!(
        theme.resolve(
            Path::new("unknown"),
            false,
            false,
            false,
            None,
            Appearance::HighContrast
        ),
        Some("contrast.svg")
    );
}

#[test]
fn folders_use_named_expanded_and_root_associations() {
    let theme = theme();
    for (path, expanded, root, expected) in [
        ("project/src", false, false, "named-folder.svg"),
        ("project/src", true, false, "named-open.svg"),
        ("project/docs", false, false, "folder.svg"),
        ("project/docs", true, false, "open.svg"),
        ("project", false, true, "root.svg"),
        ("project", true, true, "root-open.svg"),
    ] {
        assert_eq!(
            theme.resolve(
                Path::new(path),
                true,
                expanded,
                root,
                None,
                Appearance::Dark
            ),
            Some(expected)
        );
    }
    assert!(theme.hides_explorer_arrows);
}

#[test]
fn jsonc_and_format_errors_are_explicit() {
    let theme = IconTheme::parse("\u{feff}{ // comment\n\"iconDefinitions\":{\"f\":{\"iconPath\":\"f.svg\"}},\"file\":\"f\",}").unwrap();
    assert_eq!(
        theme.resolve(Path::new("a"), false, false, false, None, Appearance::Dark),
        Some("f.svg")
    );
    for text in [
        "{invalid}",
        "{}",
        r#"{"iconDefinitions":{"f":{"iconPath":"f.svg"}},"file":"missing"}"#,
        r#"{"iconDefinitions":{"f":{"iconPath":"f.jpg"}}}"#,
    ] {
        assert!(IconTheme::parse(text).is_err());
    }
    let error = IconTheme::parse(r#"{"iconDefinitions":{"f":{"fontCharacter":"x"}},"file":"f"}"#)
        .unwrap_err();
    assert!(error.contains("icon fonts"));
    assert!(IconTheme::parse(&" ".repeat(MAX_JSON_BYTES as usize + 1)).is_err());
}

fn extension(root: &Path) {
    fs::create_dir(root.join("themes")).unwrap();
    fs::create_dir(root.join("icons")).unwrap();
    fs::write(
        root.join("package.json"),
        r#"{"contributes":{"iconThemes":[
        {"id":"one","label":"%themeName%","path":"./themes/one.json"},
        {"id":"two","label":"Two","path":"./themes/two.json"}]}}"#,
    )
    .unwrap();
    fs::write(
        root.join("package.nls.json"),
        r#"{"themeName":"One localized"}"#,
    )
    .unwrap();
    for name in ["one", "two"] {
        fs::write(
            root.join(format!("themes/{name}.json")),
            r#"{"iconDefinitions":{"f":{"iconPath":"../icons/f.svg"}},"file":"f"}"#,
        )
        .unwrap();
    }
}

#[test]
fn discovers_extension_folders_and_direct_json_with_sibling_assets() {
    let dir = tempfile::tempdir().unwrap();
    extension(dir.path());
    let entries = discover(dir.path()).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "One localized");
    assert_eq!(
        discover(&entries[0].path).unwrap(),
        vec![entries[0].clone()]
    );
    assert_eq!(discover(&dir.path().join("package.json")).unwrap(), entries);
    for entry in entries {
        let theme = load(&entry).unwrap();
        let asset = theme
            .resolve(Path::new("a"), false, false, false, None, Appearance::Dark)
            .unwrap();
        assert_eq!(
            asset_path(&entry, asset).unwrap(),
            dir.path().canonicalize().unwrap().join("icons/f.svg")
        );
        for bad in [
            "../../outside.svg",
            "https://example.test/icon.svg",
            "/tmp/icon.svg",
            "..\\..\\outside.svg",
        ] {
            assert!(asset_path(&entry, bad).is_err(), "{bad}");
        }
    }
    let empty = tempfile::tempdir().unwrap();
    fs::write(empty.path().join("package.json"), "{}").unwrap();
    assert!(
        discover(empty.path())
            .unwrap_err()
            .contains("no contributed")
    );
}

#[test]
fn selection_survives_restart_without_rewriting_other_settings() {
    let dir = tempfile::tempdir().unwrap();
    extension(dir.path());
    let path = dir.path().join("config.toml");
    let original = "# User settings\n[editor]\nfont_size = 17 # keep this\n\n[theme.dark]\nbackground = '#112233'\n";
    fs::write(&path, original).unwrap();
    let imports = discover(dir.path()).unwrap();
    let preferences = IconSettings {
        theme: imports[1].id(),
        imports,
        high_contrast: true,
    };
    preferences.save(&path).unwrap();
    let saved = fs::read_to_string(&path).unwrap();
    assert!(saved.starts_with(original));
    let loaded = crate::Settings::load_from(&path).unwrap();
    assert_eq!(loaded.icons, preferences);
    assert_eq!(loaded.editor.font_size, 17.);
    assert_eq!(loaded.theme.dark.background, Some(crate::Color(0x112233ff)));
    let mut disabled = preferences;
    disabled.theme = "none".into();
    disabled.save(&path).unwrap();
    assert_eq!(crate::Settings::load_from(&path).unwrap().icons, disabled);
    fs::write(&path, "[invalid").unwrap();
    assert!(disabled.save(&path).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "[invalid");
}

#[cfg(unix)]
#[test]
fn saving_preserves_a_symlinked_config_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("dotfiles.toml");
    let path = dir.path().join("config.toml");
    fs::write(&target, "# Managed dotfile\n[editor]\nfont_size = 17\n").unwrap();
    std::os::unix::fs::symlink("dotfiles.toml", &path).unwrap();
    let preferences = IconSettings {
        theme: "none".into(),
        ..Default::default()
    };
    preferences.save(&path).unwrap();
    assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
    assert_eq!(
        crate::Settings::load_from(&target).unwrap().icons,
        preferences
    );
    assert_eq!(
        crate::Settings::load_from(&path).unwrap().editor.font_size,
        17.
    );
}

#[test]
fn configured_paths_are_relative_to_the_config_file() {
    let dir = tempfile::tempdir().unwrap();
    extension(dir.path());
    let path = dir.path().join("config.toml");
    fs::write(&path,"[icons]\ntheme='themes/one.json'\n[[icons.imports]]\nname='One'\npath='themes/one.json'\nroot='.'\n").unwrap();
    let settings = crate::Settings::load_from(&path).unwrap();
    assert_eq!(settings.icons.theme, settings.icons.imports[0].id());
    assert!(load(&settings.icons.imports[0]).is_ok());
}

#[test]
fn extracted_vsix_root_finds_the_extension_subfolder() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("extension")).unwrap();
    extension(&dir.path().join("extension"));
    assert_eq!(discover(dir.path()).unwrap().len(), 2);
}
