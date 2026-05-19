use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use peridot_common::{McpServerConfig, McpTransport, PeriError, PeriResult};
use serde_json::{Value, json};

use crate::http::http_request;
use crate::stdio::stdio_request;
use crate::types::{McpCallResult, McpTool};

/// MCP client.
///
/// Caches the most recent `tools/list` response per `(server, ttl)`
/// pair so repeated lookups during the same process do not re-pay the
/// initialise + handshake cost. Cache TTL comes from
/// `McpServerConfig::schema_cache_seconds` (default 300s, `0` disables).
#[derive(Clone, Debug)]
pub struct McpClient {
    config: McpServerConfig,
    timeout: Duration,
    tools_cache: Arc<Mutex<Option<CachedTools>>>,
}

#[derive(Clone, Debug)]
struct CachedTools {
    tools: Vec<McpTool>,
    fetched_at: Instant,
    ttl: Duration,
}

impl McpClient {
    /// Creates an MCP client.
    pub fn new(config: McpServerConfig) -> Self {
        let timeout = Duration::from_secs(config.timeout_seconds.max(1));
        Self {
            config,
            timeout,
            tools_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Creates an MCP client with an explicit timeout.
    pub fn with_timeout(config: McpServerConfig, timeout: Duration) -> Self {
        Self {
            config,
            timeout,
            tools_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Returns server config.
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Initializes the server and lists exposed tools. Returns cached
    /// results when `schema_cache_seconds > 0` and the cache entry is
    /// fresh.
    pub async fn list_tools(&self) -> PeriResult<Vec<McpTool>> {
        if let Some(cached) = self.cached_tools_if_fresh() {
            return Ok(cached);
        }
        let result = self.request("tools/list", json!({}), 2).await?;
        let parsed = result.get("tools").cloned().unwrap_or_else(|| json!([]));
        let tools: Vec<McpTool> = serde_json::from_value(parsed)
            .map_err(|err| PeriError::Parse(format!("invalid MCP tools/list response: {err}")))?;
        self.store_tools_in_cache(&tools);
        Ok(tools)
    }

    fn cached_tools_if_fresh(&self) -> Option<Vec<McpTool>> {
        let cache = self.tools_cache.lock().ok()?;
        let entry = cache.as_ref()?;
        if entry.ttl.is_zero() {
            return None;
        }
        if entry.fetched_at.elapsed() < entry.ttl {
            return Some(entry.tools.clone());
        }
        None
    }

    fn store_tools_in_cache(&self, tools: &[McpTool]) {
        if self.config.schema_cache_seconds == 0 {
            return;
        }
        if let Ok(mut cache) = self.tools_cache.lock() {
            *cache = Some(CachedTools {
                tools: tools.to_vec(),
                fetched_at: Instant::now(),
                ttl: Duration::from_secs(self.config.schema_cache_seconds),
            });
        }
    }

    /// Drops any cached `tools/list` payload. Used by `mcp doctor` or
    /// when the operator explicitly refreshes a server.
    pub fn invalidate_tools_cache(&self) {
        if let Ok(mut cache) = self.tools_cache.lock() {
            *cache = None;
        }
    }

    /// Performs a lightweight health probe. Calls `tools/list` against
    /// the configured transport with a short timeout and returns the
    /// elapsed wall-clock time on success, or a structured error on
    /// failure. Callers (TUI status bar, `peridot mcp test`) display
    /// the result without caching it — health probes are explicit and
    /// should always hit the wire.
    pub async fn health_check(&self) -> PeriResult<Duration> {
        let started = Instant::now();
        let _ = self.request("tools/list", json!({}), 1).await?;
        Ok(started.elapsed())
    }

    /// Calls one MCP tool.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> PeriResult<McpCallResult> {
        let result = self
            .request(
                "tools/call",
                json!({
                    "name": name,
                    "arguments": arguments
                }),
                2,
            )
            .await?;
        serde_json::from_value(result)
            .map_err(|err| PeriError::Parse(format!("invalid MCP tools/call response: {err}")))
    }

    async fn request(&self, method: &str, params: Value, id: u64) -> PeriResult<Value> {
        match self.config.transport {
            McpTransport::Stdio => {
                stdio_request(&self.config, self.timeout, method, params, id).await
            }
            McpTransport::Http => {
                http_request(&self.config, self.timeout, method, params, id).await
            }
        }
    }
}
