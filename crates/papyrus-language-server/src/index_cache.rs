use std::{
    collections::HashMap,
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use lsp_types::DocumentSymbol;
use serde::{Deserialize, Serialize};

use crate::{cache_paths::cache_directory, semantic::SemanticDocument};

const SCHEMA_VERSION: u32 = 5;
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;
const RETAINED_GENERATIONS: usize = 2;
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(crate) struct CachedDocument {
    pub(crate) symbols: Vec<DocumentSymbol>,
    pub(crate) semantic: SemanticDocument,
    pub(crate) content_hash: blake3::Hash,
}

#[derive(Default, Deserialize, Serialize)]
struct CacheFile {
    schema: u32,
    records: HashMap<String, CacheRecord>,
}

#[derive(Deserialize, Serialize)]
struct CacheRecord {
    size: u64,
    modified_nanos: u128,
    content_hash: String,
    semantic: SemanticDocument,
}

pub(crate) struct IndexCache {
    directory: Option<PathBuf>,
    file: CacheFile,
    dirty: bool,
    pub(crate) hits: usize,
    pub(crate) misses: usize,
}

impl IndexCache {
    pub(crate) fn load() -> Self {
        match cache_directory() {
            Ok(directory) => Self::load_from(Some(directory)),
            Err(error) => {
                eprintln!("papyrus-language-server: persistent semantic cache disabled: {error}");
                Self::load_from(None)
            }
        }
    }

    fn load_from(directory: Option<PathBuf>) -> Self {
        let file = directory
            .as_deref()
            .and_then(load_latest_generation)
            .unwrap_or_else(empty_cache_file);
        Self {
            directory,
            file,
            dirty: false,
            hits: 0,
            misses: 0,
        }
    }

    pub(crate) fn get(
        &mut self,
        path: &Path,
        expected_content_hash: blake3::Hash,
    ) -> Option<CachedDocument> {
        let Ok((size, modified_nanos)) = fingerprint(path) else {
            self.misses += 1;
            return None;
        };
        let key = path_key(path);
        let Some(record) = self.file.records.get(&key) else {
            self.misses += 1;
            return None;
        };
        if record.size != size || record.modified_nanos != modified_nanos {
            self.misses += 1;
            return None;
        }
        let Ok(content_hash) = blake3::Hash::from_hex(&record.content_hash) else {
            self.misses += 1;
            return None;
        };
        if content_hash != expected_content_hash {
            self.misses += 1;
            return None;
        }
        self.hits += 1;
        Some(CachedDocument {
            symbols: record.semantic.symbols.clone(),
            semantic: record.semantic.clone(),
            content_hash,
        })
    }

    pub(crate) fn insert(&mut self, path: &Path, document: &CachedDocument) {
        let Ok((size, modified_nanos)) = fingerprint(path) else {
            return;
        };
        self.file.records.insert(
            path_key(path),
            CacheRecord {
                size,
                modified_nanos,
                content_hash: document.content_hash.to_hex().to_string(),
                semantic: document.semantic.clone(),
            },
        );
        self.dirty = true;
    }

    pub(crate) fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let Some(directory) = self.directory.as_deref() else {
            self.dirty = false;
            return Ok(());
        };
        let generation = generation_id();
        let pending = directory.join(format!(
            ".semantic-index-v{SCHEMA_VERSION}-{generation}.pending"
        ));
        let published = directory.join(format!(
            "semantic-index-v{SCHEMA_VERSION}-{generation}.json"
        ));
        let bytes = serialize_cache_file(&self.file)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&pending)?;
        if let Err(error) = write_generation(&mut file, &bytes) {
            drop(file);
            let _ = fs::remove_file(&pending);
            return Err(error);
        }
        drop(file);
        if let Err(error) = fs::rename(&pending, &published) {
            let _ = fs::remove_file(&pending);
            return Err(error);
        }
        sync_directory(directory)?;
        prune_generations(directory, RETAINED_GENERATIONS);
        self.dirty = false;
        Ok(())
    }
}

fn fingerprint(path: &Path) -> io::Result<(u64, u128)> {
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok((metadata.len(), modified))
}

fn path_key(path: &Path) -> String {
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let mut key = String::new();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        key.push_str("unix:");
        for byte in path.as_os_str().as_bytes() {
            let _ = write!(key, "{byte:02x}");
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        key.push_str("windows:");
        for unit in path.as_os_str().encode_wide() {
            let _ = write!(key, "{unit:04x}");
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        key.push_str("other:");
        key.push_str(&path.to_string_lossy());
    }
    key
}

fn empty_cache_file() -> CacheFile {
    CacheFile {
        schema: SCHEMA_VERSION,
        records: HashMap::new(),
    }
}

fn serialize_cache_file(file: &CacheFile) -> io::Result<Vec<u8>> {
    let mut output = LimitedBuffer::new(MAX_CACHE_BYTES as usize);
    serde_json::to_writer(&mut output, file).map_err(io::Error::other)?;
    Ok(output.into_inner())
}

struct LimitedBuffer {
    bytes: Vec<u8>,
    limit: usize,
}

impl LimitedBuffer {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl io::Write for LimitedBuffer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.bytes.len().saturating_add(buffer.len()) > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "semantic index cache exceeds its size limit",
            ));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn generation_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
    format!(
        "{timestamp:032x}-{:08x}-{sequence:016x}",
        std::process::id()
    )
}

fn generation_paths(directory: &Path) -> Vec<PathBuf> {
    let prefix = format!("semantic-index-v{SCHEMA_VERSION}-");
    let mut paths = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            let name = entry.file_name();
            let name = name.to_str()?;
            (file_type.is_file() && is_generation_name(name, &prefix)).then(|| entry.path())
        })
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    paths
}

