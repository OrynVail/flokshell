use std::time::Duration;

use anyhow::{Result, bail};

pub fn client(user_agent: &str, timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(user_agent)
        .timeout(timeout)
        .build()?)
}

pub struct Page {
    pub url: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

// Stops reading at `max` instead of failing: a page's head is all a preview
// needs, and an image past the cap is refused by the caller anyway.
pub async fn get(client: &reqwest::Client, url: &str, max: usize) -> Result<Page> {
    let mut resp = client.get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        bail!("HTTP {}", status.as_u16());
    }
    let page_url = resp.url().to_string();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        let room = max - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(room)]);
        if body.len() >= max {
            break;
        }
    }
    Ok(Page {
        url: page_url,
        content_type,
        body,
    })
}
