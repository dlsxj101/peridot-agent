//! Web search and page-fetch tools.
//!
//! Provides two read-only, zero-config tools the agent can call when it
//! needs information that isn't in the workspace:
//!
//! - `web_search`: DuckDuckGo HTML scrape. Returns `{title, url, snippet}`
//!   list. No API key, no service to run.
//! - `web_fetch`: HTTP GET a URL and return stripped, whitespace-collapsed
//!   text, capped at 50k chars so a single fetch can't blow the parent
//!   context.
//!
//! Typical pairing: the model calls `web_search` to discover URLs, then
//! `web_fetch` on the most relevant hit to read the page body. Each tool
//! is `PermissionLevel::Read` — auto-approved under `Auto` permission.

use std::net::{IpAddr, ToSocketAddrs};
use std::time::Duration;

use async_trait::async_trait;
use peridot_common::{PeriError, PeriResult, PermissionLevel, ToolGroup, ToolResult};
use serde_json::{Value, json};

use crate::path::required_str;
use crate::{Tool, ToolContext};

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) peridot-agent/0.2 Safari/537.36";
const DDG_HTML_URL: &str = "https://html.duckduckgo.com/html/";
const DEFAULT_MAX_RESULTS: usize = 10;
const MAX_RESULTS_CAP: u64 = 25;
const FETCH_MAX_CHARS: usize = 50_000;
const REQUEST_TIMEOUT_SECS: u64 = 15;
// Hard ceiling on bytes buffered from a response body. The model fully controls
// the fetched URL, so an unbounded `response.text()` is an OOM/DoS vector — read
// chunk-by-chunk and stop once this many bytes have accumulated.
const MAX_FETCH_BYTES: usize = 5 * 1024 * 1024;

/// `web_search` — DuckDuckGo HTML scrape returning title/url/snippet triples.
#[derive(Clone, Debug)]
pub struct WebSearchTool;

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Web
    }

    fn description(&self) -> &str {
        "Search the web via DuckDuckGo. Returns a list of {title, url, snippet}. \
         Pair with `web_fetch` to read full page contents."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Max results to return (default 10, capped at 25)",
                    "minimum": 1,
                    "maximum": MAX_RESULTS_CAP
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> PeriResult<ToolResult> {
        let query = required_str(&params, "query")?;
        let max_results = params
            .get("max_results")
            .and_then(Value::as_u64)
            .map(|n| (n.clamp(1, MAX_RESULTS_CAP)) as usize)
            .unwrap_or(DEFAULT_MAX_RESULTS);

        let html = ddg_fetch_html(query).await?;
        let results = parse_ddg_results(&html, max_results);
        let summary = if results.is_empty() {
            format!("web_search: no results for {query:?}")
        } else {
            format!("web_search: {} results for {:?}", results.len(), query)
        };
        let output = json!({
            "query": query,
            "results": results.iter().map(SearchResult::to_json).collect::<Vec<_>>(),
        });
        Ok(ToolResult::success(summary, output))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }

    fn risk_class(&self) -> peridot_common::RiskClass {
        // Reads only, but reaches the public internet — risk surface is
        // data exfiltration / supply-chain via redirects, not local
        // mutation. Class it as external network so policies can opt
        // network-touching tools into prompt-on-use regardless of read
        // semantics.
        peridot_common::RiskClass::ExternalNetwork
    }
}

