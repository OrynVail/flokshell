use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::bus::{Bus, Request};
use crate::{config, http};

const SERVICE: &str = "coverart";
const TIMEOUT: Duration = Duration::from_secs(8);
const MAX_BYTES: usize = 8 * 1024 * 1024;
const CACHE_MAX: usize = 300;

const SIDECAR_STEMS: [&str; 6] = ["cover", "folder", "front", "album", "albumart", "artwork"];
const SIDECAR_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct State {
    // Echoes the request, so a reply for a track already skipped is ignored.
    key: String,
    ok: bool,
    path: String,
    source: String,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(default)]
struct Track {
    key: String,
    art_url: String,
    track_url: String,
    title: String,
    artist: String,
    album: String,
    embedded: bool,
    online: bool,
}

pub struct Service {
    bus: Bus,
    http: reqwest::Client,
}

impl Service {
    pub fn new(bus: Bus) -> Result<Self> {
        Ok(Self {
            bus,
            http: http::client("flokshell", TIMEOUT)?,
        })
    }

    pub async fn run(self, mut requests: mpsc::Receiver<Request>) {
        let dir = config::cache_dir().join("coverart");
        let mut pending: Option<JoinHandle<()>> = None;

        while let Some(req) = requests.recv().await {
            if req.cmd != "resolve" {
                tracing::warn!("unknown command: {}", req.cmd);
                continue;
            }
            let track: Track = match serde_json::from_value(req.args) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("bad resolve: {e}");
                    continue;
                }
            };
            // A newer track makes the old lookup worthless, network included.
            if let Some(p) = pending.take() {
                p.abort();
            }
            let (bus, http, dir) = (self.bus.clone(), self.http.clone(), dir.clone());
            pending = Some(tokio::spawn(async move {
                let state = resolve(&http, &dir, track).await;
                match serde_json::to_value(&state) {
                    Ok(v) => bus.publish(SERVICE, &v),
                    Err(e) => tracing::error!("serialise failed: {e}"),
                }
            }));
        }
    }
}

async fn resolve(http: &reqwest::Client, dir: &Path, t: Track) -> State {
    let miss = State {
        key: t.key.clone(),
        source: "none".into(),
        ..Default::default()
    };

    let mut found = from_art_url(http, &t.art_url).await.map(|d| ("artUrl", d));
    let local = file_path(&t.track_url);
    if found.is_none()
        && t.embedded
        && let Some(path) = &local
    {
        let p = path.clone();
        found = tokio::task::spawn_blocking(move || embedded(&p))
            .await
            .ok()
            .flatten()
            .map(|d| ("embedded", d));
        if found.is_none() {
            found = ffmpeg_picture(path).await.map(|d| ("embedded", d));
        }
        if found.is_none() {
            let p = path.clone();
            found = tokio::task::spawn_blocking(move || sidecar(&p))
                .await
                .ok()
                .flatten()
                .map(|d| ("sidecar", d));
        }
    }
    if found.is_none() && t.online {
        found = online(http, &t.artist, &t.album, &t.title)
            .await
            .map(|d| ("online", d));
    }

    let Some((source, data)) = found else {
        return miss;
    };
    let dir = dir.to_path_buf();
    match tokio::task::spawn_blocking(move || store(&dir, &data)).await {
        Ok(Some(path)) => State {
            key: t.key,
            ok: true,
            path: path.to_string_lossy().into_owned(),
            source: source.into(),
        },
        _ => miss,
    }
}

fn sniff(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if data.len() >= 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("webp")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("gif")
    } else {
        None
    }
}

fn image(data: Vec<u8>) -> Option<Vec<u8>> {
    (sniff(&data).is_some() && data.len() < MAX_BYTES).then_some(data)
}

// Content-addressed: Qt caches pixmaps by URL, so a reused path shows stale art.
fn store(dir: &Path, data: &[u8]) -> Option<PathBuf> {
    let ext = sniff(data)?;
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{}.{ext}", hex(&Sha256::digest(data))));
    if !path.exists() {
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, data).ok()?;
        std::fs::rename(&tmp, &path).ok()?;
        prune(dir);
    }
    Some(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((meta.modified().ok()?, e.path()))
        })
        .collect();
    if files.len() <= CACHE_MAX {
        return;
    }
    files.sort();
    let excess = files.len() - CACHE_MAX;
    for (_, path) in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
}

fn file_path(url: &str) -> Option<PathBuf> {
    url::Url::parse(url)
        .ok()
        .filter(|u| u.scheme() == "file")?
        .to_file_path()
        .ok()
}

fn read_file(path: &Path) -> Option<Vec<u8>> {
    if std::fs::metadata(path).ok()?.len() as usize >= MAX_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

fn data_url(url: &str) -> Option<Vec<u8>> {
    let (head, payload) = url.strip_prefix("data:")?.split_once(',')?;
    if head.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .ok()
    } else {
        Some(percent_encoding::percent_decode_str(payload).collect())
    }
}

