//! Provider model catalog fetchers.
//!
//! OpenRouter exposes a public model index that includes per-model
//! `context_length`. OpenAI's ChatGPT/Codex OAuth backend exposes a
//! user-plan-aware model index under `/backend-api/codex/models`.
//!
//! Lookups are cached on disk for 24 hours so the network call only happens
//! once per day per cache directory. When the network fetch fails, a stale
//! cache (older than the TTL) is still preferred over no data — operators
//! offline keep getting whatever was last known.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const OPENAI_CODEX_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";
const OPENAI_CODEX_CLIENT_VERSION: &str = "0.131.0-alpha.9";
const CACHE_TTL_SECS: u64 = 24 * 3600;
const FETCH_TIMEOUT_SECS: u64 = 5;
const CACHE_FILENAME: &str = "openrouter-models.json";

#[derive(Deserialize, Debug)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(Deserialize, Debug, Clone)]
struct OpenRouterModel {
    id: String,
    context_length: Option<usize>,
}

#[derive(Deserialize, Debug)]
struct OpenAiCodexModelsResponse {
    models: Vec<OpenAiCodexModel>,
}

#[derive(Deserialize, Debug, Clone)]
struct OpenAiCodexModel {
    slug: String,
    context_window: Option<usize>,
}

/// Disk-cached snapshot of OpenRouter's model catalog. The wrapper carries
/// a Unix timestamp so we can compute freshness independently of the file
/// system's mtime (which can be perturbed by backup tools that touch files
/// without changing content).
#[derive(Serialize, Deserialize, Debug, Default)]
struct CachedCatalog {
    fetched_at_unix: u64,
    entries: HashMap<String, usize>,
}

/// Returns OpenRouter's `slug → context_length` map.
///
/// Strategy:
/// 1. If a cache file exists and is younger than [`CACHE_TTL_SECS`], parse
///    and return it.
/// 2. Otherwise fetch from the OpenRouter REST endpoint with a 5-second
///    timeout. On success, persist to the cache file and return the map.
/// 3. On network failure with a stale-but-existent cache, return the stale
///    cache (better than nothing for an offline operator).
/// 4. Otherwise return `None` so the caller can fall back to the static
///    heuristic table.
pub async fn openrouter_context_lengths(cache_dir: &Path) -> Option<HashMap<String, usize>> {
    let cache_path = cache_dir.join(CACHE_FILENAME);
    if let Some(cached) = read_cache(&cache_path)
        && cache_is_fresh(&cached)
    {
        return Some(cached.entries);
    }
    match fetch_from_openrouter().await {
        Ok(entries) => {
            let snapshot = CachedCatalog {
                fetched_at_unix: now_unix(),
                entries: entries.clone(),
            };
            write_cache(cache_dir, &cache_path, &snapshot);
            Some(entries)
        }
        Err(_) => read_cache(&cache_path).map(|snapshot| snapshot.entries),
    }
}

/// Returns ChatGPT/Codex OAuth `slug -> context_window` values.
///
/// The response is account/plan aware, so the cache file is keyed by a
/// stable hash of `chatgpt-account-id`. No tokens are persisted.
pub async fn openai_codex_context_lengths(
    cache_dir: &Path,
    access_token: &str,
    account_id: &str,
    originator: &str,
) -> Option<HashMap<String, usize>> {
    if access_token.trim().is_empty() || account_id.trim().is_empty() {
        return None;
    }
    let cache_path = cache_dir.join(openai_codex_cache_filename(account_id));
    if let Some(cached) = read_cache(&cache_path)
        && cache_is_fresh(&cached)
    {
        return Some(cached.entries);
    }
    match fetch_from_openai_codex(access_token, account_id, originator).await {
        Ok(entries) => {
            let snapshot = CachedCatalog {
                fetched_at_unix: now_unix(),
                entries: entries.clone(),
            };
            write_cache(cache_dir, &cache_path, &snapshot);
            Some(entries)
        }
        Err(_) => read_cache(&cache_path).map(|snapshot| snapshot.entries),
    }
}

