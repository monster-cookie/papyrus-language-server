use std::{
    fs,
    path::{Path, PathBuf},
};

const CREATION_KIT_APP_ID: &str = "2722710";

/// Locates Starfield Creation Kit's source archive through Steam metadata.
pub(crate) fn discover_starfield_archive() -> Option<PathBuf> {
    steam_roots()
        .into_iter()
        .flat_map(|root| steam_libraries(&root))
        .find_map(|library| archive_from_library(&library))
}

fn archive_from_library(library: &Path) -> Option<PathBuf> {
    let manifest = library
        .join("steamapps")
        .join(format!("appmanifest_{CREATION_KIT_APP_ID}.acf"));
    let text = fs::read_to_string(manifest).ok()?;
    let install_dir = quoted_value(&text, "installdir")?;
    let archive = library
        .join("steamapps")
        .join("common")
        .join(install_dir)
        .join("Tools")
        .join("ContentResources.zip");
    archive.is_file().then_some(archive)
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

    use super::{archive_from_library, quoted_value};

    #[test]
    fn reads_steam_key_values() {
        assert_eq!(
            quoted_value("\"installdir\" \t \"Starfield\"", "installdir"),
            Some("Starfield")
        );
    }

    #[test]
    fn resolves_creation_kit_archive_from_manifest() {
        let library = std::env::temp_dir().join(format!(
            "papyrus-steam-test-{}",
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
        fs::write(
            library.join("steamapps/common/Starfield/Tools/ContentResources.zip"),
            [],
        )
        .unwrap();
        assert!(archive_from_library(&library).is_some());
        fs::remove_dir_all(library).unwrap();
    }
}
