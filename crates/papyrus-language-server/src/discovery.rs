use std::{
    fs,
    path::{Path, PathBuf},
};

const CREATION_KIT_APP_ID: &str = "2722710";

/// Source locations supplied by a Starfield Creation Kit installation.
pub(crate) struct StarfieldSources {
    /// Directly installed Papyrus sources, preferred when the Creation Kit has expanded them.
    pub(crate) source_directory: Option<PathBuf>,
    /// Creation Kit source archive used when directly installed sources are unavailable.
    pub(crate) archive: Option<PathBuf>,
}

/// Locates Starfield Creation Kit's installed sources and source archive through Steam metadata.
pub(crate) fn discover_starfield_sources() -> Option<StarfieldSources> {
    steam_roots()
        .into_iter()
        .flat_map(|root| steam_libraries(&root))
        .find_map(|library| sources_from_library(&library))
}

fn sources_from_library(library: &Path) -> Option<StarfieldSources> {
    let manifest = library
        .join("steamapps")
        .join(format!("appmanifest_{CREATION_KIT_APP_ID}.acf"));
    let text = fs::read_to_string(manifest).ok()?;
    let install_dir = quoted_value(&text, "installdir")?;
    let installation = library.join("steamapps").join("common").join(install_dir);
    let source_directory = installation.join("Data").join("Scripts").join("Source");
    let archive = installation.join("Tools").join("ContentResources.zip");
    let sources = StarfieldSources {
        source_directory: source_directory.is_dir().then_some(source_directory),
        archive: archive.is_file().then_some(archive),
    };
    (sources.source_directory.is_some() || sources.archive.is_some()).then_some(sources)
}

fn steam_libraries(root: &Path) -> Vec<PathBuf> {
    let mut libraries = vec![root.to_owned()];
    let path = root.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = fs::read_to_string(path) {
        for line in text.lines() {
            if let Some(path) = quoted_value(line, "path") {
                let candidate = PathBuf::from(path.replace("\\\\", "\\"));
                if !libraries.iter().any(|existing| existing == &candidate) {
                    libraries.push(candidate);
                }
            }
        }
    }
    libraries
}

fn quoted_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let mut values = line.split('"').skip(1).step_by(2);
        let found_key = values.next()?;
        let value = values.next()?;
        found_key.eq_ignore_ascii_case(key).then_some(value)
    })
}

#[cfg(windows)]
fn steam_roots() -> Vec<PathBuf> {
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    };

    let candidates = [
        (HKEY_CURRENT_USER, r"Software\Valve\Steam", "SteamPath"),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Valve\Steam",
            "InstallPath",
        ),
    ];
    let mut roots = Vec::new();
    for (hive, key, value) in candidates {
        if let Ok(key) = RegKey::predef(hive).open_subkey(key)
            && let Ok::<String, _>(path) = key.get_value(value)
        {
            let path = PathBuf::from(path);
            if !roots.iter().any(|root| root == &path) {
                roots.push(path);
            }
        }
    }
    roots
}

#[cfg(not(windows))]
fn steam_roots() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{quoted_value, sources_from_library};

    #[test]
    fn reads_steam_key_values() {
        assert_eq!(
            quoted_value("\"installdir\" \t \"Starfield\"", "installdir"),
            Some("Starfield")
        );
    }

    #[test]
    fn resolves_creation_kit_sources_from_manifest() {
        let library = std::env::temp_dir().join(format!(
            "papyrus-steam-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(library.join("steamapps/common/Starfield/Tools")).unwrap();
        fs::create_dir_all(library.join("steamapps/common/Starfield/Data/Scripts/Source")).unwrap();
        fs::write(
            library.join("steamapps/appmanifest_2722710.acf"),
            "\"AppState\"\n{\n\t\"installdir\"\t\"Starfield\"\n}\n",
        )
        .unwrap();
        fs::write(
            library.join("steamapps/common/Starfield/Tools/ContentResources.zip"),
            [],
        )
        .unwrap();
        let sources = sources_from_library(&library).unwrap();
        assert_eq!(
            sources.source_directory,
            Some(library.join("steamapps/common/Starfield/Data/Scripts/Source"))
        );
        assert_eq!(
            sources.archive,
            Some(library.join("steamapps/common/Starfield/Tools/ContentResources.zip"))
        );
        fs::remove_dir_all(library).unwrap();
    }

    #[test]
    fn retains_archive_when_direct_sources_are_unavailable() {
        let library = std::env::temp_dir().join(format!(
            "papyrus-steam-archive-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(library.join("steamapps/common/Starfield/Tools")).unwrap();
        fs::write(
            library.join("steamapps/appmanifest_2722710.acf"),
            "\"AppState\"\n{\n\t\"installdir\"\t\"Starfield\"\n}\n",
        )
        .unwrap();
        let archive = library.join("steamapps/common/Starfield/Tools/ContentResources.zip");
        fs::write(&archive, []).unwrap();
        let sources = sources_from_library(&library).unwrap();
        assert_eq!(sources.source_directory, None);
        assert_eq!(sources.archive, Some(archive));
        fs::remove_dir_all(library).unwrap();
    }
}