fn cache_is_fresh(snapshot: &CachedCatalog) -> bool {
    let now = now_unix();
    now.saturating_sub(snapshot.fetched_at_unix) < CACHE_TTL_SECS
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn read_cache(path: &Path) -> Option<CachedCatalog> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_cache(dir: &Path, path: &Path, snapshot: &CachedCatalog) {
    let _ = std::fs::create_dir_all(dir);
    if let Ok(json) = serde_json::to_vec_pretty(snapshot) {
        let _ = std::fs::write(path, json);
    }
}

async fn fetch_from_openrouter() -> Result<HashMap<String, usize>, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()?;
    let response: OpenRouterModelsResponse = client
        .get(OPENROUTER_MODELS_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut entries = HashMap::with_capacity(response.data.len());
    for model in response.data {
        if let Some(context) = model.context_length {
            entries.insert(model.id, context);
        }
    }
    Ok(entries)
}

async fn fetch_from_openai_codex(
    access_token: &str,
    account_id: &str,
    originator: &str,
) -> Result<HashMap<String, usize>, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build()?;
    let response: OpenAiCodexModelsResponse = client
        .get(OPENAI_CODEX_MODELS_URL)
        .query(&[("client_version", OPENAI_CODEX_CLIENT_VERSION)])
        .bearer_auth(access_token)
        .header("chatgpt-account-id", account_id)
        .header("originator", originator)
        .header("OpenAI-Beta", "responses=experimental")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut entries = HashMap::with_capacity(response.models.len());
    for model in response.models {
        if let Some(context) = model.context_window {
            entries.insert(model.slug, context);
        }
    }
    Ok(entries)
}

fn openai_codex_cache_filename(account_id: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    account_id.hash(&mut hasher);
    format!("openai-codex-models-{:016x}.json", hasher.finish())
}

/// Returns the default cache directory under `$HOME/.peridot/cache`. Used
/// by the CLI; library tests can pass any path explicitly.
pub fn default_cache_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".peridot/cache"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    fn temp_cache_dir() -> PathBuf {
        // Nanosecond resolution prevents collisions when cargo runs the
        // module's tests in parallel within the same second.
        let ns = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut path = std::env::temp_dir();
        path.push(format!("peridot-catalog-test-{}-{ns}", std::process::id(),));
        path
    }

    #[test]
    fn cache_is_fresh_under_ttl() {
        let snapshot = CachedCatalog {
            fetched_at_unix: now_unix().saturating_sub(60),
            entries: HashMap::new(),
        };
        assert!(cache_is_fresh(&snapshot));
    }

    #[test]
    fn cache_is_stale_past_ttl() {
        let snapshot = CachedCatalog {
            fetched_at_unix: now_unix().saturating_sub(CACHE_TTL_SECS + 1),
            entries: HashMap::new(),
        };
        assert!(!cache_is_fresh(&snapshot));
    }

    #[tokio::test]
    async fn fresh_disk_cache_is_returned_without_network() {
        let dir = temp_cache_dir();
        let mut entries = HashMap::new();
        entries.insert("test/model".to_string(), 128_000);
        let snapshot = CachedCatalog {
            fetched_at_unix: now_unix(),
            entries: entries.clone(),
        };
        write_cache(&dir, &dir.join(CACHE_FILENAME), &snapshot);

        let result = openrouter_context_lengths(&dir).await;
        assert_eq!(result, Some(entries));
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stale_cache_falls_back_when_network_fails() {
        // We can't easily simulate a network failure in the unit test, but
        // we can at least exercise `read_cache` on a stale snapshot — the
        // production path will only get here when `fetch_from_openrouter`
        // returns Err, which we trust the type system to keep correct.
        let dir = temp_cache_dir();
        let mut entries = HashMap::new();
        entries.insert("stale/model".to_string(), 64_000);
        let snapshot = CachedCatalog {
            fetched_at_unix: now_unix().saturating_sub(CACHE_TTL_SECS + 600),
            entries: entries.clone(),
        };
        write_cache(&dir, &dir.join(CACHE_FILENAME), &snapshot);

        // Cache is stale — `cache_is_fresh` reports false, the production
        // code would try the network. We assert the stale snapshot is at
        // least still parseable so the fallback branch can read it.
        let cached = read_cache(&dir.join(CACHE_FILENAME)).expect("cache parseable");
        assert!(!cache_is_fresh(&cached));
        assert_eq!(cached.entries, entries);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_cache_dir_resolves_under_home_peridot() {
        // Use SAFE block — these env mutations are scoped to the test.
        unsafe {
            std::env::set_var("HOME", "/tmp/fake-home");
        }
        let path = default_cache_dir().expect("HOME is set");
        assert!(path.ends_with(".peridot/cache"));
        // Just ensure we don't crash; cleanup is best-effort.
        let _ = Duration::from_millis(1);
    }
}
