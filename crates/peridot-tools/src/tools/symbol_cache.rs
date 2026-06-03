//! Persistent per-file symbol outline cache (feature F1).
//!
//! Split out of `tools/file.rs`: the in-process + on-disk store of parsed
//! symbol outlines, keyed by absolute path + (mtime, size). The filesystem
//! watcher and the parsing/tool code that *uses* this cache stay in
//! `tools/file.rs`; this module owns only the storage, freshness, and disk
//! (de)serialization so it can be reasoned about and tested in isolation.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

/// A single source symbol surfaced by the outline/search tools. Lives here
/// because the cache stores and round-trips it to disk.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct SymbolEntry {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) kind: String,
    pub(crate) name: String,
    /// Owning type/class for associated items (e.g. `Scanner` for
    /// `Scanner::scan`). Omitted for top-level symbols and for the
    /// line-based heuristic, which has no container information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) container: Option<String>,
    pub(crate) signature: String,
}

/// Cap on the number of files held in the per-file outline cache. When
/// exceeded the cache is cleared wholesale — crude but bounded, and the entries
/// rebuild lazily on the next query.
const OUTLINE_CACHE_MAX_FILES: usize = 8_192;

/// Disk schema version for the persisted symbol cache. Bump to invalidate old
/// caches when the entry shape changes.
const OUTLINE_CACHE_VERSION: u32 = 1;

/// Project-relative path of the persisted symbol cache.
const OUTLINE_CACHE_REL_PATH: &str = ".peridot/symbol-cache.json";

/// A cached per-file symbol outline, valid while the file's mtime and size are
/// unchanged. Persisted to disk so the index survives a daemon/agent restart.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct OutlineCacheEntry {
    /// Modification time as epoch milliseconds (portable across serde); `None`
    /// when the platform/file doesn't report one.
    mtime_ms: Option<u64>,
    size: u64,
    /// The full outline (no result limit), keyed below by absolute path.
    symbols: Vec<SymbolEntry>,
}

/// On-disk form of the cache: a version tag plus absolute-path-keyed entries.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct OutlineCacheDisk {
    version: u32,
    entries: HashMap<String, OutlineCacheEntry>,
}

/// In-process cache plus the disk-persistence bookkeeping.
#[derive(Default)]
struct OutlineCacheState {
    entries: HashMap<PathBuf, OutlineCacheEntry>,
    /// Where the cache is persisted, set from the first project root seen.
    disk_path: Option<PathBuf>,
    /// Whether the on-disk cache has been loaded this process.
    loaded: bool,
    /// Whether there are unsaved changes since the last flush.
    dirty: bool,
}

static OUTLINE_CACHE: LazyLock<Mutex<OutlineCacheState>> =
    LazyLock::new(|| Mutex::new(OutlineCacheState::default()));

/// Converts a stat mtime to portable epoch milliseconds.
pub(crate) fn mtime_to_millis(mtime: Option<SystemTime>) -> Option<u64> {
    mtime
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis() as u64)
}

/// Loads the persisted cache once per process, keyed off `project_root`. Disk
/// entries are merged in; a corrupt or missing file is treated as empty.
pub(crate) fn outline_cache_ensure_loaded(project_root: &Path) {
    let Ok(mut state) = OUTLINE_CACHE.lock() else {
        return;
    };
    if state.loaded {
        return;
    }
    state.loaded = true;
    let disk_path = project_root.join(OUTLINE_CACHE_REL_PATH);
    if let Some(entries) = outline_cache_read_disk(&disk_path) {
        for (key, entry) in entries {
            state.entries.entry(key).or_insert(entry);
        }
    }
    state.disk_path = Some(disk_path);
}

/// Reads and parses the persisted cache file, returning its entries when
/// present, parseable, and version-matched. Pure (no global state) so the
/// persistence format is unit-testable.
fn outline_cache_read_disk(path: &Path) -> Option<HashMap<PathBuf, OutlineCacheEntry>> {
    let text = fs::read_to_string(path).ok()?;
    let disk = serde_json::from_str::<OutlineCacheDisk>(&text).ok()?;
    if disk.version != OUTLINE_CACHE_VERSION {
        return None;
    }
    Some(
        disk.entries
            .into_iter()
            .map(|(key, entry)| (PathBuf::from(key), entry))
            .collect(),
    )
}

/// Serializes and writes the cache entries to `path`, creating the parent dir.
fn outline_cache_write_disk(
    path: &Path,
    entries: &HashMap<PathBuf, OutlineCacheEntry>,
) -> std::io::Result<()> {
    let disk = OutlineCacheDisk {
        version: OUTLINE_CACHE_VERSION,
        entries: entries
            .iter()
            .map(|(key, entry)| (key.to_string_lossy().into_owned(), entry.clone()))
            .collect(),
    };
    let text = serde_json::to_string(&disk).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
}