async fn from_art_url(http: &reqwest::Client, url: &str) -> Option<Vec<u8>> {
    let data = if url.starts_with("data:") {
        data_url(url)
    } else if url.starts_with("http://") || url.starts_with("https://") {
        http::get(http, url, MAX_BYTES).await.ok().map(|p| p.body)
    } else {
        let path = file_path(url).or_else(|| url.starts_with('/').then(|| PathBuf::from(url)))?;
        tokio::task::spawn_blocking(move || read_file(&path))
            .await
            .ok()
            .flatten()
    }?;
    image(data)
}

fn embedded(path: &Path) -> Option<Vec<u8>> {
    let tagged = lofty::read_from_path(path).ok()?;
    let pictures: Vec<_> = tagged.tags().iter().flat_map(|t| t.pictures()).collect();
    let pick = pictures
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or(pictures.first())?;
    image(pick.data().to_vec())
}

// Containers lofty does not read (video with an attached cover) still have one.
async fn ffmpeg_picture(path: &Path) -> Option<Vec<u8>> {
    if !path.is_file() {
        return None;
    }
    for codec in [&["-c", "copy"][..], &["-c:v", "mjpeg"][..]] {
        let out = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new("ffmpeg")
                .args(["-nostdin", "-loglevel", "error", "-i"])
                .arg(path)
                .args(["-map", "0:v:0", "-frames:v", "1", "-f", "image2"])
                .args(codec)
                .arg("pipe:1")
                .kill_on_drop(true)
                .output(),
        )
        .await
        .ok()?
        .ok()?;
        if let Some(d) = image(out.stdout) {
            return Some(d);
        }
    }
    None
}

fn sidecar(track: &Path) -> Option<Vec<u8>> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(track.parent()?)
        .ok()?
        .filter_map(|e| Some(e.ok()?.path()))
        .collect();
    entries.sort();
    entries.into_iter().find_map(|p| {
        let stem = p.file_stem()?.to_str()?.to_lowercase();
        let ext = p.extension()?.to_str()?.to_lowercase();
        if SIDECAR_STEMS.contains(&stem.as_str()) && SIDECAR_EXTS.contains(&ext.as_str()) {
            image(read_file(&p)?)
        } else {
            None
        }
    })
}

#[derive(Deserialize)]
struct Deezer {
    #[serde(default)]
    data: Vec<DeezerHit>,
}

#[derive(Deserialize)]
struct DeezerHit {
    album: Option<DeezerAlbum>,
}

#[derive(Deserialize)]
struct DeezerAlbum {
    cover_xl: Option<String>,
}

#[derive(Deserialize)]
struct Itunes {
    #[serde(default)]
    results: Vec<ItunesHit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ItunesHit {
    artwork_url100: Option<String>,
}

// No artist means a browser tab title, which matches arbitrary garbage.
async fn online(http: &reqwest::Client, artist: &str, album: &str, title: &str) -> Option<Vec<u8>> {
    if artist.is_empty() {
        return None;
    }
    let term = format!("{artist} {}", if album.is_empty() { title } else { album });
    let term = term.trim();

    let art = async {
        let r: Deezer = http
            .get("https://api.deezer.com/search")
            .query(&[("limit", "1"), ("q", term)])
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        r.data.into_iter().find_map(|h| h.album?.cover_xl)
    }
    .await;
    let art = match art {
        Some(a) => Some(a),
        None => {
            let r: Itunes = http
                .get("https://itunes.apple.com/search")
                .query(&[("entity", "album"), ("limit", "1"), ("term", term)])
                .send()
                .await
                .ok()?
                .json()
                .await
                .ok()?;
            r.results
                .into_iter()
                .find_map(|h| h.artwork_url100)
                .map(|a| a.replace("100x100", "600x600"))
        }
    }?;
    image(http::get(http, &art, MAX_BYTES).await.ok()?.body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_knows_the_four_formats_and_nothing_else() {
        assert_eq!(sniff(b"\xff\xd8\xff\xe0...."), Some("jpg"));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("png"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(sniff(b"GIF89a.."), Some("gif"));
        assert_eq!(sniff(b"<html>"), None);
        assert_eq!(sniff(b"RIFF"), None);
    }

    #[test]
    fn data_urls_in_both_encodings() {
        assert_eq!(
            data_url("data:image/png;base64,iVBORw==").unwrap(),
            b"\x89PNG"
        );
        assert_eq!(data_url("data:text/plain,a%20b").unwrap(), b"a b");
        assert!(data_url("data:nocomma").is_none());
    }

    #[test]
    fn file_urls_are_decoded() {
        assert_eq!(
            file_path("file:///home/o/Music/A%20B/01.flac").unwrap(),
            PathBuf::from("/home/o/Music/A B/01.flac")
        );
        assert!(file_path("https://x/y.mp3").is_none());
        assert!(file_path("").is_none());
    }

    #[test]
    fn cache_names_are_content_hashes() {
        let dir = std::env::temp_dir().join(format!("flokd-coverart-{}", std::process::id()));
        let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
        let a = store(&dir, &png).unwrap();
        let b = store(&dir, &png).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.extension().unwrap(), "png");
        assert_eq!(a.file_stem().unwrap().len(), 64);
        assert!(store(&dir, b"not an image").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn requests_tolerate_missing_fields() {
        let t: Track =
            serde_json::from_value(serde_json::json!({"key": "k", "art_url": "x"})).unwrap();
        assert_eq!(t.key, "k");
        assert!(!t.online);
    }
}
