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
    response
        .text()
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
    response
        .text()
        .await
        .map_err(|err| PeriError::Tool(format!("web_fetch: failed to read body: {err}")))
}

fn build_http_client() -> PeriResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|err| PeriError::Tool(format!("failed to build http client: {err}")))
}

fn validate_http_url(url: &str) -> PeriResult<()> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(PeriError::Tool(format!(
            "web_fetch: only http(s) urls are supported, got {url}"
        )))
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
    fn validate_http_url_accepts_http_and_https() {
        assert!(validate_http_url("https://example.com").is_ok());
        assert!(validate_http_url("http://example.com").is_ok());
        assert!(validate_http_url("file:///etc/passwd").is_err());
        assert!(validate_http_url("javascript:alert(1)").is_err());
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