/// Returns the cached full outline for `path` when the cached mtime and size
/// still match.
pub(crate) fn outline_cache_get(
    path: &Path,
    mtime_ms: Option<u64>,
    size: u64,
) -> Option<Vec<SymbolEntry>> {
    let state = OUTLINE_CACHE.lock().ok()?;
    let entry = state.entries.get(path)?;
    if entry.size == size && entry.mtime_ms == mtime_ms {
        Some(entry.symbols.clone())
    } else {
        None
    }
}

/// Stores the full outline for `path` under its current mtime/size and marks
/// the cache dirty for the next flush.
pub(crate) fn outline_cache_put(
    path: &Path,
    mtime_ms: Option<u64>,
    size: u64,
    symbols: Vec<SymbolEntry>,
) {
    let Ok(mut state) = OUTLINE_CACHE.lock() else {
        return;
    };
    if state.entries.len() >= OUTLINE_CACHE_MAX_FILES && !state.entries.contains_key(path) {
        state.entries.clear();
    }
    state.entries.insert(
        path.to_path_buf(),
        OutlineCacheEntry {
            mtime_ms,
            size,
            symbols,
        },
    );
    state.dirty = true;
}

/// Writes the cache to disk when it has unsaved changes. Best-effort: a write
/// failure (e.g. read-only workspace) is ignored, the in-process cache stands.
/// Called at the end of a query so disk I/O is bounded to ~once per scan.
pub(crate) fn outline_cache_flush() {
    let Ok(mut state) = OUTLINE_CACHE.lock() else {
        return;
    };
    if !state.dirty {
        return;
    }
    let Some(disk_path) = state.disk_path.clone() else {
        return;
    };
    if outline_cache_write_disk(&disk_path, &state.entries).is_ok() {
        state.dirty = false;
    }
}

/// Drops cache entries for `paths` (e.g. on a filesystem change) and marks the
/// cache dirty so the next flush removes them from disk too.
pub(crate) fn outline_cache_invalidate<I, P>(paths: I)
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let Ok(mut state) = OUTLINE_CACHE.lock() else {
        return;
    };
    let mut changed = false;
    for path in paths {
        if state.entries.remove(path.as_ref()).is_some() {
            changed = true;
        }
    }
    if changed {
        state.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> OutlineCacheEntry {
        OutlineCacheEntry {
            mtime_ms: Some(1_234_567),
            size: 42,
            symbols: vec![SymbolEntry {
                path: "src/lib.rs".to_string(),
                line: 7,
                kind: "fn".to_string(),
                name: "demo".to_string(),
                container: Some("Widget".to_string()),
                signature: "fn demo()".to_string(),
            }],
        }
    }

    #[test]
    fn disk_cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("peridot-symcache-rt-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("symbol-cache.json");
        let key = dir.join("abs/src/lib.rs");

        let mut entries = HashMap::new();
        entries.insert(key.clone(), sample_entry());
        outline_cache_write_disk(&path, &entries).unwrap();

        let loaded = outline_cache_read_disk(&path).expect("readable");
        let entry = loaded.get(&key).expect("entry present");
        assert_eq!(entry.size, 42);
        assert_eq!(entry.mtime_ms, Some(1_234_567));
        assert_eq!(entry.symbols.len(), 1);
        assert_eq!(entry.symbols[0].name, "demo");
        assert_eq!(entry.symbols[0].container.as_deref(), Some("Widget"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn disk_cache_rejects_version_mismatch() {
        let dir = std::env::temp_dir().join(format!("peridot-symcache-ver-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("symbol-cache.json");
        fs::write(&path, r#"{"version":999,"entries":{}}"#).unwrap();
        assert!(
            outline_cache_read_disk(&path).is_none(),
            "a future cache version must be ignored, not misread"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn disk_cache_missing_file_is_none() {
        let missing = std::env::temp_dir().join("peridot-symcache-does-not-exist.json");
        let _ = fs::remove_file(&missing);
        assert!(outline_cache_read_disk(&missing).is_none());
    }

    #[test]
    fn invalidate_drops_matching_entries() {
        let kept = PathBuf::from(format!("/peridot-test-keep-{}.rs", std::process::id()));
        let dropped = PathBuf::from(format!("/peridot-test-drop-{}.rs", std::process::id()));
        outline_cache_put(&kept, Some(1), 10, Vec::new());
        outline_cache_put(&dropped, Some(1), 10, Vec::new());

        outline_cache_invalidate([dropped.clone()]);

        assert!(outline_cache_get(&kept, Some(1), 10).is_some());
        assert!(outline_cache_get(&dropped, Some(1), 10).is_none());
    }
}
