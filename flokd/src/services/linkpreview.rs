use std::time::Duration;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::bus::{Bus, Request};
use crate::http;

const SERVICE: &str = "linkpreview";
const TIMEOUT: Duration = Duration::from_secs(5);
// The head is all a preview needs; the rest of a page is wasted bandwidth.
const MAX_HTML: usize = 500 * 1024;
// Sites serve bots a different page, or none.
const BROWSER: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

// One finished fetch; the shell matches it back by url and id.
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct State {
    url: String,
    id: String,
    data: Value,
}

#[derive(Deserialize)]
struct Fetch {
    url: String,
    #[serde(default)]
    id: String,
}

pub struct Service {
    bus: Bus,
    http: reqwest::Client,
}

impl Service {
    pub fn new(bus: Bus) -> Result<Self> {
        Ok(Self {
            bus,
            http: http::client(BROWSER, TIMEOUT)?,
        })
    }

    pub async fn run(self, mut requests: mpsc::Receiver<Request>) {
        while let Some(req) = requests.recv().await {
            if req.cmd != "fetch" {
                tracing::warn!("unknown command: {}", req.cmd);
                continue;
            }
            let Ok(f) = serde_json::from_value::<Fetch>(req.args) else {
                tracing::warn!("bad fetch");
                continue;
            };
            let (bus, http) = (self.bus.clone(), self.http.clone());
            tokio::spawn(async move {
                let data = match preview(&http, &f.url).await {
                    Ok(v) => v,
                    Err(e) => {
                        json!({"error": format!("{e:#}"), "url": f.url, "request_url": f.url})
                    }
                };
                let state = State {
                    url: f.url,
                    id: f.id,
                    data,
                };
                match serde_json::to_value(&state) {
                    Ok(v) => bus.publish(SERVICE, &v),
                    Err(e) => tracing::error!("serialise failed: {e}"),
                }
            });
        }
    }
}

async fn preview(http: &reqwest::Client, url: &str) -> Result<Value> {
    let parsed = url::Url::parse(url)?;
    let host = parsed.host_str().unwrap_or_default().to_string();
    if host.is_empty() {
        bail!("Invalid URL");
    }

    if let Some(id) = youtube_id(url)
        && let Ok(v) = youtube(http, url, &id).await
    {
        return Ok(v);
    }
    if is_twitter(&host)
        && let Ok(v) = twitter(http, url).await
    {
        return Ok(v);
    }

    let page = http::get(http, url, MAX_HTML).await?;
    if !page.content_type.contains("text/html") {
        bail!("Not an HTML page");
    }
    let html = String::from_utf8_lossy(&page.body);
    let mut meta = scrape(&html);
    let base = url::Url::parse(&page.url)?;

    if !meta.image.is_empty() {
        meta.image = absolute(&base, &meta.image);
    }
    meta.favicon = if meta.favicon.is_empty() {
        format!(
            "{}://{}/favicon.ico",
            base.scheme(),
            base.host_str().unwrap_or_default()
        )
    } else {
        absolute(&base, &meta.favicon)
    };
    if meta.url.is_empty() {
        meta.url = url.to_string();
    }
    if meta.site_name.is_empty() {
        meta.site_name = base.host_str().unwrap_or_default().to_string();
    }
    Ok(json!({
        "title": meta.title,
        "description": meta.description,
        "image": meta.image,
        "url": meta.url,
        "request_url": url,
        "site_name": meta.site_name,
        "type": meta.kind,
        "favicon": meta.favicon,
    }))
}

fn absolute(base: &url::Url, href: &str) -> String {
    base.join(href)
        .map_or_else(|_| href.to_string(), |u| u.to_string())
}

fn is_twitter(host: &str) -> bool {
    let h = host
        .trim_start_matches("www.")
        .trim_start_matches("mobile.");
    h == "twitter.com" || h == "x.com"
}

