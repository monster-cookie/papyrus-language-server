use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use zip::ZipArchive;

use crate::{cache_paths::cache_directory, source_filter::is_generated_source};

const MAX_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_SOURCE_FILES: usize = 50_000;
const MAX_SOURCE_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_SOURCE_DEPTH: usize = 64;
const STARFIELD_CACHE_VERSION: u32 = 1;
static NEXT_STAGING_DIRECTORY: AtomicU64 = AtomicU64::new(0);

pub(crate) struct CacheResult {
    pub(crate) root: PathBuf,
    pub(crate) indexed: usize,
    pub(crate) excluded: usize,
}

pub(crate) fn materialize_starfield_sources_with_cancel(
    archive_path: &Path,
    cancelled: &AtomicBool,
) -> io::Result<CacheResult> {
    materialize_at(archive_path, &cache_base()?, cancelled)
}

fn materialize_at(
    archive_path: &Path,
    base: &Path,
    cancelled: &AtomicBool,
) -> io::Result<CacheResult> {
    fs::create_dir_all(base)?;
    let (archive_file, fingerprint) = open_hashed_archive(archive_path, cancelled)?;
    let cache_root = base.join(format!("archive-v{STARFIELD_CACHE_VERSION}-{fingerprint}"));
    let source_root = cache_root.join("Scripts").join("Source");
    let marker = cache_root.join(".complete");
    if let Some(result) = completed_cache(&source_root, &marker, &fingerprint) {
        return Ok(result);
    }

    let staging = base.join(staging_name());
    fs::create_dir(&staging)?;
    if let Err(error) = fs::create_dir_all(staging.join("Scripts").join("Source")) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let extraction = extract_archive(archive_file, &staging, cancelled);
    let (indexed, excluded) = match extraction {
        Ok(counts) => counts,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if let Err(error) = write_marker(&staging, &fingerprint, indexed, excluded) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }

    match fs::rename(&staging, &cache_root) {
        Ok(()) => {}
        Err(error) if completed_cache(&source_root, &marker, &fingerprint).is_some() => {
            let _ = fs::remove_dir_all(&staging);
            let _ = error;
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
    }
    completed_cache(&source_root, &marker, &fingerprint).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "published Starfield cache is incomplete",
        )
    })
}

fn write_marker(
    staging: &Path,
    fingerprint: &str,
    indexed: usize,
    excluded: usize,
) -> io::Result<()> {
    let mut complete = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(staging.join(".complete"))?;
    writeln!(complete, "schema={STARFIELD_CACHE_VERSION}")?;
    writeln!(complete, "archive={fingerprint}")?;
    writeln!(complete, "indexed={indexed}")?;
    writeln!(complete, "excluded={excluded}")?;
    complete.sync_all()
}

fn extract_archive(
    archive_file: File,
    cache_root: &Path,
    cancelled: &AtomicBool,
) -> io::Result<(usize, usize)> {
    let mut archive = ZipArchive::new(archive_file).map_err(io::Error::other)?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(limit_error("archive entry count"));
    }
    let mut indexed = 0;
    let mut excluded = 0;
    let mut extracted_bytes = 0_u64;
    for index in 0..archive.len() {
        check_cancelled(cancelled)?;
        let mut entry = archive.by_index(index).map_err(io::Error::other)?;
        let Some(relative) = safe_source_path(entry.name()) else {
            continue;
        };
        if is_generated_source(&relative) {
            excluded += 1;
            continue;
        }
        if indexed >= MAX_SOURCE_FILES {
            return Err(limit_error("retained source count"));
        }
        if entry.size() > MAX_SOURCE_FILE_BYTES
            || extracted_bytes.saturating_add(entry.size()) > MAX_EXTRACTED_BYTES
        {
            return Err(limit_error("extracted source bytes"));
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry
            .by_ref()
            .take(MAX_SOURCE_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SOURCE_FILE_BYTES
            || extracted_bytes.saturating_add(bytes.len() as u64) > MAX_EXTRACTED_BYTES
        {
            return Err(limit_error("extracted source bytes"));
        }
        let decoded = String::from_utf8_lossy(&bytes);
        let output_bytes = decoded.as_bytes();
        if output_bytes.len() as u64 > MAX_SOURCE_FILE_BYTES
            || extracted_bytes.saturating_add(output_bytes.len() as u64) > MAX_EXTRACTED_BYTES
        {
            return Err(limit_error("materialized source bytes"));
        }
        let output = cache_root.join(&relative);
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        output_file.write_all(output_bytes)?;
        extracted_bytes += output_bytes.len() as u64;
        indexed += 1;
    }
    Ok((indexed, excluded))
}

fn open_hashed_archive(archive_path: &Path, cancelled: &AtomicBool) -> io::Result<(File, String)> {
    let metadata = fs::metadata(archive_path)?;
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(limit_error("archive size"));
    }
    let mut file = File::open(archive_path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        check_cancelled(cancelled)?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > MAX_ARCHIVE_BYTES {
            return Err(limit_error("archive size"));
        }
        hasher.update(&buffer[..read]);
    }
    file.rewind()?;
    Ok((file, hasher.finalize().to_hex().to_string()))
}

