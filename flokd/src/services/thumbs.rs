use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use image::DynamicImage;
use image::codecs::jpeg::JpegEncoder;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};

use crate::bus::{Bus, Request};

const SERVICE: &str = "thumbs";

const VIDEO: [&str; 5] = ["mp4", "webm", "mov", "avi", "mkv"];
const IMAGE: [&str; 7] = ["jpg", "jpeg", "png", "webp", "tif", "tiff", "bmp"];
const GIF: &str = "gif";

const WALLPAPER_SIZE: u32 = 140;
const DESKTOP_SIZE: u32 = 64;
const QUALITY: u8 = 85;
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(30);

// Thumbnails are named after the source's basename, so a renamed file could
// inherit the thumbnail of a deleted one with the same name. The manifest
// records which source, at which mtime and size, each one was built from.
const MANIFEST: &str = ".manifest.json";

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct State {
    // Bumped when a wallpaper batch lands; the shell reloads its thumbnails on it.
    wallpapers: u64,
    scan: Scan,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Scan {
    dir: String,
    // The configured folder held nothing, so these are the bundled examples.
    fallback: bool,
    files: Vec<String>,
    // Top-level subfolders, the wallpaper tab's filters.
    folders: Vec<String>,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", content = "args", rename_all = "lowercase")]
enum Command {
    Scan {
        dir: String,
        fallback: String,
        cache: String,
    },
    Wallpapers {
        dir: String,
        cache: String,
    },
    Desktop {
        dir: String,
        cache: String,
    },
    Lockframe {
        path: String,
        cache: String,
    },
}

impl TryFrom<Request> for Command {
    type Error = serde_json::Error;

    fn try_from(req: Request) -> Result<Self, Self::Error> {
        let mut v = serde_json::json!({ "cmd": req.cmd });
        if !req.args.is_null() {
            v["args"] = req.args;
        }
        serde_json::from_value(v)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Video,
    Image,
    Gif,
}

fn kind(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if VIDEO.contains(&ext.as_str()) {
        Some(Kind::Video)
    } else if IMAGE.contains(&ext.as_str()) {
        Some(Kind::Image)
    } else if ext == GIF {
        Some(Kind::Gif)
    } else {
        None
    }
}

pub struct Service {
    bus: Bus,
    state: Arc<std::sync::Mutex<State>>,
    // One batch per kind at a time; a request arriving mid-batch runs after it.
    wallpapers: Arc<Mutex<()>>,
    desktop: Arc<Mutex<()>>,
    lockframe: Arc<Mutex<()>>,
}

impl Service {
    pub fn new(bus: Bus) -> Self {
        Self {
            bus,
            state: Arc::default(),
            wallpapers: Arc::default(),
            desktop: Arc::default(),
            lockframe: Arc::default(),
        }
    }

    pub async fn run(self, mut requests: mpsc::Receiver<Request>) {
        self.publish();
        let mut watched: Option<Watched> = None;
        let (changed_tx, mut changed) = mpsc::unbounded_channel();
        loop {
            let req = tokio::select! {
                Some(req) = requests.recv() => req,
                Some(()) = changed.recv() => {
                    // A copy or an unpack lands as a burst; wait for it to settle.
                    while tokio::time::timeout(SETTLE, changed.recv()).await.is_ok() {}
                    if let Some(w) = &watched {
                        self.rescan(w);
                    }
                    continue;
                }
                else => return,
            };
            match Command::try_from(req) {
                Ok(Command::Scan {
                    dir,
                    fallback,
                    cache,
                }) => {
                    let w = Watched {
                        dir: PathBuf::from(&dir),
                        fallback: PathBuf::from(fallback),
                        cache: PathBuf::from(cache),
                        _watcher: watch(Path::new(&dir), changed_tx.clone())
                            .inspect_err(|e| {
                                tracing::warn!(dir, "wallpaper folder unwatched: {e:#}")
                            })
                            .ok(),
                    };
                    self.rescan(&w);
                    watched = Some(w);
                }
                Ok(Command::Wallpapers { dir, cache }) => {
                    let (lock, bus, state) = (
                        self.wallpapers.clone(),
                        self.bus.clone(),
                        self.state.clone(),
                    );
                    tokio::spawn(async move {
                        let _held = lock.lock().await;
                        let root = PathBuf::from(cache).join("thumbnails");
                        match wallpapers(Path::new(&dir), &root).await {
                            Ok(n) => tracing::info!(built = n, "wallpaper thumbnails"),
                            Err(e) => tracing::warn!(dir, "wallpaper thumbnails: {e:#}"),
                        }
                        let snapshot = {
                            let mut s = state.lock().unwrap();
                            s.wallpapers += 1;
                            s.clone()
                        };
                        publish(&bus, &snapshot);
                    });
                }
                Ok(Command::Desktop { dir, cache }) => {
                    let lock = self.desktop.clone();
                    tokio::spawn(async move {
                        let _held = lock.lock().await;
                        match desktop(Path::new(&dir), Path::new(&cache)).await {
                            Ok(n) => tracing::debug!(built = n, "desktop thumbnails"),
                            Err(e) => tracing::warn!(dir, "desktop thumbnails: {e:#}"),
                        }
                    });
                }
                Ok(Command::Lockframe { path, cache }) => {
                    let lock = self.lockframe.clone();
                    tokio::spawn(async move {
                        let _held = lock.lock().await;
                        let dir = PathBuf::from(cache).join("lockscreen");
                        if let Err(e) = lockframe(Path::new(&path), &dir).await {
                            tracing::warn!(path, "lockscreen frame: {e:#}");
                        }
                    });
                }
                Err(e) => tracing::warn!("bad command: {e}"),
            }
        }
    }

    fn rescan(&self, w: &Watched) {
        let (dir, fallback, cache) = (w.dir.clone(), w.fallback.clone(), w.cache.clone());
        let (lock, bus, state) = (
            self.wallpapers.clone(),
            self.bus.clone(),
            self.state.clone(),
        );
        tokio::spawn(async move {
            let scan = match tokio::task::spawn_blocking(move || scan(&dir, &fallback)).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("scan panicked: {e}");
                    return;
                }
            };
            let used = PathBuf::from(&scan.dir);
            let snapshot = {
                let mut s = state.lock().unwrap();
                s.scan = scan;
                s.clone()
            };
            publish(&bus, &snapshot);

            let _held = lock.lock().await;
            match wallpapers(&used, &cache.join("thumbnails")).await {
                Ok(n) => tracing::info!(built = n, "wallpaper thumbnails"),
                Err(e) => tracing::warn!("wallpaper thumbnails: {e:#}"),
            }
            let snapshot = {
                let mut s = state.lock().unwrap();
                s.wallpapers += 1;
                s.clone()
            };
            publish(&bus, &snapshot);
        });
    }

    fn publish(&self) {
        let snapshot = self.state.lock().unwrap().clone();
        publish(&self.bus, &snapshot);
    }
}

fn publish(bus: &Bus, state: &State) {
    match serde_json::to_value(state) {
        Ok(v) => bus.publish(SERVICE, &v),
        Err(e) => tracing::error!("serialise failed: {e}"),
    }
}

const SETTLE: Duration = Duration::from_millis(500);

struct Watched {
    dir: PathBuf,
    fallback: PathBuf,
    cache: PathBuf,
    // Dropping it stops the watch, so it lives as long as this scan target.
    _watcher: Option<notify::RecommendedWatcher>,
}

fn watch(dir: &Path, tx: mpsc::UnboundedSender<()>) -> Result<notify::RecommendedWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(e) = res
            && !matches!(e.kind, EventKind::Access(_))
        {
            let _ = tx.send(());
        }
    })?;
    w.watch(dir, RecursiveMode::Recursive)?;
    Ok(w)
}

