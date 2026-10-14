//! The `web_search` tool: a web search through DuckDuckGo's HTML endpoint
//! (no key) or Brave Search (with a key), titles, URLs and snippets back.
//!
//! The model reads a result's page with the `fetch` tool when its host is
//! allowed; the search itself only ever sends the query to the engine.

use crate::fetch::decode_entities;
use pulse_provider::Tool;
use pulse_provider::apikey::ApiKeyLogin;
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;
use std::time::Duration;

pub const WEB_SEARCH_TOOL: &str = "web_search";
/// Results asked for when the call names no count.
pub const SEARCH_RESULTS: usize = 8;
/// The most results one call may ask for.
pub const SEARCH_RESULTS_MAX: usize = 20;
/// The longest query sent.
pub const SEARCH_QUERY_LIMIT: usize = 400;
/// The longest snippet shown per result.
pub const SNIPPET_LIMIT: usize = 300;
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);
pub const DUCKDUCKGO_URL: &str = "https://html.duckduckgo.com/html/";
pub const BRAVE_URL: &str = "https://api.search.brave.com/res/v1/web/search";
/// The environment variables a Brave Search key is read from, after the
/// private file `<home>/private/providers/brave.json`.
pub const BRAVE_KEY_ENV: &[&str] = &["BRAVE_SEARCH_API_KEY", "BRAVE_API_KEY"];
/// Points either engine at another endpoint (a test server, a proxy).
pub const ENDPOINT_ENV: &str = "PULSE_WEB_SEARCH_URL";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Engine {
    DuckDuckGo { url: String },
    Brave { url: String, key: String },
}

impl Engine {
    pub fn name(&self) -> &'static str {
        match self {
            Engine::DuckDuckGo { .. } => "duckduckgo",
            Engine::Brave { .. } => "brave",
        }
    }
}

/// The search the run offers: an engine, or nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebSearch {
    engine: Option<Engine>,
}

impl WebSearch {
    pub fn off() -> Self {
        Self::default()
    }

    pub fn duckduckgo(url: &str) -> Self {
        Self {
            engine: Some(Engine::DuckDuckGo {
                url: url.to_string(),
            }),
        }
    }

    pub fn brave(url: &str, key: &str) -> Self {
        Self {
            engine: Some(Engine::Brave {
                url: url.to_string(),
                key: key.to_string(),
            }),
        }
    }

    /// Brave when a key is stored or in the environment, else DuckDuckGo;
    /// [`ENDPOINT_ENV`] moves either to another endpoint.
    pub fn from_env(home: &str) -> Self {
        let endpoint = std::env::var(ENDPOINT_ENV).unwrap_or_default();
        let endpoint = endpoint.trim();
        let key = ApiKeyLogin::new(
            "brave",
            "Brave Search",
            home,
            BRAVE_KEY_ENV.iter().copied(),
            None,
        )
        .source()
        .map(|s| s.token)
        .unwrap_or_default();
        if !key.is_empty() {
            Self::brave(
                if endpoint.is_empty() {
                    BRAVE_URL
                } else {
                    endpoint
                },
                &key,
            )
        } else {
            Self::duckduckgo(if endpoint.is_empty() {
                DUCKDUCKGO_URL
            } else {
                endpoint
            })
        }
    }

    pub fn is_off(&self) -> bool {
        self.engine.is_none()
    }

    /// `duckduckgo`, `brave`, or `off`.
    pub fn describe(&self) -> String {
        self.engine
            .as_ref()
            .map_or_else(|| "off".to_string(), |e| e.name().to_string())
    }

