use papyrus_language_server::PapyrusAnalyzer;

const VALID_FIXTURES: &[(&str, &str)] = &[
    (
        "Starfield basic",
        include_str!("../../../test-data/starfield/BasicStarfield.psc"),
    ),
    (
        "Starfield advanced",
        include_str!("../../../test-data/starfield/AdvancedStarfield.psc"),
    ),
    (
        "Skyrim basic",
        include_str!("../../../test-data/skyrim/BasicSkyrim.psc"),
    ),
    (
        "Skyrim advanced",
        include_str!("../../../test-data/skyrim/AdvancedSkyrim.psc"),
    ),
    (
        "Fallout 4 basic",
        include_str!("../../../test-data/fallout4/BasicFallout4.psc"),
    ),
    (
        "Fallout 4 advanced",
        include_str!("../../../test-data/fallout4/AdvancedFallout4.psc"),
    ),
];

#[test]
fn valid_cross_dialect_fixtures_have_no_diagnostics() {
    let mut analyzer = PapyrusAnalyzer::new().expect("grammar should load");
    for (name, source) in VALID_FIXTURES {
        let diagnostics = analyzer.diagnostics(source);
        assert!(diagnostics.is_empty(), "{name}: {diagnostics:#?}");
    }
}

#[test]
fn starfield_fixture_names_the_missing_endif() {
    let source = include_str!("../../../test-data/invalid/InvalidSyntax.psc");
    let mut analyzer = PapyrusAnalyzer::new().expect("grammar should load");
    let diagnostics = analyzer.diagnostics(source);

    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.message == "Missing EndIf before EndFunction"
                && diagnostic.range.start.line == 5
        }),
        "{diagnostics:#?}"
    );
}

#[test]
fn dialect_fixtures_name_missing_outer_closers() {
    let cases = [
        (
            include_str!("../../../test-data/invalid/InvalidSkyrim.psc"),
            "Missing EndState",
        ),
        (
            include_str!("../../../test-data/invalid/InvalidFallout4.psc"),
            "Missing EndStruct",
        ),
    ];
    let mut analyzer = PapyrusAnalyzer::new().expect("grammar should load");

    for (source, expected) in cases {
        let diagnostics = analyzer.diagnostics(source);
        assert_eq!(diagnostics.len(), 1, "expected one issue: {diagnostics:#?}");
        assert_eq!(diagnostics[0].message, expected);
    }
}

#[test]
fn inserting_endif_clears_the_structural_diagnostic() {
    let invalid = "ScriptName Test\nFunction Run()\nIf True\nEndFunction\n";
    let valid = "ScriptName Test\nFunction Run()\nIf True\nEndIf\nEndFunction\n";
    let mut analyzer = PapyrusAnalyzer::new().expect("grammar should load");

    assert!(
        analyzer
            .diagnostics(invalid)
            .iter()
            .any(|diagnostic| { diagnostic.message == "Missing EndIf before EndFunction" })
    );
    assert!(analyzer.diagnostics(valid).is_empty());
}

#[test]
fn unicode_before_an_error_uses_utf16_columns() {
    let source = "ScriptName Test\nFunction Run()\nDebug.Trace(\"😀\") EndIf\nEndFunction\n";
    let mut analyzer = PapyrusAnalyzer::new().expect("grammar should load");
    let diagnostics = analyzer.diagnostics(source);

    assert!(!diagnostics.is_empty());
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.range.start.line <= 3)
    );
}

#[test]
#[ignore = "requires PAPYRUS_AUDIT_ROOT to reference locally installed game sources"]
fn installed_source_audit() {
    let root = std::env::var_os("PAPYRUS_AUDIT_ROOT")
        .map(std::path::PathBuf::from)
        .expect("PAPYRUS_AUDIT_ROOT must be set");
    let mut files = Vec::new();
    collect_papyrus_files(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no .psc files found under {}",
        root.display()
    );

    let mut analyzer = PapyrusAnalyzer::new().expect("grammar should load");
    let mut failures = Vec::new();
    for file in &files {
        let source_bytes = match std::fs::read(file) {
            Ok(source_bytes) => source_bytes,
            Err(error) => {
                failures.push(format!(
                    "{}: could not read source: {error}",
                    file.display()
                ));
                continue;
            }
        };
        let source = String::from_utf8_lossy(&source_bytes);
        let diagnostics = analyzer.diagnostics(&source);
        if !diagnostics.is_empty() {
            failures.push(format!("{}: {diagnostics:#?}", file.display()));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} installed sources produced diagnostics:\n{}",
        failures.len(),
        files.len(),
        failures.into_iter().take(20).collect::<Vec<_>>().join("\n")
    );
    eprintln!("Audited {} installed Papyrus sources.", files.len());
}

fn collect_papyrus_files(directory: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", directory.display()));
    for entry in entries {
        let entry = entry.expect("directory entry should be readable");
        let path = entry.path();
        if path.is_dir() {
            collect_papyrus_files(&path, files);
        } else if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("psc"))
        {
            files.push(path);
        }
    }
}
