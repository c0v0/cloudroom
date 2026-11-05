//! Safe link-preview fetcher: public-IP pinning, no redirects to private
//! targets, 512 KiB HTML cap, minimal OpenGraph/title parsing.

use std::net::{IpAddr, SocketAddr};

use futures_util::StreamExt;
use regex_lite::Regex;

const MAX_BYTES: usize = 512 * 1024;
const MAX_REDIRECTS: usize = 2;

#[derive(Debug, Clone)]
pub struct Preview {
    pub url: String,
    pub title: String,
    pub description: String,
    pub site_name: String,
}

fn client(host: &str, ip: IpAddr, port: u16) -> Result<reqwest::Client, String> {
    let mut b = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("CloudRoomLinkPreview/1.0");
    if host.parse::<IpAddr>().is_err() {
        b = b.resolve(host, SocketAddr::new(ip, port));
    }
    b.build().map_err(|e| format!("preview client: {e}"))
}

async fn resolve_public(host: &str, port: u16) -> Result<IpAddr, String> {
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("preview DNS: {e}"))?;
    let mut chosen = None;
    for a in addrs {
        let ip = a.ip();
        if !crate::util::is_public_ip(ip) {
            return Err("preview URL resolves to a non-public address".into());
        }
        if chosen.is_none() {
            chosen = Some(ip);
        }
    }
    chosen.ok_or_else(|| "preview URL has no addresses".into())
}

fn validate_url(raw: &str) -> Result<url::Url, String> {
    if raw.len() > 2048 {
        return Err("URL too long".into());
    }
    let url = url::Url::parse(raw).map_err(|e| format!("invalid URL: {e}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("only http(s) URLs are supported".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("credentials in URL are not allowed".into());
    }
    let port = url.port_or_known_default().ok_or("URL has no port")?;
    if port != 80 && port != 443 {
        return Err("only ports 80/443 are allowed".into());
    }
    Ok(url)
}

pub async fn fetch(raw: &str) -> Result<Preview, String> {
    let mut url = validate_url(raw)?;
    for _ in 0..=MAX_REDIRECTS {
        let host = url.host_str().ok_or("URL has no host")?.to_string();
        let port = url.port_or_known_default().ok_or("URL has no port")?;
        let ip = resolve_public(&host, port).await?;
        let c = client(&host, ip, port)?;
        let res = c
            .get(url.clone())
            .send()
            .await
            .map_err(|e| format!("preview request failed: {e}"))?;
        if res.status().is_redirection() {
            let loc = res
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or("redirect without Location")?;
            url = url.join(loc).map_err(|e| format!("invalid redirect: {e}"))?;
            validate_url(url.as_str())?;
            continue;
        }
        if !res.status().is_success() {
            return Err(format!("preview returned {}", res.status()));
        }
        let ctype = res
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ctype.to_ascii_lowercase().starts_with("text/html")
            && !ctype.to_ascii_lowercase().starts_with("application/xhtml")
        {
            return Err("preview is not HTML".into());
        }
        let mut body = Vec::new();
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("preview read failed: {e}"))?;
            if body.len() + chunk.len() > MAX_BYTES {
                return Err("preview HTML too large".into());
            }
            body.extend_from_slice(&chunk);
        }
        let html = String::from_utf8_lossy(&body);
        return parse(&html, url.as_str());
    }
    Err("too many redirects".into())
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn clean(s: &str) -> String {
    let tag = Regex::new(r"(?is)<[^>]+>").unwrap();
    let text = tag.replace_all(s, " ");
    decode_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn meta(html: &str, keys: &[&str]) -> Option<String> {
    let tags = Regex::new(r"(?is)<meta\s+[^>]*>").unwrap();
    let attrs = Regex::new(r#"(?is)([a-z:-]+)\s*=\s*["']([^"']*)["']"#).unwrap();
    for tag in tags.find_iter(html) {
        let mut name = None;
        let mut content = None;
        for a in attrs.captures_iter(tag.as_str()) {
            let k = a.get(1)?.as_str().to_ascii_lowercase();
            let v = a.get(2)?.as_str().to_string();
            if k == "property" || k == "name" {
                name = Some(v.to_ascii_lowercase());
            } else if k == "content" {
                content = Some(v);
            }
        }
        if let (Some(n), Some(c)) = (name, content) {
            if keys.contains(&n.as_str()) {
                let c = clean(&c);
                if !c.is_empty() {
                    return Some(c);
                }
            }
        }
    }
    None
}

fn parse(html: &str, final_url: &str) -> Result<Preview, String> {
    let title = meta(html, &["og:title", "twitter:title"]).or_else(|| {
        Regex::new(r"(?is)<title[^>]*>(.*?)</title>")
            .ok()
            .and_then(|re| re.captures(html))
            .map(|c| clean(&c[1]))
            .filter(|s| !s.is_empty())
    });
    let description = meta(
        html,
        &["og:description", "twitter:description", "description"],
    );
    let site_name = meta(html, &["og:site_name"]).or_else(|| {
        url::Url::parse(final_url)
            .ok()
            .and_then(|u| u.host_str().map(|h| h.to_string()))
    });
    let title = title.unwrap_or_default();
    let description = description.unwrap_or_default();
    if title.is_empty() && description.is_empty() {
        return Err("no preview metadata".into());
    }
    Ok(Preview {
        url: final_url.to_string(),
        title: title.chars().take(300).collect(),
        description: description.chars().take(1000).collect(),
        site_name: site_name.unwrap_or_default().chars().take(100).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_open_graph_and_title_fallback() {
        let p = parse(
            r#"<html><head><title>Fallback</title>
            <meta property="og:title" content="Hello &amp; world">
            <meta name="description" content="A page">
            <meta property="og:site_name" content="Example"></head></html>"#,
            "https://example.com/x",
        )
        .unwrap();
        assert_eq!(p.title, "Hello & world");
        assert_eq!(p.description, "A page");
        assert_eq!(p.site_name, "Example");
        assert_eq!(p.url, "https://example.com/x");
    }
}
