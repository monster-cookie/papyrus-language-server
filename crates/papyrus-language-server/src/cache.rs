use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use zip::ZipArchive;

use crate::source_filter::is_generated_source;

pub(crate) struct CacheResult {
    pub(crate) root: PathBuf,
    pub(crate) indexed: usize,
    pub(crate) excluded: usize,
}

/// Materializes navigable, retained SFCK sources in a versioned local cache.
pub(crate) fn materialize_starfield_sources(archive_path: &Path) -> io::Result<CacheResult> {
    materialize_at(archive_path, &cache_base()?)
}

fn materialize_at(archive_path: &Path, base: &Path) -> io::Result<CacheResult> {
    let metadata = fs::metadata(archive_path)?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let fingerprint = format!("{}-{modified}", metadata.len());
    let cache_root = base.join(fingerprint);
    let source_root = cache_root.join("Scripts").join("Source");
    let marker = cache_root.join(".complete");
    if marker.is_file() {
        let indexed = count_sources(&source_root);
        let excluded = fs::read_to_string(&marker)
            .ok()
            .and_then(|content| marker_count(&content, "excluded"))
            .unwrap_or_default();
        return Ok(CacheResult {
            root: source_root,
            indexed,
            excluded,
        });
    }

    fs::create_dir_all(&cache_root)?;
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file).map_err(io::Error::other)?;
    let mut indexed = 0;
    let mut excluded = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(io::Error::other)?;
        let Some(relative) = safe_source_path(entry.name()) else {
            continue;
        };
        if is_generated_source(&relative) {
            excluded += 1;
            continue;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        let output = cache_root.join(&relative);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(output, String::from_utf8_lossy(&bytes).as_bytes())?;
        indexed += 1;
    }
    let mut complete = File::create(marker)?;
    writeln!(complete, "indexed={indexed}")?;
    writeln!(complete, "excluded={excluded}")?;
    Ok(CacheResult {
        root: source_root,
        indexed,
        excluded,
    })
}

fn marker_count(content: &str, key: &str) -> Option<usize> {
    content.lines().find_map(|line| {
        let (candidate, value) = line.split_once('=')?;
        (candidate == key).then(|| value.parse().ok()).flatten()
    })
}

fn cache_base() -> io::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = base
        .join("papyrus-language-server")
        .join("cache")
        .join("starfield");
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn safe_source_path(name: &str) -> Option<PathBuf> {
    let normalized = name.split('/').collect::<PathBuf>();
    let is_source = normalized
        .components()
        .take(2)
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .eq(["scripts".to_owned(), "source".to_owned()]);
    if !is_source
        || !normalized
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("psc"))
        || normalized.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    Some(normalized)
}

fn count_sources(root: &Path) -> usize {
    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                count_sources(&path)
            } else {
                usize::from(
                    path.extension()
                        .is_some_and(|value| value.eq_ignore_ascii_case("psc")),
                )
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    use zip::{ZipWriter, write::SimpleFileOptions};

    use super::materialize_at;

    #[test]
    fn extracts_retained_sources_and_filters_fragments() {
        let root = std::env::temp_dir().join(format!(
            "papyrus-cache-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let archive_path = root.join("ContentResources.zip");
        let file = fs::File::create(&archive_path).unwrap();
        let mut archive = ZipWriter::new(file);
        archive
            .start_file("Scripts/Source/Actor.psc", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"ScriptName Actor\n").unwrap();
        archive
            .start_file(
                "Scripts/Source/Fragments/Quests/QF_Test.psc",
                SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"ScriptName QF_Test\n").unwrap();
        archive.finish().unwrap();

        let result = materialize_at(&archive_path, &root.join("cache")).unwrap();
        assert_eq!(result.indexed, 1);
        assert_eq!(result.excluded, 1);
        assert!(result.root.join("Actor.psc").is_file());
        assert!(!result.root.join("Fragments/Quests/QF_Test.psc").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