    /// Runs one search; `count` is clamped to 1..=[`SEARCH_RESULTS_MAX`].
    pub fn search(&self, query: &str, count: usize, timeout: Duration) -> SearchResult {
        let Some(engine) = &self.engine else {
            return SearchResult::failed("web_search: no search engine is on for this run");
        };
        let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
        if query.is_empty() {
            return SearchResult::failed("web_search: the query is empty");
        }
        if query.chars().count() > SEARCH_QUERY_LIMIT {
            return SearchResult::failed(format!(
                "web_search: the query is {} characters; keep it under {SEARCH_QUERY_LIMIT}",
                query.chars().count()
            ));
        }
        let count = count.clamp(1, SEARCH_RESULTS_MAX);
        let client = match reqwest::blocking::Client::builder()
            .user_agent(format!(
                "Mozilla/5.0 (Macintosh) pulse-harness/{}",
                env!("CARGO_PKG_VERSION")
            ))
            .connect_timeout(Duration::from_secs(10).min(timeout))
            .timeout(timeout)
            .build()
        {
            Ok(c) => c,
            Err(e) => return SearchResult::failed(format!("web_search: http client: {e}")),
        };
        let request = match engine {
            Engine::DuckDuckGo { url } => {
                let mut target = match reqwest::Url::parse(url) {
                    Ok(u) => u,
                    Err(e) => {
                        return SearchResult::failed(format!("web_search: endpoint {url:?}: {e}"));
                    }
                };
                target.query_pairs_mut().append_pair("q", &query);
                client.get(target).header(
                    reqwest::header::ACCEPT,
                    "text/html, application/xhtml+xml, */*;q=0.5",
                )
            }
            Engine::Brave { url, key } => {
                let mut target = match reqwest::Url::parse(url) {
                    Ok(u) => u,
                    Err(e) => {
                        return SearchResult::failed(format!("web_search: endpoint {url:?}: {e}"));
                    }
                };
                target
                    .query_pairs_mut()
                    .append_pair("q", &query)
                    .append_pair("count", &count.to_string());
                client
                    .get(target)
                    .header(reqwest::header::ACCEPT, "application/json")
                    .header("X-Subscription-Token", key)
            }
        };
        let resp = match request.send() {
            Ok(r) => r,
            Err(e) => {
                return SearchResult::failed(format!(
                    "web_search: {} did not answer: {e}",
                    engine.name()
                ));
            }
        };
        let status = resp.status();
        let body = match resp.text() {
            Ok(b) => b,
            Err(e) => {
                return SearchResult::failed(format!(
                    "web_search: reading {}'s answer: {e}",
                    engine.name()
                ));
            }
        };
        if !status.is_success() {
            return SearchResult::failed(format!(
                "web_search: HTTP {} from {}: {}",
                status.as_u16(),
                engine.name(),
                one_line(&plain_text(&body), 200)
            ));
        }
        let hits = match engine {
            Engine::DuckDuckGo { .. } => {
                if is_bot_check(&body) {
                    return SearchResult::failed(
                        "web_search: duckduckgo asked for a human check instead of answering; try again in a while, or give the run a Brave Search key (BRAVE_SEARCH_API_KEY)",
                    );
                }
                parse_duckduckgo(&body)
            }
            Engine::Brave { .. } => match serde_json::from_str::<Value>(&body) {
                Ok(v) => parse_brave(&v),
                Err(e) => {
                    return SearchResult::failed(format!(
                        "web_search: brave's answer is not JSON: {e}"
                    ));
                }
            },
        };
        SearchResult {
            output: render(&query, &hits, count, engine.name()),
            exit_code: 0,
        }
    }
}

/// What a search gives back: the text and an exit code (0 success, 1 a
/// failure).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResult {
    pub output: String,
    pub exit_code: i32,
}

impl SearchResult {
    fn failed(text: impl Into<String>) -> Self {
        Self {
            output: text.into(),
            exit_code: 1,
        }
    }
}

