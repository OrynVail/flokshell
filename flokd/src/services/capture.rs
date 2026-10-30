use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ImageEncoder, RgbaImage};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};
use zbus::Connection;

use crate::bus::{Bus, Request};
use crate::datacontrol;
use crate::notify::{self, Notice};
use crate::wayland::{self, Frame};

const SERVICE: &str = "capture";
// The overlay loads these by name; the paths predate flokd.
const FREEZE: &str = "/tmp/flokshell_freeze";
const SWATCH: &str = "/tmp/flokshell_color.png";
const LENS_UPLOAD: &str = "https://uguu.se/upload";
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct State {
    // Bumped per freeze, so the overlay can tell a new set of frames from the last.
    seq: u64,
    outputs: Vec<Output>,
    dir: String,
    last: Outcome,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Output {
    name: String,
    path: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct Outcome {
    seq: u64,
    mode: String,
    ok: bool,
    path: String,
    error: String,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", content = "args", rename_all = "lowercase")]
enum Command {
    Freeze,
    Release,
    Act {
        mode: String,
        #[serde(default)]
        output: String,
        #[serde(default)]
        x: i32,
        #[serde(default)]
        y: i32,
        #[serde(default)]
        w: i32,
        #[serde(default)]
        h: i32,
        #[serde(default)]
        langs: String,
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

pub struct Service {
    bus: Bus,
    wayland: wayland::Handle,
    clip: datacontrol::Handle,
    http: reqwest::Client,
    state: State,
    frames: Vec<Frame>,
    // Colour notifications still open, by id, with what each action copies.
    swatches: HashMap<u32, [String; 3]>,
}

impl Service {
    pub fn new(bus: Bus, wayland: wayland::Handle, clip: datacontrol::Handle) -> Result<Self> {
        Ok(Self {
            bus,
            wayland,
            clip,
            http: crate::http::client("flokshell", Duration::from_secs(30))?,
            state: State {
                dir: pictures()
                    .join("Screenshots")
                    .to_string_lossy()
                    .into_owned(),
                ..Default::default()
            },
            frames: Vec::new(),
            swatches: HashMap::new(),
        })
    }

    pub async fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        self.publish();
        let session = Connection::session().await.ok();
        let mut clicks = match &session {
            Some(c) => notify::actions(c).await.ok().map(Box::pin),
            None => None,
        };

        loop {
            tokio::select! {
                Some(req) = requests.recv() => match Command::try_from(req) {
                    Ok(Command::Freeze) => self.freeze().await,
                    Ok(Command::Release) => self.frames.clear(),
                    Ok(Command::Act { mode, output, x, y, w, h, langs }) => {
                        let result = self.act(session.as_ref(), &mode, &output, (x, y, w, h), &langs).await;
                        if let Err(e) = &result {
                            tracing::warn!(mode, "capture: {e:#}");
                        }
                        self.state.last = Outcome {
                            seq: self.state.seq,
                            mode,
                            ok: result.is_ok(),
                            path: result.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                            error: result.err().map(|e| format!("{e:#}")).unwrap_or_default(),
                        };
                        self.publish();
                    }
                    Err(e) => tracing::warn!("bad command: {e}"),
                },
                Some((id, key)) = next(&mut clicks) => {
                    if let Some(values) = self.swatches.remove(&id) {
                        let text = match key.as_str() {
                            "rgb" => &values[1],
                            "hsv" => &values[2],
                            _ => &values[0],
                        };
                        self.clip.text(text);
                    }
                }
            }
        }
    }

    async fn freeze(&mut self) {
        let (tx, rx) = oneshot::channel();
        self.wayland.send(wayland::Request::Capture(tx));
        let frames = match rx.await.map_err(anyhow::Error::from).and_then(|r| r) {
            Ok(f) => f,
            Err(e) => {
                let e = format!("{e:#}");
                tracing::warn!("freeze: {e}");
                self.state.last = Outcome {
                    mode: "freeze".into(),
                    error: e,
                    ..Default::default()
                };
                self.publish();
                return;
            }
        };

        // The overlay reads these from disk. PNG at the fastest setting: they
        // are thrown away within seconds.
        let jobs: Vec<_> = frames
            .iter()
            .map(|f| {
                let path = format!("{FREEZE}_{}.png", f.name);
                let image = f.image.clone();
                let p = path.clone();
                (
                    path,
                    tokio::task::spawn_blocking(move || write_png(&image, Path::new(&p), true)),
                )
            })
            .collect();
        let mut outputs = Vec::new();
        for ((path, job), f) in jobs.into_iter().zip(&frames) {
            match job.await {
                Ok(Ok(())) => outputs.push(Output {
                    name: f.name.clone(),
                    path,
                    x: f.x,
                    y: f.y,
                    w: f.w,
                    h: f.h,
                }),
                Ok(Err(e)) => tracing::warn!(output = f.name, "freeze write: {e:#}"),
                Err(e) => tracing::error!("freeze write panicked: {e}"),
            }
        }
        self.frames = frames;
        self.state.seq += 1;
        self.state.outputs = outputs;
        self.publish();
    }

    async fn act(
        &mut self,
        session: Option<&Connection>,
        mode: &str,
        output: &str,
        (x, y, w, h): (i32, i32, i32, i32),
        langs: &str,
    ) -> Result<PathBuf> {
        let frame = if output.is_empty() {
            self.frames.iter().find(|f| contains(f, x, y))
        } else {
            self.frames.iter().find(|f| f.name == output)
        }
        .context("nothing frozen at that point")?;

        if mode == "color" {
            let px = pixel(frame, x, y).context("point outside the frame")?;
            return self.color(session, px).await;
        }

        let crop = if output.is_empty() {
            crop(frame, x, y, w, h).context("empty selection")?
        } else {
            frame.image.clone()
        };
        let png = tokio::task::spawn_blocking(move || encode_png(&crop, false)).await??;

        match mode {
            "save" => {
                let dir = PathBuf::from(&self.state.dir);
                std::fs::create_dir_all(&dir)?;
                let path = unique(&dir, &format!("Screenshot_{}", timestamp()));
                std::fs::write(&path, &png)?;
                self.clip.bytes("image/png", &png);
                Ok(path)
            }
            "ocr" => {
                let langs = if langs.is_empty() { "eng" } else { langs };
                let text = tool("tesseract", &["-", "-", "-l", langs], &png).await?;
                self.deliver(
                    session,
                    "OCR Result",
                    text.trim(),
                    "edit-paste",
                    "Text copied to clipboard",
                    "No text detected",
                )
                .await
            }
            "qr" => {
                let text = tool("zbarimg", &["-q", "--raw", "-"], &png)
                    .await
                    .unwrap_or_default();
                self.deliver(
                    session,
                    "QR/Barcode Result",
                    text.trim_end_matches('\n'),
                    "qr-code",
                    "Content copied to clipboard",
                    "No code detected",
                )
                .await
            }
            "lens" => {
                if let Some(c) = session {
                    let _ = notify::send(
                        c,
                        Notice {
                            summary: "Google Lens",
                            body: "Uploading image for analysis…",
                            ..Default::default()
                        },
                    )
                    .await;
                }
                let link = upload(&self.http, png).await?;
                let url = format!("https://lens.google.com/uploadbyurl?url={link}");
                tokio::process::Command::new("xdg-open").arg(&url).spawn()?;
                Ok(PathBuf::new())
            }
            m => bail!("unknown mode {m}"),
        }
    }

    async fn deliver(
        &self,
        session: Option<&Connection>,
        summary: &str,
        text: &str,
        icon: &str,
        found: &str,
        missing: &str,
    ) -> Result<PathBuf> {
        let empty = text.is_empty();
        if !empty {
            self.clip.text(text);
        }
        if let Some(c) = session {
            notify::send(
                c,
                Notice {
                    summary,
                    body: if empty { missing } else { found },
                    icon: if empty { "dialog-error" } else { icon },
                    urgency: if empty { 0 } else { 1 },
                    ..Default::default()
                },
            )
            .await?;
        }
        Ok(PathBuf::new())
    }

    async fn color(&mut self, session: Option<&Connection>, [r, g, b]: [u8; 3]) -> Result<PathBuf> {
        let values = color_strings(r, g, b);
        self.clip.text(&values[0]);
        let swatch = RgbaImage::from_pixel(64, 64, image::Rgba([r, g, b, 255]));
        tokio::task::spawn_blocking(move || write_png(&swatch, Path::new(SWATCH), true)).await??;
        if let Some(c) = session {
            let id = notify::send(
                c,
                Notice {
                    summary: "Color Picked",
                    body: &format!("{} copied to clipboard", values[0]),
                    icon: SWATCH,
                    actions: &["hex", "Copy HEX", "rgb", "Copy RGB", "hsv", "Copy HSV"],
                    ..Default::default()
                },
            )
            .await?;
            self.swatches.insert(id, values);
            // Only the latest few can still be on screen.
            if self.swatches.len() > 8 {
                let oldest = *self.swatches.keys().min().unwrap();
                self.swatches.remove(&oldest);
            }
        }
        Ok(PathBuf::new())
    }

    fn publish(&self) {
        match serde_json::to_value(&self.state) {
            Ok(v) => self.bus.publish(SERVICE, &v),
            Err(e) => tracing::error!("serialise failed: {e}"),
        }
    }
}

async fn next<S: futures_util::Stream + Unpin>(stream: &mut Option<S>) -> Option<S::Item> {
    match stream {
        Some(s) => s.next().await,
        None => std::future::pending().await,
    }
}

fn contains(f: &Frame, x: i32, y: i32) -> bool {
    x >= f.x && x < f.x + f.w && y >= f.y && y < f.y + f.h
}

fn scale(f: &Frame) -> f64 {
    f.image.width() as f64 / f.w.max(1) as f64
}

// Logical global rect to physical pixels on this output, clipped to it.
fn crop(f: &Frame, x: i32, y: i32, w: i32, h: i32) -> Option<RgbaImage> {
    let s = scale(f);
    let to_px = |v: i32, origin: i32| ((v - origin) as f64 * s).round().max(0.0) as u32;
    let (x0, y0) = (to_px(x, f.x), to_px(y, f.y));
    let (x1, y1) = (
        to_px(x + w, f.x).min(f.image.width()),
        to_px(y + h, f.y).min(f.image.height()),
    );
    (x1 > x0 && y1 > y0)
        .then(|| image::imageops::crop_imm(&f.image, x0, y0, x1 - x0, y1 - y0).to_image())
}

fn pixel(f: &Frame, x: i32, y: i32) -> Option<[u8; 3]> {
    let s = scale(f);
    let px = ((x - f.x) as f64 * s) as u32;
    let py = ((y - f.y) as f64 * s) as u32;
    let p = f.image.get_pixel_checked(px, py)?;
    Some([p[0], p[1], p[2]])
}

fn color_strings(r: u8, g: u8, b: u8) -> [String; 3] {
    let (rn, gn, bn) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = rn.max(gn).max(bn);
    let min = rn.min(gn).min(bn);
    let d = max - min;
    let hue = if d == 0.0 {
        0.0
    } else if max == rn {
        60.0 * ((gn - bn) / d).rem_euclid(6.0)
    } else if max == gn {
        60.0 * ((bn - rn) / d + 2.0)
    } else {
        60.0 * ((rn - gn) / d + 4.0)
    };
    let sat = if max == 0.0 { 0.0 } else { d / max };
    [
        format!("#{r:02X}{g:02X}{b:02X}"),
        format!("rgb({r}, {g}, {b})"),
        format!(
            "hsv({}, {}%, {}%)",
            hue.round(),
            (sat * 100.0).round(),
            (max * 100.0).round()
        ),
    ]
}

fn encode_png(img: &RgbaImage, fast: bool) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let (c, f) = if fast {
        (CompressionType::Fast, FilterType::NoFilter)
    } else {
        (CompressionType::Default, FilterType::Adaptive)
    };
    PngEncoder::new_with_quality(Cursor::new(&mut buf), c, f).write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(buf)
}

fn write_png(img: &RgbaImage, path: &Path, fast: bool) -> Result<()> {
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, encode_png(img, fast)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

async fn tool(bin: &str, args: &[&str], input: &[u8]) -> Result<String> {
    let mut child = tokio::process::Command::new(bin)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("{bin} not found"))?;
    let mut stdin = child.stdin.take().context("no stdin")?;
    let input = input.to_vec();
    tokio::spawn(async move {
        let _ = stdin.write_all(&input).await;
    });
    let out = tokio::time::timeout(TOOL_TIMEOUT, child.wait_with_output())
        .await
        .context("timed out")??;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Deserialize)]
struct Uploaded {
    files: Vec<UploadedFile>,
}

#[derive(Deserialize)]
struct UploadedFile {
    url: String,
}

async fn upload(http: &reqwest::Client, png: Vec<u8>) -> Result<String> {
    let part = reqwest::multipart::Part::bytes(png)
        .file_name("image.png")
        .mime_str("image/png")?;
    let form = reqwest::multipart::Form::new().part("files[]", part);
    let r: Uploaded = http
        .post(LENS_UPLOAD)
        .multipart(form)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    r.files
        .into_iter()
        .next()
        .map(|f| f.url)
        .ok_or_else(|| anyhow!("upload returned no link"))
}

fn pictures() -> PathBuf {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let dirs = crate::config::dir()
        .parent()
        .and_then(|p| p.parent())
        .map(|c| c.join("user-dirs.dirs"));
    dirs.and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| user_dir(&s, "XDG_PICTURES_DIR", &home))
        .unwrap_or_else(|| home.join("Pictures"))
}

// user-dirs.dirs is shell syntax, but only ever "$HOME/x" or "/abs" in practice.
fn user_dir(file: &str, key: &str, home: &Path) -> Option<PathBuf> {
    let line = file.lines().find(|l| l.trim_start().starts_with(key))?;
    let value = line.split_once('=')?.1.trim().trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    path.is_absolute().then_some(path)
}

// Two captures inside one second must not overwrite each other.
fn unique(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.png"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}_{n}.png")))
        .find(|p| !p.exists())
        .unwrap()
}