/// `web_fetch` — HTTP GET a URL and return stripped readable text.
#[derive(Clone, Debug)]
pub struct WebFetchTool;

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn group(&self) -> ToolGroup {
        ToolGroup::Web
    }

    fn description(&self) -> &str {
        "Fetch a URL and return its main text content with HTML/scripts stripped. \
         Output is capped at 50000 chars."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "HTTP(S) URL to fetch"
                }
            },
            "required": ["url"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, params: Value, _ctx: &ToolContext) -> PeriResult<ToolResult> {
        let url = required_str(&params, "url")?;
        validate_http_url(url)?;
        let body = http_fetch_text(url).await?;
        let cleaned = html_to_text(&body);
        let truncated = truncate_chars(&cleaned, FETCH_MAX_CHARS);
        let summary = format!("web_fetch: {} chars from {url}", truncated.chars().count());
        Ok(ToolResult::success(
            summary,
            json!({
                "url": url,
                "content": truncated,
            }),
        ))
    }

    fn permission_level(&self) -> PermissionLevel {
        PermissionLevel::Read
    }

    fn risk_class(&self) -> peridot_common::RiskClass {
        peridot_common::RiskClass::ExternalNetwork
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

impl SearchResult {
    fn to_json(&self) -> Value {
        json!({
            "title": self.title,
            "url": self.url,
            "snippet": self.snippet,
        })
    }
}

async fn ddg_fetch_html(query: &str) -> PeriResult<String> {
    let client = build_http_client()?;
    let response = client
        .post(DDG_HTML_URL)
        .form(&[("q", query), ("kl", "wt-wt")])
        .send()
        .await
        .map_err(|err| PeriError::Tool(format!("web_search: DDG request failed: {err}")))?;
    if !response.status().is_success() {
        return Err(PeriError::Tool(format!(
            "web_search: DDG returned status {}",
            response.status()
        )));
    }
    read_body_capped(response, MAX_FETCH_BYTES)
        .await
        .map_err(|err| PeriError::Tool(format!("web_search: failed to read DDG response: {err}")))
}

async fn http_fetch_text(url: &str) -> PeriResult<String> {
    let client = build_http_client()?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| PeriError::Tool(format!("web_fetch: request failed: {err}")))?;
    if !response.status().is_success() {
        return Err(PeriError::Tool(format!(
            "web_fetch: {url} returned status {}",
            response.status()
        )));
    }
    read_body_capped(response, MAX_FETCH_BYTES)
        .await
        .map_err(|err| PeriError::Tool(format!("web_fetch: failed to read body: {err}")))
}

/// Reads a response body chunk-by-chunk, stopping once `max_bytes` have
/// accumulated, so a model-controlled URL can't force an unbounded download
/// into memory. Returns lossy UTF-8 (bodies are fed to text extraction).
async fn read_body_capped(response: reqwest::Response, max_bytes: usize) -> PeriResult<String> {
    let mut response = response;
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| PeriError::Tool(format!("failed to read response chunk: {err}")))?
    {
        if buf.len() >= max_bytes {
            break;
        }
        let take = chunk.len().min(max_bytes - buf.len());
        buf.extend_from_slice(&chunk[..take]);
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn build_http_client() -> PeriResult<reqwest::Client> {
    // reqwest follows redirects by default, which would let a public URL
    // bounce us to an internal one (metadata/loopback/RFC1918) after the
    // initial `validate_http_url` check. Re-validate every redirect hop's
    // host and refuse to follow any that resolves to an internal address.
    let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("web_fetch: too many redirects");
        }
        let url = attempt.url().as_str();
        match validate_http_url(url) {
            Ok(()) => attempt.follow(),
            Err(err) => attempt.error(std::io::Error::other(err.to_string())),
        }
    });
    reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .redirect(redirect_policy)
        .build()
        .map_err(|err| PeriError::Tool(format!("failed to build http client: {err}")))
}

fn validate_http_url(url: &str) -> PeriResult<()> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(PeriError::Tool(format!(
            "web_fetch: only http(s) urls are supported, got {url}"
        )));
    }
    // SSRF guard: resolve the host and reject any address that points at
    // the host's own network — cloud metadata (169.254.169.254), loopback,
    // link-local, private (RFC1918 / fc00::/7), and unspecified addresses.
    // Without this, a model-controlled URL can read instance credentials or
    // probe internal services.
    let host = host_from_url(url).ok_or_else(|| {
        PeriError::Tool(format!("web_fetch: could not parse host from url {url}"))
    })?;
    let addrs = resolve_host(&host)?;
    if addrs.is_empty() {
        return Err(PeriError::Tool(format!(
            "web_fetch: host {host} did not resolve to any address"
        )));
    }
    for addr in &addrs {
        if is_blocked_addr(addr) {
            return Err(PeriError::Tool(format!(
                "web_fetch: refusing to fetch internal address {addr} (host {host})"
            )));
        }
    }
    Ok(())
}

/// Extracts the host portion (without port / userinfo) from an http(s) URL.
fn host_from_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    // Authority ends at the first '/', '?', or '#'.
    let authority_end = rest
        .find(['/', '?', '#'])
        .unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    // Strip optional userinfo (user:pass@host).
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if host_port.is_empty() {
        return None;
    }
    // IPv6 literal: [::1]:port
    if let Some(after) = host_port.strip_prefix('[') {
        let end = after.find(']')?;
        return Some(after[..end].to_string());
    }
    // host[:port]
    let host = host_port.split(':').next().unwrap_or(host_port);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Resolves a host to its IP addresses. An IP literal resolves to itself
