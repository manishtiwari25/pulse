//! The `fetch` tool: one GET of a web page, returned as text, for the
//! hosts the owner allows (`harness.fetch_allow`). HTML is reduced to its
//! text with links kept; JSON and other text come back as they are.

use pulse_provider::Tool;
use regex::Regex;
use serde_json::json;
use std::io::Read;
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;

/// The name of the tool.
pub const FETCH_TOOL: &str = "fetch";

/// Bytes of a response body read before the rest is dropped.
pub const FETCH_BYTE_LIMIT: usize = 2 * 1024 * 1024;

/// How long one fetch (every redirect included) may take.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Redirects followed before the fetch gives up.
pub const FETCH_REDIRECTS: usize = 5;

/// Exit code of a fetch the policy refused, like a denied command.
pub const FETCH_REFUSED_EXIT: i32 = 126;

/// Which hosts `fetch` may reach: a list of host names, `*.suffix`
/// patterns and `*` (any public host). Empty means the tool is off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FetchPolicy {
    pub patterns: Vec<String>,
}

impl FetchPolicy {
    /// Parses the setting: hosts separated by commas or whitespace, case
    /// ignored, a leading scheme or trailing path dropped.
    pub fn parse(spec: &str) -> Self {
        let mut patterns: Vec<String> = Vec::new();
        for raw in spec.split([',', ' ', '\n', '\t', ';']) {
            let mut p = raw.trim().to_lowercase();
            if let Some(rest) = p.split_once("://").map(|(_, r)| r.to_string()) {
                p = rest;
            }
            if let Some((host, _)) = p.split_once('/') {
                p = host.to_string();
            }
            let p = p.trim_end_matches('.').to_string();
            if p.is_empty() || p == "off" || p == "none" {
                continue;
            }
            if !patterns.contains(&p) {
                patterns.push(p);
            }
        }
        Self { patterns }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// True when `host` may be fetched. `*` covers public names only:
    /// localhost, `.local`/`.internal` names and IP literals need their
    /// own entry.
    pub fn allows(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_lowercase();
        if host.is_empty() {
            return false;
        }
        self.patterns.iter().any(|p| {
            if p == "*" {
                !is_local_or_ip(&host)
            } else if let Some(suffix) = p.strip_prefix("*.") {
                host == suffix || host.ends_with(&format!(".{suffix}"))
            } else {
                *p == host
            }
        })
    }

    /// The hosts in words, for the tool description and refusals.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for p in &self.patterns {
            if p == "*" {
                parts.push("any public host".to_string());
            } else {
                parts.push(p.clone());
            }
        }
        parts.join(", ")
    }
}

fn is_local_or_ip(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok()
}

/// The `fetch` tool, its description naming the allowed hosts.
pub fn fetch_tool(policy: &FetchPolicy) -> Tool {
    Tool {
        name: FETCH_TOOL.to_string(),
        description: format!(
            "Read a web page or API response as text: one GET of an http(s) URL, HTML reduced to text with links as text (url), at most {} KB. Allowed hosts: {}.",
            FETCH_BYTE_LIMIT / 1024,
            policy.describe()
        ),
        parameters: json!({"type":"object","properties":{
            "url":{"type":"string","description":"The http or https URL to read."}
        },"required":["url"]}),
    }
}

/// What a fetch gives back: the text and an exit code (0 success, 1 a
/// failure or an HTTP error status, [`FETCH_REFUSED_EXIT`] a refusal).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FetchResult {
    pub output: String,
    pub exit_code: i32,
}

impl FetchResult {
    fn failed(text: impl Into<String>) -> Self {
        Self {
            output: text.into(),
            exit_code: 1,
        }
    }
}