fn completed_cache(source_root: &Path, marker: &Path, fingerprint: &str) -> Option<CacheResult> {
    let content = read_marker(marker)?;
    if marker_value(&content, "schema")?.parse::<u32>().ok()? != STARFIELD_CACHE_VERSION
        || marker_value(&content, "archive")? != fingerprint
    {
        return None;
    }
    if !source_root.is_dir() {
        return None;
    }
    let indexed = marker_count(&content, "indexed")?;
    if count_sources(source_root)? != indexed {
        return None;
    }
    Some(CacheResult {
        root: source_root.to_owned(),
        indexed,
        excluded: marker_count(&content, "excluded")?,
    })
}

fn count_sources(root: &Path) -> Option<usize> {
    fn visit(path: &Path, depth: usize, entries: &mut usize, count: &mut usize) -> Option<()> {
        if depth > MAX_SOURCE_DEPTH || *entries > MAX_ARCHIVE_ENTRIES || *count > MAX_SOURCE_FILES {
            return None;
        }
        for entry in fs::read_dir(path).ok()?.flatten() {
            *entries += 1;
            if *entries > MAX_ARCHIVE_ENTRIES {
                return None;
            }
            let file_type = entry.file_type().ok()?;
            if file_type.is_dir() && !file_type.is_symlink() {
                visit(&entry.path(), depth + 1, entries, count)?;
            } else if file_type.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("psc"))
            {
                *count += 1;
                if *count > MAX_SOURCE_FILES {
                    return None;
                }
            }
        }
        Some(())
    }

    let mut entries = 0;
    let mut count = 0;
    visit(root, 0, &mut entries, &mut count)?;
    Some(count)
}

fn read_marker(path: &Path) -> Option<String> {
    const MAX_MARKER_BYTES: u64 = 4 * 1024;
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(MAX_MARKER_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_MARKER_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn marker_value<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    content.lines().find_map(|line| {
        let (candidate, value) = line.split_once('=')?;
        (candidate == key).then_some(value)
    })
}

fn marker_count(content: &str, key: &str) -> Option<usize> {
    content.lines().find_map(|line| {
        let (candidate, value) = line.split_once('=')?;
        (candidate == key).then(|| value.parse().ok()).flatten()
    })
}

fn cache_base() -> io::Result<PathBuf> {
    let path = cache_directory()?.join("starfield");
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn staging_name() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_STAGING_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    format!(
        ".staging-{timestamp:032x}-{:08x}-{sequence:016x}",
        std::process::id()
    )
}

fn check_cancelled(cancelled: &AtomicBool) -> io::Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Starfield source extraction cancelled",
        ))
    } else {
        Ok(())
    }
}

fn limit_error(limit: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("Starfield source {limit} exceeds the configured safety limit"),
    )
}

fn safe_source_path(name: &str) -> Option<PathBuf> {
    let normalized = name.split('/').collect::<PathBuf>();
    let is_source = normalized
        .components()
        .take(2)
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .eq(["scripts".to_owned(), "source".to_owned()]);
    if !is_source
        || normalized.components().count() > MAX_SOURCE_DEPTH
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };

    use zip::{ZipWriter, write::SimpleFileOptions};

    use std::sync::atomic::{AtomicBool, Ordering};

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

        let result =
            materialize_at(&archive_path, &root.join("cache"), &AtomicBool::new(false)).unwrap();
        assert_eq!(result.indexed, 1);
        assert_eq!(result.excluded, 1);
        assert!(result.root.join("Actor.psc").is_file());
        assert!(!result.root.join("Fragments/Quests/QF_Test.psc").exists());

        let file = fs::File::create(&archive_path).unwrap();
        let mut archive = ZipWriter::new(file);
        archive
            .start_file("Scripts/Source/Actor.psc", SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"ScriptName Different\n").unwrap();
        archive.finish().unwrap();
        let changed =
            materialize_at(&archive_path, &root.join("cache"), &AtomicBool::new(false)).unwrap();
        assert_ne!(result.root, changed.root);
        assert_eq!(
            fs::read_to_string(changed.root.join("Actor.psc")).unwrap(),
            "ScriptName Different\n"
        );
        let cancelled = AtomicBool::new(true);
        let error = match materialize_at(&archive_path, &root.join("cancelled"), &cancelled) {
            Ok(_) => panic!("pre-cancelled extraction should fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(cancelled.load(Ordering::Relaxed));
        fs::remove_dir_all(root).unwrap();
    }
}
