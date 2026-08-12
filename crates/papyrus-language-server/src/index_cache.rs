use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use lsp_types::DocumentSymbol;
use serde::{Deserialize, Serialize};

use crate::semantic::SemanticDocument;

const SCHEMA_VERSION: u32 = 3;

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
    symbols: Vec<DocumentSymbol>,
    semantic: SemanticDocument,
}

pub(crate) struct IndexCache {
    path: PathBuf,
    file: CacheFile,
    dirty: bool,
    pub(crate) hits: usize,
    pub(crate) misses: usize,
}

impl IndexCache {
    pub(crate) fn load() -> Self {
        let path = cache_path();
        let file = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CacheFile>(&bytes).ok())
            .filter(|file| file.schema == SCHEMA_VERSION)
            .unwrap_or_else(|| CacheFile {
                schema: SCHEMA_VERSION,
                records: HashMap::new(),
            });
        Self {
            path,
            file,
            dirty: false,
            hits: 0,
            misses: 0,
        }
    }

    pub(crate) fn get(&mut self, path: &Path) -> Option<CachedDocument> {
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
        self.hits += 1;
        Some(CachedDocument {
            symbols: record.symbols.clone(),
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
                symbols: document.symbols.clone(),
                semantic: document.semantic.clone(),
            },
        );
        self.dirty = true;
    }

    pub(crate) fn save(&mut self) -> io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        fs::write(
            &temporary,
            serde_json::to_vec(&self.file).map_err(io::Error::other)?,
        )?;
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        fs::rename(temporary, &self.path)?;
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
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_owned())
        .to_string_lossy()
        .to_ascii_lowercase()
}

fn cache_path() -> PathBuf {
    #[cfg(test)]
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_CACHE: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "papyrus-language-server-test-index-{}-{}.json",
            std::process::id(),
            NEXT_CACHE.fetch_add(1, Ordering::Relaxed)
        ))
    }
    #[cfg(not(test))]
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("papyrus-language-server")
        .join("cache")
        .join("semantic-index-v3.json")
}