fn scan(dir: &Path, fallback: &Path) -> Scan {
    let list = |d: &Path| {
        let mut files = Vec::new();
        walk(d, &mut files);
        let mut files: Vec<String> = files
            .into_iter()
            .filter_map(|p| p.to_str().map(str::to_string))
            .collect();
        files.sort();
        files
    };
    let files = list(dir);
    if files.is_empty() && dir != fallback {
        return Scan {
            dir: fallback.to_string_lossy().into_owned(),
            fallback: true,
            files: list(fallback),
            folders: Vec::new(),
        };
    }
    let mut folders: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !hidden(&e.path()))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    folders.sort();
    Scan {
        dir: dir.to_string_lossy().into_owned(),
        fallback: false,
        files,
        folders,
    }
}

fn workers() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4)
}

// Thumbnail of `name.ext` is `name.ext.jpg`, as the shell computes it.
fn thumb_name(src: &Path) -> Option<String> {
    Some(format!("{}.jpg", src.file_name()?.to_str()?))
}

fn hidden(p: &Path) -> bool {
    p.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if hidden(&p) {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&p, out),
            Ok(t) if t.is_file() && kind(&p).is_some() => out.push(p),
            _ => {}
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Stamp {
    src: String,
    mtime: f64,
    size: u64,
}

fn stamp(p: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(p).ok()?;
    Some(Stamp {
        src: p.to_str()?.to_string(),
        mtime: m
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs_f64(),
        size: m.len(),
    })
}

// The manifest predates flokd; Python's float mtime can differ in the last bit.
fn same(a: &Stamp, b: &Stamp) -> bool {
    a.src == b.src && a.size == b.size && (a.mtime - b.mtime).abs() < 1e-6
}

async fn wallpapers(dir: &Path, root: &Path) -> Result<usize> {
    let (dir, root) = (dir.to_path_buf(), root.to_path_buf());
    let (jobs, mut manifest) = tokio::task::spawn_blocking({
        let (dir, root) = (dir.clone(), root.clone());
        move || plan_wallpapers(&dir, &root)
    })
    .await??;

    let built = run_jobs(&jobs, WALLPAPER_SIZE).await;
    for ((source, thumb), ok) in jobs.iter().zip(&built) {
        if *ok && let (Some(s), Some(k)) = (stamp(source), key(&root, thumb)) {
            manifest.insert(k, s);
        }
    }
    let n = built.iter().filter(|b| **b).count();
    tokio::task::spawn_blocking(move || save_manifest(&root, &manifest)).await??;
    Ok(n)
}

type Job = (PathBuf, PathBuf);

fn key(root: &Path, thumb: &Path) -> Option<String> {
    Some(thumb.strip_prefix(root).ok()?.to_str()?.to_string())
}

fn plan_wallpapers(dir: &Path, root: &Path) -> Result<(Vec<Job>, HashMap<String, Stamp>)> {
    if !dir.is_dir() {
        bail!("not a directory");
    }
    std::fs::create_dir_all(root)?;
    let mut manifest: HashMap<String, Stamp> = std::fs::read(root.join(MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();

    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();

    let wanted: HashMap<String, (PathBuf, PathBuf)> = files
        .into_iter()
        .filter_map(|src| {
            let rel = src.strip_prefix(dir).ok()?.parent()?.to_path_buf();
            let thumb = root.join(rel).join(thumb_name(&src)?);
            Some((key(root, &thumb)?, (src, thumb)))
        })
        .collect();

    // Orphans first, or a stale thumbnail could pass for a current one.
    prune(root, root, &wanted);
    manifest.retain(|k, _| root.join(k).exists());

    let mut jobs: Vec<Job> = wanted
        .into_iter()
        .filter(|(k, (src, thumb))| {
            !thumb.exists()
                || match (manifest.get(k), stamp(src)) {
                    (Some(old), Some(now)) => !same(old, &now),
                    _ => true,
                }
        })
        .map(|(_, job)| job)
        .collect();
    jobs.sort();
    Ok((jobs, manifest))
}

fn prune(root: &Path, dir: &Path, wanted: &HashMap<String, (PathBuf, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            prune(root, &p, wanted);
            let _ = std::fs::remove_dir(&p); // only when empty
        } else if !p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(MANIFEST))
            && key(root, &p).is_none_or(|k| !wanted.contains_key(&k))
        {
            let _ = std::fs::remove_file(&p);
        }
    }
}

fn save_manifest(root: &Path, manifest: &HashMap<String, Stamp>) -> Result<()> {
    let tmp = root.join(format!("{MANIFEST}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(manifest)?)?;
    std::fs::rename(tmp, root.join(MANIFEST))?;
    Ok(())
}

async fn desktop(dir: &Path, cache: &Path) -> Result<usize> {
    std::fs::create_dir_all(cache)?;
    let mut jobs: Vec<Job> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && kind(p).is_some())
        .filter_map(|src| {
            let thumb = cache.join(thumb_name(&src)?);
            let stale = match (modified(&src), modified(&thumb)) {
                (Some(s), Some(t)) => s > t,
                _ => true,
            };
            stale.then_some((src, thumb))
        })
        .collect();
    jobs.sort();
    Ok(run_jobs(&jobs, DESKTOP_SIZE)
        .await
        .iter()
        .filter(|b| **b)
        .count())
}

fn modified(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

async fn run_jobs(jobs: &[Job], size: u32) -> Vec<bool> {
    let mut done: Vec<(usize, bool)> = futures_util::stream::iter(jobs.iter().cloned().enumerate())
        .map(|(i, (src, dst))| async move {
            let r = thumbnail(&src, &dst, size).await;
            if let Err(e) = &r {
                tracing::debug!(src = %src.display(), "thumbnail failed: {e:#}");
            }
            (i, r.is_ok())
        })
        .buffer_unordered(workers())
        .collect()
        .await;
    done.sort();
    done.into_iter().map(|(_, ok)| ok).collect()
}

async fn thumbnail(src: &Path, dst: &Path, size: u32) -> Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match kind(src).context("unsupported")? {
        Kind::Video => {
            let crop = format!(
                "scale={size}:{size}:force_original_aspect_ratio=increase,crop={size}:{size}"
            );
            // A second in skips fade-from-black intros; clips shorter than that
            // get their first frame instead.
            if ffmpeg(src, dst, Some("1"), &["-vf", &crop]).await.is_err() {
                ffmpeg(src, dst, None, &["-vf", &crop]).await?;
            }
            Ok(())
        }
        Kind::Image | Kind::Gif => {
            let (src, dst) = (src.to_path_buf(), dst.to_path_buf());
            tokio::task::spawn_blocking(move || {
                let img = image::ImageReader::open(&src)?
                    .with_guessed_format()?
                    .decode()?;
                write_jpeg(&cover(&img, size), &dst)
            })
            .await?
        }
    }
}

// Scale to fill a size×size square, then cut the centre out of it.
fn cover(img: &DynamicImage, size: u32) -> DynamicImage {
    let (w, h) = (img.width().max(1), img.height().max(1));
    let scale = (size as f64 / w as f64).max(size as f64 / h as f64);
    let (nw, nh) = (
        ((w as f64 * scale).ceil() as u32).max(size),
        ((h as f64 * scale).ceil() as u32).max(size),
    );
    img.thumbnail_exact(nw, nh)
        .crop_imm((nw - size) / 2, (nh - size) / 2, size, size)
}

fn write_jpeg(img: &DynamicImage, dst: &Path) -> Result<()> {
    let tmp = dst.with_extension("part");
    let file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
    img.to_rgb8()
        .write_with_encoder(JpegEncoder::new_with_quality(file, QUALITY))?;
    std::fs::rename(tmp, dst)?;
    Ok(())
}

async fn ffmpeg(src: &Path, dst: &Path, seek: Option<&str>, filter: &[&str]) -> Result<()> {
    let tmp = dst.with_extension("part");
    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.args(["-nostdin", "-loglevel", "error", "-y"]);
    if let Some(at) = seek {
        cmd.args(["-ss", at]);
    }
    cmd.arg("-i")
        .arg(src)
        .args(["-frames:v", "1"])
        .args(filter)
        .args(["-q:v", "2", "-f", "image2"])
        .arg(&tmp)
        .kill_on_drop(true);
    let out = tokio::time::timeout(FFMPEG_TIMEOUT, cmd.output())
        .await
        .context("timed out")??;
    if !out.status.success() || !tmp.exists() {
        let _ = std::fs::remove_file(&tmp);
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    std::fs::rename(tmp, dst)?;
    Ok(())
}

// Still images are used as they are; only video and GIF need a frame.
async fn lockframe(src: &Path, dir: &Path) -> Result<()> {
    let k = kind(src).context("unsupported")?;
    if k == Kind::Image {
        return Ok(());
    }
    if !src.is_file() {
        bail!("not found");
    }
    std::fs::create_dir_all(dir)?;
    for e in std::fs::read_dir(dir)?.flatten() {
        if e.path().is_file() {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let dst = dir.join(thumb_name(src).context("bad name")?);
    if k == Kind::Video {
        return ffmpeg(src, &dst, None, &[]).await;
    }
    let src = src.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let img = image::ImageReader::open(&src)?
            .with_guessed_format()?
            .decode()?;
        write_jpeg(&img, &dst)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_extension() {
        assert_eq!(kind(Path::new("a/b.MP4")), Some(Kind::Video));
        assert_eq!(kind(Path::new("a.jpeg")), Some(Kind::Image));
        assert_eq!(kind(Path::new("a.gif")), Some(Kind::Gif));
        assert_eq!(kind(Path::new("a.txt")), None);
        assert_eq!(kind(Path::new("noext")), None);
    }

    #[test]
    fn thumbnail_names_keep_the_extension() {
        assert_eq!(
            thumb_name(Path::new("/w/dark forest.jpeg")).unwrap(),
            "dark forest.jpeg.jpg"
        );
    }

    #[test]
    fn cover_fills_and_centres() {
        let wide = DynamicImage::new_rgb8(400, 100);
        let t = cover(&wide, 64);
        assert_eq!((t.width(), t.height()), (64, 64));
        let tiny = DynamicImage::new_rgb8(10, 30);
        let t = cover(&tiny, 140);
        assert_eq!((t.width(), t.height()), (140, 140));
    }

    #[test]
    fn python_manifest_stamps_still_match() {
        let old: Stamp =
            serde_json::from_str(r#"{"src":"/w/a.jpg","mtime":1786000000.1234567,"size":42}"#)
                .unwrap();
        let now = Stamp {
            src: "/w/a.jpg".into(),
            mtime: 1786000000.1234568,
            size: 42,
        };
        assert!(same(&old, &now));
        assert!(!same(
            &old,
            &Stamp {
                size: 43,
                ..now.clone()
            }
        ));
        assert!(!same(
            &old,
            &Stamp {
                src: "/w/b.jpg".into(),
                ..now
            }
        ));
    }

    #[test]
    fn wallpaper_plan_prunes_orphans_and_rebuilds_the_stale() {
        let base = std::env::temp_dir().join(format!("flokd-thumbs-{}", std::process::id()));
        let (walls, root) = (base.join("walls"), base.join("cache/thumbnails"));
        std::fs::create_dir_all(walls.join("sub")).unwrap();
        std::fs::create_dir_all(walls.join(".hidden")).unwrap();
        std::fs::create_dir_all(root.join("gone")).unwrap();
        std::fs::write(walls.join("a.png"), b"x").unwrap();
        std::fs::write(walls.join("sub/b.mp4"), b"x").unwrap();
        std::fs::write(walls.join(".hidden/c.png"), b"x").unwrap();
        std::fs::write(walls.join("notes.txt"), b"x").unwrap();
        std::fs::write(root.join("gone/old.png.jpg"), b"x").unwrap();
        std::fs::write(root.join("a.png.jpg"), b"x").unwrap();

        let (jobs, _) = plan_wallpapers(&walls, &root).unwrap();
        let dsts: Vec<_> = jobs.iter().map(|(_, d)| key(&root, d).unwrap()).collect();
        // a.png has a thumbnail but no manifest row, so it is rebuilt once.
        assert_eq!(dsts, ["a.png.jpg", "sub/b.mp4.jpg"]);
        assert!(!root.join("gone").exists());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn scan_lists_media_and_folders_or_falls_back() {
        let base = std::env::temp_dir().join(format!("flokd-scan-{}", std::process::id()));
        let (walls, example, empty) =
            (base.join("walls"), base.join("example"), base.join("empty"));
        for d in [
            walls.join("cats"),
            walls.join(".git"),
            example.clone(),
            empty.clone(),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(walls.join("b.PNG"), b"x").unwrap();
        std::fs::write(walls.join("cats/a.mp4"), b"x").unwrap();
        std::fs::write(walls.join(".git/x.png"), b"x").unwrap();
        std::fs::write(walls.join("readme.md"), b"x").unwrap();
        std::fs::write(example.join("e.jpg"), b"x").unwrap();

        let s = scan(&walls, &example);
        assert!(!s.fallback);
        assert_eq!(s.folders, ["cats"]);
        let names: Vec<_> = s
            .files
            .iter()
            .map(|f| f.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(names, ["b.PNG", "a.mp4"]);

        let s = scan(&empty, &example);
        assert!(s.fallback);
        assert!(s.files[0].ends_with("e.jpg"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn commands_parse() {
        let r = Request {
            cmd: "lockframe".into(),
            args: serde_json::json!({"path": "/w/a.mp4", "cache": "/c"}),
        };
        assert_eq!(
            Command::try_from(r).unwrap(),
            Command::Lockframe {
                path: "/w/a.mp4".into(),
                cache: "/c".into()
            }
        );
    }
}