/// One result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// The `web_search` tool, its description naming the engine.
pub fn web_search_tool(search: &WebSearch) -> Tool {
    Tool {
        name: WEB_SEARCH_TOOL.to_string(),
        description: format!(
            "Search the web through {} for pages about a question - documentation, an error message, a library's name - and get back titles, URLs and snippets ({SEARCH_RESULTS} by default, {SEARCH_RESULTS_MAX} at most). Read a result's page with the fetch tool when its host is allowed. The query goes to the search engine; keep secrets out of it.",
            search.describe()
        ),
        parameters: json!({"type":"object","properties":{
            "query":{"type":"string","description":"What to search for, as you would type it into a search engine."},
            "count":{"type":"integer","description":format!("How many results to return (1-{SEARCH_RESULTS_MAX}; {SEARCH_RESULTS} when left out).")}
        },"required":["query"]}),
    }
}

/// The query and count of a call's arguments; an error for a missing or
/// non-string query.
pub fn parse_arguments(arguments: &str) -> Result<(String, usize), String> {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    if !args.is_object() {
        return Err("web_search: arguments must be a JSON object with a string query".into());
    }
    let query = match args.get("query") {
        Some(Value::String(s)) => s.trim().to_string(),
        None | Some(Value::Null) => String::new(),
        Some(_) => return Err("web_search: query must be a string".into()),
    };
    if query.is_empty() {
        return Err("web_search: the query is empty".into());
    }
    let count = match args.get("count") {
        None | Some(Value::Null) => SEARCH_RESULTS,
        Some(Value::Number(n)) => n.as_u64().unwrap_or(SEARCH_RESULTS as u64) as usize,
        Some(Value::String(s)) => s.trim().parse().unwrap_or(SEARCH_RESULTS),
        Some(_) => return Err("web_search: count must be a number".into()),
    };
    Ok((query, count.clamp(1, SEARCH_RESULTS_MAX)))
}

static RESULT_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<div[^>]*class="result\b"#).unwrap());
static TITLE_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#).unwrap()
});
static TITLE_LINK_HREF_FIRST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<a[^>]*href="([^"]*)"[^>]*class="result__a"[^>]*>(.*?)</a>"#).unwrap()
});
static SNIPPET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#).unwrap());
static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());

/// The results of DuckDuckGo's HTML page, ads left out.
pub fn parse_duckduckgo(html: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    let starts: Vec<usize> = RESULT_SPLIT.find_iter(html).map(|m| m.start()).collect();
    for (i, start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(html.len());
        let block = &html[*start..end];
        let head_end = block.find('>').unwrap_or(0);
        if block[..head_end].contains("result--ad") {
            continue;
        }
        let Some(caps) = TITLE_LINK
            .captures(block)
            .or_else(|| TITLE_LINK_HREF_FIRST.captures(block))
        else {
            continue;
        };
        let url = result_url(&decode_entities(&caps[1]));
        let title = plain_text(&caps[2]);
        if url.is_empty() || title.is_empty() {
            continue;
        }
        let snippet = SNIPPET
            .captures(block)
            .map(|c| plain_text(&c[1]))
            .unwrap_or_default();
        hits.push(Hit {
            title,
            url,
            snippet,
        });
    }
    hits
}

