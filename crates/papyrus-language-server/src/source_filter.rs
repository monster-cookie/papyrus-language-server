use std::path::Path;

/// Returns whether an installed-source path is a generated gameplay fragment.
///
/// Project sources are never passed through this policy; it applies only to
/// automatically discovered SDK/Creation Kit inputs.
pub(crate) fn is_generated_source(path: &Path) -> bool {
    if path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("fragments")
    }) {
        return true;
    }
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    ["QF_", "PF_", "TIF_", "SF_"].iter().any(|prefix| {
        stem.get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::is_generated_source;

    #[test]
    fn filters_generated_fragments_but_retains_reusable_scripts() {
        for path in [
            "Scripts/Source/Fragments/Quests/QF_Test.psc",
            "Scripts/Source/PF_Test.psc",
            "Scripts/Source/TIF_Test.psc",
            "Scripts/Source/SF_Test.psc",
        ] {
            assert!(is_generated_source(Path::new(path)), "{path}");
        }
        assert!(!is_generated_source(Path::new("Scripts/Source/Quest.psc")));
        assert!(!is_generated_source(Path::new("Scripts/Source/Perk.psc")));
        assert!(!is_generated_source(Path::new("Scripts/Source/éé.psc")));
    }
}