fn youtube_id(url: &str) -> Option<String> {
    for marker in [
        "youtube.com/watch?v=",
        "youtu.be/",
        "youtube.com/embed/",
        "youtube.com/v/",
        "youtube.com/shorts/",
    ] {
        if let Some(i) = url.find(marker) {
            let id: String = url[i + marker.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if id.len() >= 11 {
                return Some(id[..11].to_string());
            }
        }
    }
    None
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OEmbed {
    title: String,
    author_name: String,
    thumbnail_url: String,
    html: String,
}

async fn oembed(http: &reqwest::Client, endpoint: &str, url: &str) -> Result<OEmbed> {
    Ok(http
        .get(endpoint)
        .query(&[("url", url), ("format", "json")])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn youtube(http: &reqwest::Client, url: &str, id: &str) -> Result<Value> {
    let o = oembed(
        http,
        "https://www.youtube.com/oembed",
        &format!("https://www.youtube.com/watch?v={id}"),
    )
    .await?;
    let author = if o.author_name.is_empty() {
        "Unknown"
    } else {
        &o.author_name
    };
    Ok(json!({
        "title": o.title,
        "description": author,
        // YouTube answers maxresdefault with hqdefault when there is none.
        "image": o.thumbnail_url.replace("hqdefault", "maxresdefault"),
        "url": url,
        "request_url": url,
        "site_name": "YouTube",
        "type": "video",
        "favicon": "https://www.youtube.com/s/desktop/9c0f82da/img/favicon_144x144.png",
        "author": o.author_name,
        "video_id": id,
    }))
}

async fn twitter(http: &reqwest::Client, url: &str) -> Result<Value> {
    let o = oembed(http, "https://publish.twitter.com/oembed", url).await?;
    Ok(json!({
        "title": if o.author_name.is_empty() { "Tweet" } else { &o.author_name },
        "description": decode_entities(&strip_tags(&o.html)),
        "image": "",
        "url": url,
        "request_url": url,
        "site_name": "X (Twitter)",
        "type": "article",
        "favicon": "https://abs.twimg.com/favicons/twitter.3.ico",
        "author": o.author_name,
    }))
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Default, Debug)]
struct Meta {
    title: String,
    description: String,
    image: String,
    url: String,
    site_name: String,
    kind: String,
    favicon: String,
}

// SVG scales, PNG is crisp, ICO is whatever is left; within a format, 32–128px.
fn favicon_score(href: &str, kind: &str, sizes: &str) -> (u8, i64) {
    let (h, k) = (href.to_ascii_lowercase(), kind.to_ascii_lowercase());
    let format = if h.contains(".svg") || k.contains("svg") {
        3
    } else if h.contains(".png") || k.contains("png") {
        2
    } else {
        1
    };
    let size: i64 = sizes
        .split_once('x')
        .and_then(|(w, _)| w.parse().ok())
        .unwrap_or(0);
    let fit = if size > 128 {
        128 - (size - 128) / 10
    } else {
        size
    };
    (format, fit)
}

fn scrape(html: &str) -> Meta {
    let mut m = Meta {
        kind: "website".into(),
        ..Default::default()
    };
    let Ok(dom) = tl::parse(html, tl::ParserOptions::default()) else {
        return m;
    };
    let parser = dom.parser();
    let mut fallback_title = String::new();
    let mut best_icon: Option<((u8, i64), String)> = None;
    let (mut tw_title, mut tw_desc, mut tw_image, mut name_desc) =
        (String::new(), String::new(), String::new(), String::new());

    for node in dom.nodes() {
        let Some(tag) = node.as_tag() else { continue };
        let attrs = tag.attributes();
        let attr = |k: &str| {
            attrs
                .get(k)
                .flatten()
                .map(|b| decode_entities(&b.as_utf8_str()))
                .unwrap_or_default()
        };
        match tag.name().as_utf8_str().to_ascii_lowercase().as_str() {
            "title" if fallback_title.is_empty() => {
                fallback_title = decode_entities(tag.inner_text(parser).trim());
            }
            "link" => {
                let (rel, href) = (attr("rel"), attr("href"));
                if !href.is_empty() && rel.to_ascii_lowercase().contains("icon") {
                    let score = favicon_score(&href, &attr("type"), &attr("sizes"));
                    if best_icon.as_ref().is_none_or(|(s, _)| score > *s) {
                        best_icon = Some((score, href));
                    }
                }
            }
            "meta" => {
                let content = attr("content");
                if content.is_empty() {
                    continue;
                }
                match attr("property").as_str() {
                    "og:title" => m.title = content.clone(),
                    "og:description" => m.description = content.clone(),
                    "og:image" => m.image = content.clone(),
                    "og:url" => m.url = content.clone(),
                    "og:site_name" => m.site_name = content.clone(),
                    "og:type" => m.kind = content.clone(),
                    _ => {}
                }
                let slot = match attr("name").as_str() {
                    "twitter:title" => &mut tw_title,
                    "twitter:description" => &mut tw_desc,
                    "twitter:image" => &mut tw_image,
                    "description" => &mut name_desc,
                    _ => continue,
                };
                if slot.is_empty() {
                    *slot = content;
                }
            }
            _ => {}
        }
    }

    for (field, fallbacks) in [
        (&mut m.title, [tw_title, fallback_title]),
        (&mut m.description, [tw_desc, name_desc]),
        (&mut m.image, [tw_image, String::new()]),
    ] {
        if field.is_empty()
            && let Some(f) = fallbacks.into_iter().find(|f| !f.is_empty())
        {
            *field = f;
        }
    }
    m.favicon = best_icon.map(|(_, h)| h).unwrap_or_default();
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_ids_from_every_url_shape() {
        for u in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1",
            "https://youtu.be/dQw4w9WgXcQ?si=x",
            "https://youtube.com/embed/dQw4w9WgXcQ",
            "https://youtube.com/shorts/dQw4w9WgXcQ",
        ] {
            assert_eq!(youtube_id(u).as_deref(), Some("dQw4w9WgXcQ"), "{u}");
        }
        assert!(youtube_id("https://youtube.com/watch?v=short").is_none());
        assert!(youtube_id("https://example.com").is_none());
    }

    #[test]
    fn twitter_hosts_only() {
        assert!(is_twitter("x.com"));
        assert!(is_twitter("mobile.twitter.com"));
        assert!(!is_twitter("box.com"));
        assert!(!is_twitter("netflix.com"));
    }

    #[test]
    fn open_graph_beats_twitter_beats_title() {
        let html = r#"<html><head>
            <title>Plain &amp; simple</title>
            <meta name="twitter:title" content="Tw title">
            <meta property="og:title" content="OG &quot;title&quot;">
            <meta name="description" content="Named desc">
            <meta name="twitter:image" content="/img.png">
            <link rel="icon" href="/favicon.ico">
            <link rel="icon" type="image/png" sizes="64x64" href="/icon-64.png">
            <link rel="apple-touch-icon" sizes="1024x1024" href="/huge.png">
            </head></html>"#;
        let m = scrape(html);
        assert_eq!(m.title, "OG \"title\"");
        assert_eq!(m.description, "Named desc");
        assert_eq!(m.image, "/img.png");
        assert_eq!(m.favicon, "/icon-64.png");
        assert_eq!(m.kind, "website");

        let bare = scrape("<title> Just &#8212; a title </title>");
        assert_eq!(bare.title, "Just \u{2014} a title");
    }

    #[test]
    fn entities_and_tags() {
        assert_eq!(
            decode_entities("a &amp; b &#x27;c&#39; &bogus; &"),
            "a & b 'c' &bogus; &"
        );
        assert_eq!(strip_tags("<p>hi <a href=\"x\">there</a></p>"), "hi there");
    }

    #[test]
    fn relative_links_resolve_against_the_final_page() {
        let base = url::Url::parse("https://ex.com/a/b.html").unwrap();
        assert_eq!(absolute(&base, "/i.png"), "https://ex.com/i.png");
        assert_eq!(absolute(&base, "c.png"), "https://ex.com/a/c.png");
        assert_eq!(
            absolute(&base, "https://cdn.x/y.png"),
            "https://cdn.x/y.png"
        );
    }
}