/// The results of Brave's JSON answer.
pub fn parse_brave(v: &Value) -> Vec<Hit> {
    v.pointer("/web/results")
        .and_then(Value::as_array)
        .map(|results| {
            results
                .iter()
                .filter_map(|r| {
                    let url = r.get("url")?.as_str()?.trim().to_string();
                    let title = plain_text(r.get("title").and_then(Value::as_str).unwrap_or(""));
                    if url.is_empty() || title.is_empty() {
                        return None;
                    }
                    Some(Hit {
                        title,
                        url,
                        snippet: plain_text(
                            r.get("description").and_then(Value::as_str).unwrap_or(""),
                        ),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The page a DuckDuckGo result link leads to: the `uddg` target of its
/// redirect, or the link itself.
fn result_url(href: &str) -> String {
    let href = href.trim();
    let absolute = if let Some(rest) = href.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        href.to_string()
    };
    if let Ok(u) = reqwest::Url::parse(&absolute)
        && u.path().starts_with("/l/")
        && let Some((_, target)) = u.query_pairs().find(|(k, _)| k == "uddg")
    {
        return target.trim().to_string();
    }
    if absolute.starts_with("http://") || absolute.starts_with("https://") {
        absolute
    } else {
        String::new()
    }
}

fn is_bot_check(html: &str) -> bool {
    html.contains("anomaly-modal") || html.contains("bots use DuckDuckGo too")
}

fn plain_text(html: &str) -> String {
    decode_entities(&TAGS.replace_all(html, ""))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn one_line(s: &str, limit: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= limit {
        s
    } else {
        let cut: String = s.chars().take(limit).collect();
        format!("{cut}...")
    }
}

fn render(query: &str, hits: &[Hit], count: usize, engine: &str) -> String {
    if hits.is_empty() {
        return format!("no results for {query:?} from {engine}; try other words");
    }
    let shown = hits.iter().take(count).collect::<Vec<_>>();
    let mut out = format!(
        "{} result{} for {query:?} from {engine}:\n",
        shown.len(),
        if shown.len() == 1 { "" } else { "s" }
    );
    for (i, hit) in shown.iter().enumerate() {
        out.push_str(&format!("\n{}. {}\n   {}\n", i + 1, hit.title, hit.url));
        if !hit.snippet.is_empty() {
            out.push_str(&format!("   {}\n", one_line(&hit.snippet, SNIPPET_LIMIT)));
        }
    }
    out.trim_end().to_string()
}

/// Local-server helpers the harness tests share with this module's.
#[cfg(test)]
pub(crate) mod test_support {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    pub(crate) fn ddg_page(results: &[(&str, &str, &str)], ad: bool) -> String {
        let mut html = String::from("<html><body><div id=\"links\" class=\"results\">");
        if ad {
            html.push_str(r#"<div class="result results_links results_links_deep result--ad"><div class="links_main links_deep result__body"><h2 class="result__title"><a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad_domain=shop.example">Buy things</a></h2><a class="result__snippet" href="/ad">An ad</a></div></div>"#);
        }
        for (title, url, snippet) in results {
            let encoded: String = reqwest::Url::parse_with_params(
                "https://duckduckgo.com/l/",
                &[("uddg", *url), ("rut", "abc")],
            )
            .unwrap()
            .to_string()
            .replace("https://duckduckgo.com", "//duckduckgo.com")
            .replace('&', "&amp;");
            html.push_str(&format!(
                r#"<div class="result results_links results_links_deep web-result "><div class="links_main links_deep result__body"><h2 class="result__title"><a rel="nofollow" class="result__a" href="{encoded}">{title}</a></h2><div class="result__extras"><a class="result__url" href="{encoded}">{url}</a></div><a class="result__snippet" href="{encoded}">{snippet}</a></div></div>"#
            ));
        }
        html.push_str("</div></body></html>");
        html
    }

    /// A one-shot HTTP server answering each connection with the next
    /// response; returns its base URL and the requests it saw.
    pub(crate) fn serve(
        responses: Vec<String>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).to_string());
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        (format!("http://127.0.0.1:{}", addr.port()), seen)
    }

    pub(crate) fn http(status: &str, kind: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{ddg_page, http, serve};
    use super::*;

    #[test]
    fn duckduckgo_results_parse_with_ads_left_out_and_targets_decoded() {
        let page = ddg_page(
            &[
                (
                    "read_to_string in std::fs - Rust",
                    "https://doc.rust-lang.org/std/fs/fn.read_to_string.html",
                    "Reads the entire contents of a file into a <b>string</b>. &amp; more",
                ),
                ("std::fs - Rust", "https://doc.rust-lang.org/std/fs/", ""),
            ],
            true,
        );
        let hits = parse_duckduckgo(&page);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].title, "read_to_string in std::fs - Rust");
        assert_eq!(
            hits[0].url,
            "https://doc.rust-lang.org/std/fs/fn.read_to_string.html"
        );
        assert_eq!(
            hits[0].snippet,
            "Reads the entire contents of a file into a string. & more"
        );
        assert_eq!(hits[1].url, "https://doc.rust-lang.org/std/fs/");
        assert_eq!(hits[1].snippet, "");
        assert!(parse_duckduckgo("<html><body>nothing</body></html>").is_empty());
    }

    #[test]
    fn brave_results_parse_from_the_json_answer() {
        let v = json!({"web": {"results": [
            {"title": "A <strong>title</strong>", "url": "https://a.example/", "description": "First &amp; best"},
            {"title": "", "url": "https://skipped.example/"},
            {"title": "No url"},
            {"title": "B", "url": "https://b.example/x"}
        ]}});
        let hits = parse_brave(&v);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(
            hits[0],
            Hit {
                title: "A title".into(),
                url: "https://a.example/".into(),
                snippet: "First & best".into()
            }
        );
        assert_eq!(hits[1].url, "https://b.example/x");
        assert!(parse_brave(&json!({})).is_empty());
    }

    #[test]
    fn a_search_renders_numbered_results_and_names_failures() {
        let page = ddg_page(
            &[
                ("One", "https://one.example/", "the first"),
                ("Two", "https://two.example/", &"long ".repeat(100)),
                ("Three", "https://three.example/", ""),
            ],
            false,
        );
        let (base, seen) = serve(vec![
            http("200 OK", "text/html; charset=utf-8", &page),
            http("200 OK", "text/html", &ddg_page(&[], false)),
            http(
                "202 Accepted",
                "text/html",
                "<html><body><div class=\"anomaly-modal__title\">bots use DuckDuckGo too</div></body></html>",
            ),
            http(
                "429 Too Many Requests",
                "text/html",
                "<p>Slow <b>down</b> please</p>",
            ),
        ]);
        let search = WebSearch::duckduckgo(&format!("{base}/html/"));
        assert_eq!(search.describe(), "duckduckgo");
        let r = search.search("  rust   read_to_string ", 2, SEARCH_TIMEOUT);
        assert_eq!(r.exit_code, 0, "{}", r.output);
        assert_eq!(
            r.output,
            "2 results for \"rust read_to_string\" from duckduckgo:\n\n1. One\n   https://one.example/\n   the first\n\n2. Two\n   https://two.example/\n   ".to_string()
                + &format!("{}...", "long ".repeat(100).chars().take(SNIPPET_LIMIT).collect::<String>())
        );
        let request = seen.lock().unwrap()[0].clone();
        assert!(
            request.starts_with("GET /html/?q=rust+read_to_string HTTP/1.1"),
            "{request}"
        );
        assert!(request.contains("pulse-harness/"), "{request}");
        let r = search.search("nothing", 5, SEARCH_TIMEOUT);
        assert_eq!(r.exit_code, 0);
        assert_eq!(
            r.output,
            "no results for \"nothing\" from duckduckgo; try other words"
        );
        let r = search.search("blocked", 5, SEARCH_TIMEOUT);
        assert_eq!(r.exit_code, 1);
        assert!(r.output.contains("asked for a human check"), "{}", r.output);
        let r = search.search("again", 5, SEARCH_TIMEOUT);
        assert_eq!(r.exit_code, 1);
        assert_eq!(
            r.output,
            "web_search: HTTP 429 from duckduckgo: Slow down please"
        );
        let r = WebSearch::off().search("x", 1, SEARCH_TIMEOUT);
        assert_eq!(r.exit_code, 1);
        assert!(r.output.contains("no search engine is on"), "{}", r.output);
        let r = search.search("   ", 1, SEARCH_TIMEOUT);
        assert!(r.output.contains("the query is empty"), "{}", r.output);
        let r = search.search(&"q".repeat(SEARCH_QUERY_LIMIT + 1), 1, SEARCH_TIMEOUT);
        assert!(r.output.contains("keep it under"), "{}", r.output);
    }

    #[test]
    fn brave_sends_the_key_and_count() {
        let answer = json!({"web": {"results": [
            {"title": "Docs", "url": "https://docs.example/", "description": "The docs"}
        ]}})
        .to_string();
        let (base, seen) = serve(vec![http("200 OK", "application/json", &answer)]);
        let search = WebSearch::brave(&format!("{base}/res/v1/web/search"), "BSA-key");
        assert_eq!(search.describe(), "brave");
        let r = search.search("docs", 50, SEARCH_TIMEOUT);
        assert_eq!(r.exit_code, 0, "{}", r.output);
        assert_eq!(
            r.output,
            "1 result for \"docs\" from brave:\n\n1. Docs\n   https://docs.example/\n   The docs"
        );
        let request = seen.lock().unwrap()[0].clone();
        assert!(
            request.starts_with(&format!(
                "GET /res/v1/web/search?q=docs&count={SEARCH_RESULTS_MAX} HTTP/1.1"
            )),
            "{request}"
        );
        assert!(
            request
                .to_lowercase()
                .contains("x-subscription-token: bsa-key"),
            "{request}"
        );
    }

    #[test]
    fn the_engine_comes_from_the_key_and_the_endpoint_from_the_environment() {
        let _serial = crate::test_helpers::serial();
        let _env = pulse_testutil::env_lock();
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_str().unwrap();
        let _unset: Vec<_> = BRAVE_KEY_ENV
            .iter()
            .chain([ENDPOINT_ENV].iter())
            .map(|k| pulse_testutil::unsetenv(k))
            .collect();
        assert_eq!(
            WebSearch::from_env(home_s),
            WebSearch::duckduckgo(DUCKDUCKGO_URL)
        );
        {
            let _endpoint = pulse_testutil::setenv(ENDPOINT_ENV, "http://127.0.0.1:9/html/");
            assert_eq!(
                WebSearch::from_env(home_s),
                WebSearch::duckduckgo("http://127.0.0.1:9/html/")
            );
        }
        let _key = pulse_testutil::setenv("BRAVE_SEARCH_API_KEY", "BSA-env");
        assert_eq!(
            WebSearch::from_env(home_s),
            WebSearch::brave(BRAVE_URL, "BSA-env")
        );
        std::fs::create_dir_all(home.path().join("private/providers")).unwrap();
        std::fs::write(
            home.path().join("private/providers/brave.json"),
            r#"{"api_key": "BSA-file", "saved_at": "2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(
            WebSearch::from_env(home_s),
            WebSearch::brave(BRAVE_URL, "BSA-file"),
            "the private file wins over the environment"
        );
    }

    #[test]
    fn arguments_parse_with_a_clamped_count() {
        assert_eq!(
            parse_arguments(r#"{"query": " rust ", "count": 3}"#).unwrap(),
            ("rust".to_string(), 3)
        );
        assert_eq!(
            parse_arguments(r#"{"query": "rust"}"#).unwrap(),
            ("rust".to_string(), SEARCH_RESULTS)
        );
        assert_eq!(
            parse_arguments(r#"{"query": "rust", "count": 99}"#)
                .unwrap()
                .1,
            SEARCH_RESULTS_MAX
        );
        assert_eq!(
            parse_arguments(r#"{"query": "rust", "count": 0}"#)
                .unwrap()
                .1,
            1
        );
        assert_eq!(
            parse_arguments(r#"{"query": "rust", "count": "4"}"#)
                .unwrap()
                .1,
            4
        );
        assert!(
            parse_arguments(r#"{"query": ""}"#)
                .unwrap_err()
                .contains("empty")
        );
        assert!(
            parse_arguments(r#"{"query": 3}"#)
                .unwrap_err()
                .contains("must be a string")
        );
        assert!(parse_arguments("[]").unwrap_err().contains("JSON object"));
        assert!(
            parse_arguments(r#"{"query": "x", "count": true}"#)
                .unwrap_err()
                .contains("number")
        );
    }
}