/// (no DNS); a name is resolved via the system resolver. The dummy port is
/// only there to satisfy `ToSocketAddrs`.
fn resolve_host(host: &str) -> PeriResult<Vec<IpAddr>> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }
    let addrs = (host, 0u16)
        .to_socket_addrs()
        .map_err(|err| PeriError::Tool(format!("web_fetch: failed to resolve {host}: {err}")))?
        .map(|sa| sa.ip())
        .collect();
    Ok(addrs)
}

/// Returns true for addresses that must never be fetched: loopback,
/// link-local (169.254/16, fe80::/10), private (10/8, 172.16/12,
/// 192.168/16, fc00::/7), and unspecified (0.0.0.0 / ::).
fn is_blocked_addr(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                // Carrier-grade NAT 100.64.0.0/10 and the rest of
                // 0.0.0.0/8 ("this network") are also non-routable.
                || v4.octets()[0] == 0
                || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // Link-local fe80::/10.
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // Unique local fc00::/7.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // IPv4-mapped (::ffff:a.b.c.d) — recheck the embedded v4.
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| is_blocked_addr(&IpAddr::V4(v4)))
                    .unwrap_or(false)
        }
    }
}

fn parse_ddg_results(html: &str, max_results: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut cursor = 0;
    while results.len() < max_results {
        let Some(rel_a) = find_ignore_case(&html[cursor..], "class=\"result__a\"") else {
            break;
        };
        let abs_a_class = cursor + rel_a;
        let Some(a_open) = html[..abs_a_class].rfind("<a ") else {
            break;
        };
        let Some(tag_close_rel) = html[a_open..].find('>') else {
            break;
        };
        let tag_close = a_open + tag_close_rel;
        let opening = &html[a_open..=tag_close];
        let Some(a_close_rel) = find_ignore_case(&html[tag_close..], "</a>") else {
            break;
        };
        let a_close = tag_close + a_close_rel;
        let inner_title = &html[tag_close + 1..a_close];

        let href = extract_attr(opening, "href").unwrap_or_default();
        let url = unwrap_ddg_redirect(&href);
        let title = decode_html_entities(&strip_tags(inner_title))
            .trim()
            .to_string();

        cursor = a_close + 4;

        let snippet = extract_snippet_after(html, &mut cursor);

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }
    results
}

fn extract_snippet_after(html: &str, cursor: &mut usize) -> String {
    let Some(rel) = find_ignore_case(&html[*cursor..], "class=\"result__snippet\"") else {
        return String::new();
    };
    let abs_class = *cursor + rel;
    // Snippet may also be rendered as <span class="result__snippet">…</span>
    // on some DDG variants; reach back to the nearest opening tag instead of
    // assuming `<a `.
    let Some(open) = html[..abs_class].rfind('<') else {
        return String::new();
    };
    let Some(tag_close_rel) = html[open..].find('>') else {
        return String::new();
    };
    let tag_close = open + tag_close_rel;
    let tag_name = tag_name_of(&html[open..=tag_close]);
    let closing = format!("</{tag_name}>");
    let Some(close_rel) = find_ignore_case(&html[tag_close..], &closing) else {
        return String::new();
    };
    let close_abs = tag_close + close_rel;
    let inner = &html[tag_close + 1..close_abs];
    *cursor = close_abs + closing.len();
    decode_html_entities(&strip_tags(inner)).trim().to_string()
}