/// Fetches `url` under `policy`, following at most [`FETCH_REDIRECTS`]
/// redirects (each checked against the policy), reading at most
/// `max_bytes` of the body, within `timeout` in all.
pub fn fetch(url: &str, policy: &FetchPolicy, max_bytes: usize, timeout: Duration) -> FetchResult {
    let mut current = match check_url(url, policy) {
        Ok(u) => u,
        Err(r) => return r,
    };
    let client = match reqwest::blocking::Client::builder()
        .user_agent(format!("pulse-harness/{}", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10).min(timeout))
        .timeout(timeout)
        .build()
    {
        Ok(c) => c,
        Err(e) => return FetchResult::failed(format!("fetch: http client: {e}")),
    };
    let started = std::time::Instant::now();
    let mut hops = 0;
    loop {
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            return FetchResult::failed(format!("fetch: timed out after {}s", timeout.as_secs()));
        }
        let resp = match client
            .get(current.clone())
            .header(
                reqwest::header::ACCEPT,
                "text/html, text/plain, application/json, text/*;q=0.9, */*;q=0.5",
            )
            .timeout(left)
            .send()
        {
            Ok(r) => r,
            Err(e) => return FetchResult::failed(format!("fetch {current}: {e}")),
        };
        let status = resp.status();
        if status.is_redirection() {
            hops += 1;
            if hops > FETCH_REDIRECTS {
                return FetchResult::failed(format!(
                    "fetch {current}: more than {FETCH_REDIRECTS} redirects"
                ));
            }
            let Some(location) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
            else {
                return FetchResult::failed(format!(
                    "fetch {current}: HTTP {} without a Location header",
                    status.as_u16()
                ));
            };
            let next = match current.join(&location) {
                Ok(u) => u,
                Err(e) => {
                    return FetchResult::failed(format!(
                        "fetch {current}: redirect to {location:?}: {e}"
                    ));
                }
            };
            current = match check_url(next.as_str(), policy) {
                Ok(u) => u,
                Err(mut r) => {
                    r.output = format!("redirected from {current}: {}", r.output);
                    return r;
                }
            };
            continue;
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mut body = Vec::new();
        let mut cut = false;
        match resp.take(max_bytes as u64 + 1).read_to_end(&mut body) {
            Ok(_) => {
                if body.len() > max_bytes {
                    body.truncate(max_bytes);
                    cut = true;
                }
            }
            Err(e) => return FetchResult::failed(format!("fetch {current}: read body: {e}")),
        }
        let kind = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let text = if is_text_type(&kind) {
            let raw = String::from_utf8_lossy(&body).to_string();
            if kind == "text/html" || kind == "application/xhtml+xml" {
                html_to_text(&raw)
            } else {
                raw
            }
        } else if body.is_empty() {
            String::new()
        } else {
            format!(
                "[binary body of type {}]",
                if kind.is_empty() { "unknown" } else { &kind }
            )
        };
        let mut head = format!(
            "{current}\nHTTP {} {}, {} bytes",
            status.as_u16(),
            if kind.is_empty() {
                "no content type"
            } else {
                &kind
            },
            body.len()
        );
        if cut {
            head.push_str(&format!(" (cut at {max_bytes})"));
        }
        return FetchResult {
            output: format!("{head}\n\n{}", text.trim_end()),
            exit_code: i32::from(!status.is_success()),
        };
    }
}

fn check_url(url: &str, policy: &FetchPolicy) -> Result<reqwest::Url, FetchResult> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|e| FetchResult::failed(format!("fetch: {url:?} is not a URL: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(FetchResult::failed(format!(
            "fetch: only http and https URLs can be read, not {}",
            parsed.scheme()
        )));
    }
    let host = parsed.host_str().unwrap_or("");
    if !policy.allows(host) {
        return Err(FetchResult {
            output: format!(
                "refused by policy: host {host:?} is not in harness.fetch_allow (allowed: {}).",
                if policy.is_empty() {
                    "nothing".to_string()
                } else {
                    policy.describe()
                }
            ),
            exit_code: FETCH_REFUSED_EXIT,
        });
    }
    Ok(parsed)
}

fn is_text_type(kind: &str) -> bool {
    kind.is_empty()
        || kind.starts_with("text/")
        || kind.ends_with("+json")
        || kind.ends_with("+xml")
        || matches!(
            kind,
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/x-yaml"
                | "application/yaml"
                | "application/toml"
                | "application/x-www-form-urlencoded"
                | "application/xhtml+xml"
        )
}

static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title\b[^>]*>(.*?)</title>").unwrap());
static DROP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?is)<head\b[^>]*>.*?</head\s*>|<title\b[^>]*>.*?</title\s*>|<script\b[^>]*>.*?</script\s*>|<style\b[^>]*>.*?</style\s*>|<noscript\b[^>]*>.*?</noscript\s*>|<svg\b[^>]*>.*?</svg\s*>|<template\b[^>]*>.*?</template\s*>|<!--.*?-->",
    )
    .unwrap()
});
static LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<a\b[^>]*?\bhref\s*=\s*["']([^"'#][^"']*)["'][^>]*>(.*?)</a\s*>"#).unwrap()
});
static BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</?(p|div|br|li|ul|ol|h[1-6]|tr|table|thead|tbody|section|article|header|footer|blockquote|pre|hr|dd|dt|dl|nav|aside|main|form|fieldset|figure|details|summary)\b[^>]*>").unwrap()
});
static CELL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)</(td|th)\s*>").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static ENTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \r\x0b\x0c\u{a0}]+").unwrap());
static BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