fn timestamp() -> String {
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    // SAFETY: localtime_r fills the zeroed tm it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut tm) };
    format!(
        "{:04}-{:02}-{:02}-{:02}-{:02}-{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: i32, w: i32, phys: u32) -> Frame {
        Frame {
            name: "DP-1".into(),
            x,
            y: 0,
            w,
            h: w / 2,
            image: RgbaImage::from_fn(phys, phys / 2, |px, _| {
                image::Rgba([(px % 256) as u8, 0, 0, 255])
            }),
        }
    }

    #[test]
    fn crops_scale_logical_to_physical() {
        // 1280 logical on a 2560 physical buffer: scale 2.
        let f = frame(1920, 1280, 2560);
        let c = crop(&f, 1920 + 10, 5, 100, 50).unwrap();
        assert_eq!((c.width(), c.height()), (200, 100));
        assert_eq!(c.get_pixel(0, 0)[0], 20);
        // Spilling past the edge is clipped, not refused.
        let c = crop(&f, 1920 + 1270, 0, 100, 10).unwrap();
        assert_eq!(c.width(), 20);
        assert!(crop(&f, 1920, 0, 0, 10).is_none());
    }

    #[test]
    fn points_find_their_output() {
        let a = frame(0, 1920, 1920);
        let b = frame(1920, 1280, 2560);
        assert!(contains(&a, 1919, 0) && !contains(&a, 1920, 0));
        assert!(contains(&b, 1920, 0));
        assert_eq!(pixel(&b, 1920 + 3, 1).unwrap(), [6, 0, 0]);
    }

    #[test]
    fn color_formats() {
        assert_eq!(
            color_strings(255, 0, 0),
            ["#FF0000", "rgb(255, 0, 0)", "hsv(0, 100%, 100%)"]
        );
        assert_eq!(color_strings(0, 128, 128)[2], "hsv(180, 100%, 50%)");
        assert_eq!(color_strings(0, 0, 0)[2], "hsv(0, 0%, 0%)");
    }

    #[test]
    fn pictures_dir_from_user_dirs() {
        let home = Path::new("/home/o");
        let file =
            "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_PICTURES_DIR=\"$HOME/Images\"\n";
        assert_eq!(
            user_dir(file, "XDG_PICTURES_DIR", home).unwrap(),
            Path::new("/home/o/Images")
        );
        assert_eq!(
            user_dir("XDG_PICTURES_DIR=\"/data/pics\"", "XDG_PICTURES_DIR", home).unwrap(),
            Path::new("/data/pics")
        );
        assert!(user_dir("", "XDG_PICTURES_DIR", home).is_none());
    }

    #[test]
    fn commands_parse() {
        let r = Request {
            cmd: "act".into(),
            args: serde_json::json!({"mode": "save", "x": 1, "y": 2, "w": 3, "h": 4}),
        };
        assert!(matches!(
            Command::try_from(r).unwrap(),
            Command::Act { w: 3, .. }
        ));
    }
}