fn tag_name_of(opening: &str) -> String {
    let trimmed = opening
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim_start_matches('/');
    trimmed
        .split(|c: char| c.is_whitespace() || c == '>')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn extract_attr(opening: &str, attr: &str) -> Option<String> {
    let needle = format!("{attr}=\"");
    let start = opening.find(&needle)? + needle.len();
    let rest = &opening[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn unwrap_ddg_redirect(href: &str) -> String {
    // DDG wraps results as //duckduckgo.com/l/?uddg=<encoded>&rut=...
    if let Some(idx) = href.find("uddg=") {
        let after = &href[idx + 5..];
        let end = after.find('&').unwrap_or(after.len());
        return url_decode(&after[..end]);
    }
    if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_string()
    }
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut buf = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            buf.push((h << 4) | l);
            i += 3;
            continue;
        }
        if b == b'+' {
            buf.push(b' ');
        } else {
            buf.push(b);
        }
        i += 1;
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

fn html_to_text(html: &str) -> String {
    let cleaned = strip_block_ci(html, "<script", "</script>");
    let cleaned = strip_block_ci(&cleaned, "<style", "</style>");
    let cleaned = strip_block_ci(&cleaned, "<noscript", "</noscript>");
    let cleaned = strip_block_ci(&cleaned, "<svg", "</svg>");
    let cleaned = strip_block_ci(&cleaned, "<!--", "-->");

    let mut out = String::with_capacity(cleaned.len());
    let mut in_tag = false;
    let mut tag_buf = String::new();
    for c in cleaned.chars() {
        match c {
            '<' => {
                in_tag = true;
                tag_buf.clear();
            }
            '>' if in_tag => {
                in_tag = false;
                if breaks_to_newline(&tag_buf) {
                    out.push('\n');
                }
            }
            _ if in_tag => tag_buf.push(c),
            _ => out.push(c),
        }
    }
    let decoded = decode_html_entities(&out);
    collapse_whitespace(&decoded)
}

fn breaks_to_newline(tag_buf: &str) -> bool {
    let name = tag_buf
        .trim_start_matches('/')
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "p" | "br"
            | "div"
            | "li"
            | "tr"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "nav"
            | "main"
            | "aside"
            | "blockquote"
            | "pre"
            | "ul"
            | "ol"
            | "table"
    )
}

fn strip_block_ci(haystack: &str, start: &str, end: &str) -> String {
    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0;
    while cursor < haystack.len() {
        match find_ignore_case(&haystack[cursor..], start) {
            None => break,
            Some(s) => {
                let s_abs = cursor + s;
                out.push_str(&haystack[cursor..s_abs]);
                match find_ignore_case(&haystack[s_abs..], end) {
                    None => {
                        cursor = haystack.len();
                        break;
                    }
                    Some(e) => cursor = s_abs + e + end.len(),
                }
            }
        }
    }
    if cursor < haystack.len() {
        out.push_str(&haystack[cursor..]);
    }
    out
}

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    let n = needle.as_bytes();
    if n.is_empty() {
        return Some(0);
    }
    let h = haystack.as_bytes();
    if h.len() < n.len() {
        return None;
    }
    'outer: for i in 0..=h.len() - n.len() {
        for j in 0..n.len() {
            if !h[i + j].eq_ignore_ascii_case(&n[j]) {
                continue 'outer;
            }
        }
        return Some(i);
    }
    None
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut consecutive_newlines = 0usize;
    let mut last_was_space = false;
    for c in s.chars() {
        if c == '\n' {
            if consecutive_newlines < 2 {
                out.push('\n');
            }
            consecutive_newlines += 1;
            last_was_space = false;
        } else if c.is_whitespace() {
            if !last_was_space && consecutive_newlines == 0 {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(c);
            consecutive_newlines = 0;
            last_was_space = false;
        }
    }
    out.trim().to_string()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push_str("\n\n[truncated]");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_decode_handles_percent_and_plus() {
        assert_eq!(url_decode("hello+world"), "hello world");
        assert_eq!(url_decode("a%2Fb%3Fc%3Dd"), "a/b?c=d");
        assert_eq!(
            url_decode("https%3A%2F%2Fexample.com%2Fpath"),
            "https://example.com/path"
        );
    }

    #[test]
    fn unwrap_ddg_redirect_extracts_target() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpath&rut=abc";
        assert_eq!(unwrap_ddg_redirect(href), "https://example.com/path");
    }

    #[test]
    fn unwrap_ddg_redirect_passes_through_direct_urls() {
        assert_eq!(
            unwrap_ddg_redirect("https://direct.example/"),
            "https://direct.example/"
        );
        assert_eq!(
            unwrap_ddg_redirect("//cdn.example/"),
            "https://cdn.example/"
        );
    }

    #[test]
    fn strip_tags_removes_html_markup() {
        assert_eq!(strip_tags("<b>hello</b> <i>world</i>"), "hello world");
        assert_eq!(strip_tags("plain"), "plain");
    }

    #[test]
    fn decode_html_entities_handles_common_escapes() {
        assert_eq!(decode_html_entities("a&amp;b"), "a&b");
        assert_eq!(decode_html_entities("&lt;tag&gt;"), "<tag>");
        assert_eq!(decode_html_entities("it&#39;s"), "it's");
    }

    #[test]
    fn html_to_text_strips_scripts_and_styles() {
        let html = "<html><head><style>body { color: red; }</style>\
                    <script>alert('x')</script></head>\
                    <body><h1>Title</h1><p>Paragraph one.</p>\
                    <p>Paragraph two.</p></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("Title"));
        assert!(text.contains("Paragraph one."));
        assert!(text.contains("Paragraph two."));
        assert!(!text.contains("alert"));
        assert!(!text.contains("color: red"));
    }

    #[test]
    fn html_to_text_collapses_whitespace() {
        let html = "<div>a   b\n\n\n\nc</div>";
        let text = html_to_text(html);
        assert!(!text.contains("    "));
        assert!(!text.contains("\n\n\n"));
    }

    #[test]
    fn truncate_chars_caps_output() {
        let s = "a".repeat(100);
        let out = truncate_chars(&s, 50);
        assert!(out.starts_with(&"a".repeat(50)));
        assert!(out.ends_with("[truncated]"));
    }

    #[test]
    fn truncate_chars_passthrough_when_short() {
        assert_eq!(truncate_chars("short", 50), "short");
    }

    #[test]
    fn find_ignore_case_matches_case_insensitively() {
        assert_eq!(find_ignore_case("Hello World", "world"), Some(6));
        assert_eq!(find_ignore_case("<SCRIPT>x", "<script"), Some(0));
        assert_eq!(find_ignore_case("abc", "xyz"), None);
    }

    #[test]
    fn validate_http_url_rejects_non_http_schemes() {
        assert!(validate_http_url("file:///etc/passwd").is_err());
        assert!(validate_http_url("javascript:alert(1)").is_err());
        assert!(validate_http_url("ftp://example.com").is_err());
    }

    #[test]
    fn host_from_url_extracts_host() {
        assert_eq!(host_from_url("http://example.com/path").as_deref(), Some("example.com"));
        assert_eq!(host_from_url("https://example.com:8443/").as_deref(), Some("example.com"));
        assert_eq!(host_from_url("http://user:pw@example.com/").as_deref(), Some("example.com"));
        assert_eq!(host_from_url("http://[::1]:80/").as_deref(), Some("::1"));
        assert_eq!(host_from_url("http://169.254.169.254/").as_deref(), Some("169.254.169.254"));
    }

    #[test]
    fn validate_http_url_blocks_ssrf_targets() {
        // IP literals never hit DNS, so these are deterministic.
        assert!(validate_http_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(validate_http_url("http://127.0.0.1/").is_err());
        assert!(validate_http_url("http://localhost/").is_err());
        assert!(validate_http_url("http://10.0.0.5/").is_err());
        assert!(validate_http_url("http://172.16.0.1/").is_err());
        assert!(validate_http_url("http://192.168.1.1/").is_err());
        assert!(validate_http_url("http://[::1]/").is_err());
        assert!(validate_http_url("http://0.0.0.0/").is_err());
    }

    #[test]
    fn is_blocked_addr_classifies_internal_ranges() {
        use std::net::{Ipv4Addr, Ipv6Addr};
        assert!(is_blocked_addr(&IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
        assert!(is_blocked_addr(&IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))));
        assert!(is_blocked_addr(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_blocked_addr(&IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
        assert!(is_blocked_addr(&IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_blocked_addr(&IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(is_blocked_addr(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_blocked_addr(&IpAddr::V6("fe80::1".parse().unwrap())));
        assert!(is_blocked_addr(&IpAddr::V6("fc00::1".parse().unwrap())));
        // Public addresses pass.
        assert!(!is_blocked_addr(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_blocked_addr(&IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
    }

    #[test]
    fn parse_ddg_results_extracts_title_url_snippet() {
        let html = r##"
        <div class="result">
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%2F&rut=z">
                The Rust Programming Language
            </a>
            <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%2F&rut=z">
                A language empowering everyone to build reliable software.
            </a>
        </div>
        <div class="result">
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2F&rut=z">
                The Rust Book
            </a>
            <a class="result__snippet" href="">
                <b>Rust</b> programming language tutorial.
            </a>
        </div>
        "##;
        let results = parse_ddg_results(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "The Rust Programming Language");
        assert_eq!(results[0].url, "https://rust-lang.org/");
        assert!(results[0].snippet.contains("reliable software"));
        assert_eq!(results[1].title, "The Rust Book");
        assert_eq!(results[1].url, "https://doc.rust-lang.org/book/");
        assert!(results[1].snippet.contains("Rust"));
    }

    #[test]
    fn parse_ddg_results_respects_max_results_cap() {
        let mut html = String::new();
        for i in 0..5 {
            html.push_str(&format!(
                r##"<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fex{i}.com%2F&rut=z">Title {i}</a>
                    <a class="result__snippet">Snippet {i}</a>"##
            ));
        }
        let results = parse_ddg_results(&html, 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn parse_ddg_results_returns_empty_on_unrecognized_html() {
        assert!(parse_ddg_results("<html><body>no results</body></html>", 10).is_empty());
    }
}