fn is_generation_name(name: &str, prefix: &str) -> bool {
    let Some(generation) = name
        .strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(".json"))
    else {
        return false;
    };
    let mut components = generation.split('-');
    matches!(
        (components.next(), components.next(), components.next(), components.next()),
        (Some(timestamp), Some(process), Some(sequence), None)
            if timestamp.len() == 32
                && process.len() == 8
                && sequence.len() == 16
                && timestamp.bytes().all(|byte| byte.is_ascii_hexdigit())
                && process.bytes().all(|byte| byte.is_ascii_hexdigit())
                && sequence.bytes().all(|byte| byte.is_ascii_hexdigit())
    )
}

fn load_latest_generation(directory: &Path) -> Option<CacheFile> {
    generation_paths(directory).into_iter().find_map(|path| {
        let metadata = fs::metadata(&path).ok()?;
        if metadata.len() > MAX_CACHE_BYTES {
            return None;
        }
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|file| file.schema == SCHEMA_VERSION)
    })
}

fn write_generation(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

fn prune_generations(directory: &Path, retained: usize) {
    for path in generation_paths(directory).into_iter().skip(retained) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        str::FromStr,
        sync::{Arc, Barrier},
        thread,
        time::SystemTime,
    };

    use lsp_types::Uri;

    use crate::semantic::SemanticExtractor;

    use super::{CachedDocument, IndexCache, fingerprint, generation_paths, path_key};

    #[test]
    fn rejects_cached_semantics_when_content_changed_but_metadata_matches() {
        let root = temp_root("content-hash");
        let path = root.join("Script.psc");
        let original = "ScriptName First\n";
        let changed = "ScriptName Other\n";
        assert_eq!(original.len(), changed.len());
        fs::write(&path, original).unwrap();
        let mut cache = IndexCache::load_from(Some(root.clone()));
        cache.insert(&path, &cached_document(original));

        fs::write(&path, changed).unwrap();
        let (size, modified_nanos) = fingerprint(&path).unwrap();
        let record = cache.file.records.get_mut(&path_key(&path)).unwrap();
        record.size = size;
        record.modified_nanos = modified_nanos;

        assert!(cache.get(&path, blake3::hash(changed.as_bytes())).is_none());
        assert_eq!(cache.misses, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publishes_immutable_generations_and_preserves_unowned_files() {
        let root = temp_root("generations");
        let path = root.join("Script.psc");
        fs::write(&path, "ScriptName First\n").unwrap();
        fs::write(root.join("unrelated.json"), "keep").unwrap();
        fs::write(
            root.join("semantic-index-v5-important.json"),
            "keep this too",
        )
        .unwrap();

        for index in 0..4 {
            let text = format!("ScriptName Script{index}\n");
            fs::write(&path, &text).unwrap();
            let mut cache = IndexCache::load_from(Some(root.clone()));
            cache.insert(&path, &cached_document(&text));
            cache.save().unwrap();
        }

        assert_eq!(generation_paths(&root).len(), 2);
        assert_eq!(
            fs::read_to_string(root.join("unrelated.json")).unwrap(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(root.join("semantic-index-v5-important.json")).unwrap(),
            "keep this too"
        );
        assert!(
            fs::read_dir(&root)
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".pending"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_writers_publish_parseable_unique_generations() {
        let root = temp_root("concurrent-generations");
        let barrier = Arc::new(Barrier::new(4));
        let workers = (0..4)
            .map(|index| {
                let root = root.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let path = root.join(format!("Script{index}.psc"));
                    let text = format!("ScriptName Script{index}\n");
                    fs::write(&path, &text).unwrap();
                    let mut cache = IndexCache::load_from(Some(root));
                    cache.insert(&path, &cached_document(&text));
                    barrier.wait();
                    cache.save().unwrap();
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            worker.join().unwrap();
        }

        let generations = generation_paths(&root);
        assert!(!generations.is_empty());
        for path in generations {
            let file: super::CacheFile = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert_eq!(file.schema, super::SCHEMA_VERSION);
        }
        assert!(
            fs::read_dir(&root)
                .unwrap()
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".pending"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persists_call_sites_without_persisting_source_text() {
        let root = temp_root("call-sites");
        let path = root.join("Script.psc");
        let text = concat!(
            "ScriptName Script\n",
            "Function Target(Int Value)\nEndFunction\n",
            "Function Test()\n  Target(1)\nEndFunction\n",
        );
        fs::write(&path, text).unwrap();
        let mut cache = IndexCache::load_from(Some(root.clone()));
        cache.insert(&path, &cached_document(text));
        cache.save().unwrap();

        let mut loaded = IndexCache::load_from(Some(root.clone()));
        let document = loaded.get(&path, blake3::hash(text.as_bytes())).unwrap();
        assert!(document.semantic.text.is_empty());
        assert_eq!(document.semantic.call_sites.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn path_keys_preserve_case_on_case_sensitive_platforms() {
        let root = temp_root("path-case");
        let upper = root.join("Case.psc");
        let lower = root.join("case.psc");
        fs::write(&upper, "ScriptName Upper\n").unwrap();
        fs::write(&lower, "ScriptName Lower\n").unwrap();
        assert_ne!(path_key(&upper), path_key(&lower));
        fs::remove_dir_all(root).unwrap();
    }

    fn cached_document(text: &str) -> CachedDocument {
        let mut extractor = SemanticExtractor::new().unwrap();
        let uri = Uri::from_str("file:///cache-test.psc").unwrap();
        let semantic = extractor.extract(uri, text);
        CachedDocument {
            symbols: semantic.symbols.clone(),
            semantic,
            content_hash: blake3::hash(text.as_bytes()),
        }
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "papyrus-index-cache-{label}-{}",
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