/// Reduces an HTML document to text: the title first, block elements as
/// line breaks, links as `text (url)`, scripts and styles dropped.
pub fn html_to_text(html: &str) -> String {
    let title = TITLE
        .captures(html)
        .map(|c| {
            decode_entities(&TAG.replace_all(&c[1], ""))
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let body = DROP.replace_all(html, "");
    let body = LINK.replace_all(&body, |caps: &regex::Captures| {
        let href = caps[1].trim();
        let text = TAG.replace_all(&caps[2], "").trim().to_string();
        if text.is_empty() || !(href.starts_with("http://") || href.starts_with("https://")) {
            caps[2].to_string()
        } else if text == href {
            text
        } else {
            format!("{text} ({href})")
        }
    });
    let body = BLOCK.replace_all(&body, "\n");
    let body = CELL.replace_all(&body, "\t");
    let body = TAG.replace_all(&body, "");
    let body = decode_entities(&body);
    let body = SPACES.replace_all(&body, " ");
    let lines: Vec<String> = body.lines().map(|l| l.trim().to_string()).collect();
    let body = BLANKS
        .replace_all(&lines.join("\n"), "\n\n")
        .trim()
        .to_string();
    if title.is_empty() {
        body
    } else {
        format!("# {title}\n\n{body}")
    }
}

fn decode_entities(s: &str) -> String {
    ENTITY
        .replace_all(s, |caps: &regex::Captures| {
            let e = &caps[1];
            match e {
                "amp" => "&".to_string(),
                "lt" => "<".to_string(),
                "gt" => ">".to_string(),
                "quot" => "\"".to_string(),
                "apos" => "'".to_string(),
                "nbsp" => " ".to_string(),
                "ndash" => "–".to_string(),
                "mdash" => "—".to_string(),
                "hellip" => "…".to_string(),
                "copy" => "©".to_string(),
                "laquo" => "«".to_string(),
                "raquo" => "»".to_string(),
                "lsquo" => "‘".to_string(),
                "rsquo" => "’".to_string(),
                "ldquo" => "“".to_string(),
                "rdquo" => "”".to_string(),
                _ => {
                    let code = e
                        .strip_prefix("#x")
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .or_else(|| e.strip_prefix('#').and_then(|d| d.parse().ok()));
                    match code.and_then(char::from_u32) {
                        Some(c) => c.to_string(),
                        None => caps[0].to_string(),
                    }
                }
            }
        })
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    #[test]
    fn policy_parses_and_matches() {
        let p = FetchPolicy::parse("Docs.Rs, https://*.github.com/path, 127.0.0.1 ;*\n");
        assert_eq!(p.patterns, ["docs.rs", "*.github.com", "127.0.0.1", "*"]);
        assert!(p.allows("docs.rs"));
        assert!(p.allows("DOCS.RS."));
        assert!(p.allows("api.github.com"));
        assert!(p.allows("github.com"));
        assert!(p.allows("example.org"), "* covers public hosts");
        assert!(p.allows("127.0.0.1"), "listed explicitly");
        assert!(!p.allows("10.0.0.1"), "* never covers IP literals");
        assert!(!p.allows("localhost"));
        assert!(!p.allows("metadata.google.internal"));
        assert!(!p.allows("printer.local"));
        assert!(!p.allows(""));
        assert_eq!(
            p.describe(),
            "docs.rs, *.github.com, 127.0.0.1, any public host"
        );
        let narrow = FetchPolicy::parse("*.example.com");
        assert!(narrow.allows("example.com"));
        assert!(narrow.allows("a.b.example.com"));
        assert!(!narrow.allows("notexample.com"));
        assert!(FetchPolicy::parse("").is_empty());
        assert!(FetchPolicy::parse("off").is_empty());
        let tool = fetch_tool(&narrow);
        assert_eq!(tool.name, "fetch");
        assert!(tool.description.contains("Allowed hosts: *.example.com."));
    }

    #[test]
    fn html_becomes_text_with_links() {
        let html = r#"<!doctype html><html><head><title>My &amp; Page</title>
<style>body{}</style><script>var x = "<p>";</script></head>
<body><h1>Hello</h1><!-- hidden --><p>Some <b>bold</b> text&nbsp;here &#x41;&#66;.</p>
<ul><li>One</li><li><a href="https://example.com/a">A link</a></li><li><a href="/rel">rel</a></li>
<li><a href="https://example.com/b">https://example.com/b</a></li></ul>
<table><tr><td>k</td><td>v</td></tr></table>


<p>End</p></body></html>"#;
        let text = html_to_text(html);
        assert_eq!(
            text,
            "# My & Page\n\nHello\n\nSome bold text here AB.\n\nOne\n\nA link (https://example.com/a)\n\nrel\n\nhttps://example.com/b\n\nk\tv\n\nEnd"
        );
        assert_eq!(
            html_to_text("plain &lt;text&gt; &unknown; &#9999999;"),
            "plain <text> &unknown; &#9999999;"
        );
    }

    #[test]
    fn refuses_bad_urls_and_hosts_without_a_request() {
        let policy = FetchPolicy::parse("example.com");
        let r = fetch("ftp://example.com/x", &policy, 1000, FETCH_TIMEOUT);
        assert_eq!(r.exit_code, 1);
        assert!(r.output.contains("only http and https"), "{}", r.output);
        let r = fetch("https://evil.example.org/", &policy, 1000, FETCH_TIMEOUT);
        assert_eq!(r.exit_code, FETCH_REFUSED_EXIT);
        assert_eq!(
            r.output,
            "refused by policy: host \"evil.example.org\" is not in harness.fetch_allow (allowed: example.com)."
        );
        let r = fetch("not a url", &FetchPolicy::parse("*"), 1000, FETCH_TIMEOUT);
        assert!(r.output.contains("is not a URL"), "{}", r.output);
        let r = fetch(
            "https://example.com/",
            &FetchPolicy::default(),
            1000,
            FETCH_TIMEOUT,
        );
        assert_eq!(r.exit_code, FETCH_REFUSED_EXIT);
        assert!(r.output.contains("(allowed: nothing)"), "{}", r.output);
    }

    /// Serves one canned response per connection, in order, on a loopback port.
    fn serve(responses: Vec<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for body in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buf = [0u8; 4096];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let _ = stream.write_all(body.as_bytes());
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    #[test]
    fn fetches_follows_an_allowed_redirect_and_caps_the_body() {
        let page = "<html><head><title>T</title></head><body><p>Hello world</p></body></html>";
        let base = serve(vec![
            "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_string(),
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            ),
            format!(
                "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{}",
                "x".repeat(20)
            ),
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc".to_string(),
        ]);
        let policy = FetchPolicy::parse("127.0.0.1");
        let r = fetch(
            &format!("{base}/start"),
            &policy,
            FETCH_BYTE_LIMIT,
            FETCH_TIMEOUT,
        );
        assert_eq!(r.exit_code, 0, "{}", r.output);
        assert_eq!(
            r.output,
            format!(
                "{base}/final\nHTTP 200 text/html, {} bytes\n\n# T\n\nHello world",
                page.len()
            )
        );
        let r = fetch(&format!("{base}/missing"), &policy, 8, FETCH_TIMEOUT);
        assert_eq!(r.exit_code, 1);
        assert_eq!(
            r.output,
            format!("{base}/missing\nHTTP 404 text/plain, 8 bytes (cut at 8)\n\nxxxxxxxx")
        );
        let r = fetch(
            &format!("{base}/bin"),
            &policy,
            FETCH_BYTE_LIMIT,
            FETCH_TIMEOUT,
        );
        assert_eq!(r.exit_code, 0);
        assert!(
            r.output
                .ends_with("3 bytes\n\n[binary body of type application/octet-stream]"),
            "{}",
            r.output
        );
    }

    #[test]
    fn a_redirect_to_a_host_outside_the_policy_is_refused() {
        let base = serve(vec![
            "HTTP/1.1 301 Moved\r\nLocation: https://elsewhere.example.org/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
        ]);
        let r = fetch(
            &format!("{base}/"),
            &FetchPolicy::parse("127.0.0.1"),
            1000,
            FETCH_TIMEOUT,
        );
        assert_eq!(r.exit_code, FETCH_REFUSED_EXIT);
        assert!(
            r.output.starts_with(&format!(
                "redirected from {base}/: refused by policy: host \"elsewhere.example.org\""
            )),
            "{}",
            r.output
        );
    }
}
