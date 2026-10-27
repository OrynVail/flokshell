use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use image::RgbaImage;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::{broadcast, mpsc, oneshot};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_seat, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notifier_v1,
};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1, zxdg_output_v1};
use wayland_protocols_wlr::gamma_control::v1::client::{
    zwlr_gamma_control_manager_v1, zwlr_gamma_control_v1,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_manager_v1,
};

const RECONNECT: Duration = Duration::from_secs(2);

/// What a service asks of the compositor. Each request replaces the last of
/// its kind and is replayed after a reconnect, so services never re-send.
#[derive(Debug)]
pub enum Request {
    /// Colour temperature for every output, or None to hand gamma back.
    Gamma(Option<u32>),
    /// One notification per timeout, in milliseconds.
    Idle(Vec<u32>),
    /// Every output, once. Not replayed: a capture is for the moment it was asked.
    Capture(oneshot::Sender<Result<Vec<Frame>>>),
}

/// One output's pixels, upright, with where it sits in the compositor's
/// logical space. `image` is physical pixels, so scale is its width over `w`.
pub struct Frame {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub image: RgbaImage,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Frame({} {}x{}+{}+{})",
            self.name, self.w, self.h, self.x, self.y
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    Connected(bool),
    /// Outputs holding our gamma, and outputs where another client already does.
    Gamma {
        applied: usize,
        refused: usize,
    },
    Idle {
        timeout: u32,
        idle: bool,
    },
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::UnboundedSender<Request>,
}

impl Handle {
    pub fn send(&self, req: Request) {
        let _ = self.tx.send(req);
    }
}

pub fn spawn(notes: broadcast::Sender<Note>) -> Handle {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(rx, notes));
    Handle { tx }
}

#[derive(Default)]
struct Wanted {
    gamma: Option<u32>,
    idle: Vec<u32>,
}

async fn run(mut rx: mpsc::UnboundedReceiver<Request>, notes: broadcast::Sender<Note>) {
    let mut wanted = Wanted::default();
    loop {
        match session(&mut rx, &notes, &mut wanted).await {
            Ok(()) => return,
            Err(e) => tracing::warn!("wayland: {e:#}"),
        }
        let _ = notes.send(Note::Connected(false));
        // Requests made while disconnected still count; keep the newest.
        let deadline = tokio::time::sleep(RECONNECT);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                req = rx.recv() => match req {
                    Some(r) => wanted.take(r),
                    None => return,
                },
            }
        }
    }
}

impl Wanted {
    fn take(&mut self, req: Request) {
        match req {
            Request::Gamma(t) => self.gamma = t,
            Request::Idle(t) => self.idle = t,
            Request::Capture(reply) => {
                let _ = reply.send(Err(anyhow!("no compositor connection")));
            }
        }
    }
}

struct Output {
    proxy: wl_output::WlOutput,
    name: String,
    gamma: Option<zwlr_gamma_control_v1::ZwlrGammaControlV1>,
    // None until the compositor says how big the ramp is; 0 once refused.
    gamma_size: Option<u32>,
    transform: wl_output::Transform,
    // Logical position and size, from xdg-output.
    logical: (i32, i32, i32, i32),
    xdg: Option<zxdg_output_v1::ZxdgOutputV1>,
}

struct Pending {
    frame: zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    format: Option<(wl_shm::Format, u32, u32, u32)>,
    shm: Option<(File, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer)>,
    y_invert: bool,
}

struct Capture {
    reply: oneshot::Sender<Result<Vec<Frame>>>,
    pending: HashMap<u32, Pending>,
    frames: Vec<Frame>,
}

struct State {
    notes: broadcast::Sender<Note>,
    outputs: HashMap<u32, Output>,
    seat: Option<wl_seat::WlSeat>,
    gamma_manager: Option<zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1>,
    idle_notifier: Option<ext_idle_notifier_v1::ExtIdleNotifierV1>,
    idle: Vec<ext_idle_notification_v1::ExtIdleNotificationV1>,
    temperature: Option<u32>,
    shm: Option<wl_shm::WlShm>,
    xdg_manager: Option<zxdg_output_manager_v1::ZxdgOutputManagerV1>,
    screencopy: Option<zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1>,
    capture: Option<Capture>,
}

async fn session(
    rx: &mut mpsc::UnboundedReceiver<Request>,
    notes: &broadcast::Sender<Note>,
    wanted: &mut Wanted,
) -> Result<()> {
    let conn = Connection::connect_to_env().context("connect")?;
    let (globals, mut queue): (_, EventQueue<State>) = registry_queue_init(&conn)?;
    let qh = queue.handle();

    let mut state = State {
        notes: notes.clone(),
        outputs: HashMap::new(),
        seat: None,
        gamma_manager: globals.bind(&qh, 1..=1, ()).ok(),
        idle_notifier: globals.bind(&qh, 1..=1, ()).ok(),
        idle: Vec::new(),
        temperature: None,
        shm: globals.bind(&qh, 1..=1, ()).ok(),
        xdg_manager: globals.bind(&qh, 2..=3, ()).ok(),
        screencopy: globals.bind(&qh, 1..=3, ()).ok(),
        capture: None,
    };
    globals.contents().with_list(|list| {
        for g in list {
            state.global(globals.registry(), &qh, g.name, &g.interface, g.version);
        }
    });
    queue.roundtrip(&mut state)?;
    tracing::info!(
        outputs = state.outputs.len(),
        gamma = state.gamma_manager.is_some(),
        idle = state.idle_notifier.is_some(),
        screencopy = state.screencopy.is_some(),
        "wayland connected"
    );
    let _ = notes.send(Note::Connected(true));

    state.set_gamma(&qh, wanted.gamma);
    state.set_idle(&qh, &wanted.idle);

    // SAFETY: the fd is the connection's, which outlives this AsyncFd: both
    // are dropped at the end of this function, the AsyncFd first.
    let fd =
        unsafe { AsyncFd::register_with_interest(conn.as_fd().as_raw_fd(), Interest::READABLE) }
            .map_err(|e| anyhow!("register: {e}"))?;

    loop {
        queue.dispatch_pending(&mut state)?;
        queue.flush()?;
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        tokio::select! {
            ready = fd.readable() => {
                let mut ready = ready?;
                match guard.read() {
                    Ok(_) => {}
                    Err(wayland_client::backend::WaylandError::Io(e))
                        if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.into()),
                }
                ready.clear_ready();
            }
            req = rx.recv() => {
                drop(guard);
                let Some(req) = req else { return Ok(()) };
                match req {
                    Request::Gamma(t) => {
                        wanted.gamma = t;
                        state.set_gamma(&qh, t);
                    }
                    Request::Idle(t) => {
                        state.set_idle(&qh, &t);
                        wanted.idle = t;
                    }
                    Request::Capture(reply) => state.capture(&qh, reply),
                }
            }
        }
    }
}

impl State {
    fn global(
        &mut self,
        registry: &wl_registry::WlRegistry,
        qh: &QueueHandle<Self>,
        name: u32,
        interface: &str,
        version: u32,
    ) {
        if interface == wl_output::WlOutput::interface().name {
            let proxy: wl_output::WlOutput = registry.bind(name, version.min(4), qh, name);
            let xdg = self
                .xdg_manager
                .as_ref()
                .map(|m| m.get_xdg_output(&proxy, qh, name));
            let mut out = Output {
                proxy,
                name: String::new(),
                gamma: None,
                gamma_size: None,
                transform: wl_output::Transform::Normal,
                logical: (0, 0, 0, 0),
                xdg,
            };
            if let (Some(_), Some(m)) = (self.temperature, &self.gamma_manager) {
                out.gamma = Some(m.get_gamma_control(&out.proxy, qh, name));
            }
            self.outputs.insert(name, out);
        } else if interface == wl_seat::WlSeat::interface().name && self.seat.is_none() {
            self.seat = Some(registry.bind(name, version.min(1), qh, ()));
        }
    }

    fn set_gamma(&mut self, qh: &QueueHandle<Self>, temperature: Option<u32>) {
        self.temperature = temperature;
        let Some(manager) = &self.gamma_manager else {
            let _ = self.notes.send(Note::Gamma {
                applied: 0,
                refused: self.outputs.len(),
            });
            return;
        };
        for (name, out) in &mut self.outputs {
            match temperature {
                // Destroying the control is what restores the original ramp.
                None => {
                    if let Some(g) = out.gamma.take() {
                        g.destroy();
                    }
                    out.gamma_size = None;
                }
                Some(_) => match (&out.gamma, out.gamma_size) {
                    (None, _) => out.gamma = Some(manager.get_gamma_control(&out.proxy, qh, *name)),
                    (Some(g), Some(size)) if size > 0 => {
                        if let Err(e) = apply_ramp(g, size, temperature.unwrap()) {
                            tracing::warn!(output = out.name, "gamma: {e:#}");
                        }
                    }
                    _ => {}
                },
            }
        }
        self.report_gamma();
    }

    fn report_gamma(&self) {
        if self.temperature.is_none() {
            let _ = self.notes.send(Note::Gamma {
                applied: 0,
                refused: 0,
            });
            return;
        }
        let applied = self
            .outputs
            .values()
            .filter(|o| o.gamma_size.is_some_and(|s| s > 0))
            .count();
        let refused = self
            .outputs
            .values()
            .filter(|o| o.gamma_size == Some(0))
            .count();
        let _ = self.notes.send(Note::Gamma { applied, refused });
    }

    fn capture(&mut self, qh: &QueueHandle<Self>, reply: oneshot::Sender<Result<Vec<Frame>>>) {
        if self.capture.is_some() {
            let _ = reply.send(Err(anyhow!("a capture is already running")));
            return;
        }
        let (Some(manager), Some(_)) = (&self.screencopy, &self.shm) else {
            let _ = reply.send(Err(anyhow!("compositor offers no screencopy")));
            return;
        };
        let pending: HashMap<u32, Pending> = self
            .outputs
            .iter()
            .map(|(global, out)| {
                (
                    *global,
                    Pending {
                        frame: manager.capture_output(0, &out.proxy, qh, *global),
                        format: None,
                        shm: None,
                        y_invert: false,
                    },
                )
            })
            .collect();
        if pending.is_empty() {
            let _ = reply.send(Ok(Vec::new()));
            return;
        }
        self.capture = Some(Capture {
            reply,
            pending,
            frames: Vec::new(),
        });
    }

    // Allocate the buffer the compositor described and ask it to fill it.
    fn copy(&mut self, qh: &QueueHandle<Self>, global: u32) {
        let Some(shm) = &self.shm else { return };
        let Some(p) = self
            .capture
            .as_mut()
            .and_then(|c| c.pending.get_mut(&global))
        else {
            return;
        };
        let Some((format, w, h, stride)) = p.format else {
            return;
        };
        let result = (|| -> Result<()> {
            let size = (stride * h) as i32;
            let file = memfd(c"flokd-capture")?;
            file.set_len(size as u64)?;
            let pool = shm.create_pool(file.as_fd(), size, qh, ());
            let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, qh, ());
            p.frame.copy(&buffer);
            p.shm = Some((file, pool, buffer));
            Ok(())
        })();
        if let Err(e) = result {
            self.finish_frame(global, Err(e));
        }
    }

    fn finish_frame(&mut self, global: u32, result: Result<Frame>) {
        let Some(capture) = self.capture.as_mut() else {
            return;
        };
        if let Some(p) = capture.pending.remove(&global) {
            p.frame.destroy();
            if let Some((_, pool, buffer)) = p.shm {
                buffer.destroy();
                pool.destroy();
            }
        }
        match result {
            Ok(f) => capture.frames.push(f),
            Err(e) => tracing::warn!("capture: {e:#}"),
        }
        if capture.pending.is_empty() {
            let c = self.capture.take().unwrap();
            let mut frames = c.frames;
            frames.sort_by_key(|f| (f.x, f.y));
            let _ = c.reply.send(if frames.is_empty() {
                Err(anyhow!("no output could be captured"))
            } else {
                Ok(frames)
            });
        }
    }

    fn read_frame(&self, global: u32) -> Result<Frame> {
        let p = self
            .capture
            .as_ref()
            .and_then(|c| c.pending.get(&global))
            .context("unknown frame")?;
        let (format, w, h, stride) = p.format.context("no buffer format")?;
        let (file, _, _) = p.shm.as_ref().context("no buffer")?;
        let mut raw = vec![0u8; (stride * h) as usize];
        file.read_exact_at(&mut raw, 0)?;
        let image = to_rgba(&raw, format, w, h, stride, p.y_invert)?;
        let out = self.outputs.get(&global).context("output gone")?;
        let image = upright(image, out.transform, out.logical);
        let (x, y, lw, lh) = out.logical;
        let (lw, lh) = if lw > 0 {
            (lw, lh)
        } else {
            (image.width() as i32, image.height() as i32)
        };
        Ok(Frame {
            name: out.name.clone(),
            x,
            y,
            w: lw,
            h: lh,
            image,
        })
    }

    fn set_idle(&mut self, qh: &QueueHandle<Self>, timeouts: &[u32]) {
        for n in self.idle.drain(..) {
            n.destroy();
        }
        let (Some(notifier), Some(seat)) = (&self.idle_notifier, &self.seat) else {
            if !timeouts.is_empty() {
                tracing::warn!("no idle notifier; idle listeners will never fire");
            }
            return;
        };
        for &t in timeouts {
            self.idle
                .push(notifier.get_idle_notification(t, seat, qh, t));
        }
    }
}

// Tanner Helland's fit of the Planckian locus, scaled so the brightest channel
// stays at 1: warming dims blue and green, never pushes red past full.
pub fn whitepoint(kelvin: u32) -> [f64; 3] {
    let t = kelvin.clamp(1000, 40000) as f64 / 100.0;
    let r = if t <= 66.0 {
        255.0
    } else {
        329.698727446 * (t - 60.0).powf(-0.1332047592)
    };
    let g = if t <= 66.0 {
        99.4708025861 * t.ln() - 161.1195681661
    } else {
        288.1221695283 * (t - 60.0).powf(-0.0755148492)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.5177312231 * (t - 10.0).ln() - 305.0447927307
    };
    let [r, g, b] = [r, g, b].map(|c| c.clamp(0.0, 255.0) / 255.0);
    let max = r.max(g).max(b);
    [r / max, g / max, b / max]
}

// Three u16 ramps back to back, red then green then blue, as the protocol wants.
pub fn ramps(size: u32, kelvin: u32) -> Vec<u8> {
    let white = whitepoint(kelvin);
    let mut out = Vec::with_capacity(size as usize * 6);
    for channel in white {
        for i in 0..size {
            let x = if size > 1 {
                i as f64 / (size - 1) as f64
            } else {
                1.0
            };
            let v = (x * channel * 65535.0).round() as u16;
            out.extend_from_slice(&v.to_ne_bytes());
        }
    }
    out
}

fn memfd(name: &std::ffi::CStr) -> Result<File> {
    // SAFETY: memfd_create returns a fresh fd or -1; ownership passes to File.
    let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(File::from(unsafe { OwnedFd::from_raw_fd(raw) }))
}

// Little-endian 8-bit formats only: what wlroots-style compositors hand out
// for SDR outputs. A 10-bit output is refused rather than shown wrong.
fn to_rgba(
    raw: &[u8],
    format: wl_shm::Format,
    w: u32,
    h: u32,
    stride: u32,
    y_invert: bool,
) -> Result<RgbaImage> {
    let bgr = match format {
        wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888 => true,
        wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888 => false,
        f => return Err(anyhow!("unsupported buffer format {f:?}")),
    };
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let src = if y_invert { h - 1 - row } else { row };
        let line = raw
            .get((src * stride) as usize..(src * stride + w * 4) as usize)
            .context("short buffer")?;
        for px in line.as_chunks::<4>().0 {
            if bgr {
                out.extend_from_slice(&[px[2], px[1], px[0], 255]);
            } else {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
    }
    RgbaImage::from_raw(w, h, out).context("buffer size mismatch")
}

// A rotated output's buffer is in its native orientation. Turned only when its
// shape disagrees with the logical one, so a compositor that already rotates
// is left alone. Untested on a rotated monitor; revisit when one is at hand.
fn upright(
    img: RgbaImage,
    transform: wl_output::Transform,
    logical: (i32, i32, i32, i32),
) -> RgbaImage {
    use image::imageops::{flip_horizontal, rotate90, rotate180, rotate270};
    use wl_output::Transform as T;
    let (_, _, lw, lh) = logical;
    let quarter = matches!(transform, T::_90 | T::_270 | T::Flipped90 | T::Flipped270);
    let shape_differs = lw > 0 && ((img.width() > img.height()) != (lw > lh));
    if quarter && !shape_differs {
        return img;
    }
    match transform {
        T::_90 => rotate270(&img),
        T::_180 => rotate180(&img),
        T::_270 => rotate90(&img),
        T::Flipped => flip_horizontal(&img),
        T::Flipped90 => rotate270(&flip_horizontal(&img)),
        T::Flipped180 => rotate180(&flip_horizontal(&img)),
        T::Flipped270 => rotate90(&flip_horizontal(&img)),
        _ => img,
    }
}

fn apply_ramp(
    control: &zwlr_gamma_control_v1::ZwlrGammaControlV1,
    size: u32,
    kelvin: u32,
) -> Result<()> {
    let mut file = memfd(c"flokd-gamma")?;
    file.write_all(&ramps(size, kelvin))?;
    file.rewind()?;
    control.set_gamma(file.as_fd());
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                state.global(registry, qh, name, &interface, version);
                state.report_gamma();
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(out) = state.outputs.remove(&name) {
                    if let Some(g) = out.gamma {
                        g.destroy();
                    }
                    if let Some(x) = out.xdg {
                        x.destroy();
                    }
                    if state
                        .capture
                        .as_ref()
                        .is_some_and(|c| c.pending.contains_key(&name))
                    {
                        state.finish_frame(name, Err(anyhow!("output removed mid-capture")));
                    }
                    out.proxy.release();
                    state.report_gamma();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outputs.get_mut(global) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => out.name = name,
            wl_output::Event::Geometry {
                transform: WEnum::Value(t),
                ..
            } => out.transform = t,
            _ => {}
        }
    }
}

impl Dispatch<zwlr_gamma_control_v1::ZwlrGammaControlV1, u32> for State {
    fn event(
        state: &mut Self,
        control: &zwlr_gamma_control_v1::ZwlrGammaControlV1,
        event: zwlr_gamma_control_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outputs.get_mut(global) else {
            return;
        };
        match event {
            zwlr_gamma_control_v1::Event::GammaSize { size } => {
                out.gamma_size = Some(size);
                if let Some(t) = state.temperature
                    && let Err(e) = apply_ramp(control, size, t)
                {
                    tracing::warn!(output = out.name, "gamma: {e:#}");
                }
            }
            // Another client (hyprsunset, wlsunset) holds this output's gamma.
            zwlr_gamma_control_v1::Event::Failed => {
                tracing::warn!(output = out.name, "gamma refused; another client holds it");
                control.destroy();
                out.gamma = None;
                out.gamma_size = Some(0);
            }
            _ => {}
        }
        state.report_gamma();
    }
}

impl Dispatch<ext_idle_notification_v1::ExtIdleNotificationV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &ext_idle_notification_v1::ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        timeout: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let idle = match event {
            ext_idle_notification_v1::Event::Idled => true,
            ext_idle_notification_v1::Event::Resumed => false,
            _ => return,
        };
        let _ = state.notes.send(Note::Idle {
            timeout: *timeout,
            idle,
        });
    }
}

impl Dispatch<zxdg_output_v1::ZxdgOutputV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &zxdg_output_v1::ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(out) = state.outputs.get_mut(global) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => {
                out.logical.0 = x;
                out.logical.1 = y;
            }
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                out.logical.2 = width;
                out.logical.3 = height;
            }
            _ => {}
        }
    }
}

impl Dispatch<zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, u32> for State {
    fn event(
        state: &mut Self,
        frame: &zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        global: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event as E;
        let global = *global;
        let Some(p) = state
            .capture
            .as_mut()
            .and_then(|c| c.pending.get_mut(&global))
        else {
            return;
        };
        match event {
            E::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } => {
                if p.format.is_none() {
                    p.format = Some((format, width, height, stride));
                }
                // Before v3 there is no buffer_done; the first offer is the one.
                if frame.version() < 3 {
                    state.copy(qh, global);
                }
            }
            E::BufferDone => state.copy(qh, global),
            E::Flags {
                flags: WEnum::Value(f),
            } => {
                p.y_invert = f.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            E::Ready { .. } => {
                let result = state.read_frame(global);
                state.finish_frame(global, result);
            }
            E::Failed => state.finish_frame(global, Err(anyhow!("compositor refused the copy"))),
            _ => {}
        }
    }
}

macro_rules! ignore_events {
    ($($t:ty),* $(,)?) => {$(
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}

ignore_events!(
    wl_seat::WlSeat,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1,
    ext_idle_notifier_v1::ExtIdleNotifierV1,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daylight_is_neutral_and_warm_dims_blue() {
        let d = whitepoint(6600);
        assert!(d.iter().all(|c| (c - 1.0).abs() < 0.02), "{d:?}");
        let w = whitepoint(4500);
        assert_eq!(w[0], 1.0);
        assert!(w[1] < 1.0 && w[2] < w[1], "{w:?}");
    }

    #[test]
    fn ramps_are_three_channels_of_rising_u16() {
        let r = ramps(256, 4500);
        assert_eq!(r.len(), 256 * 2 * 3);
        let red: Vec<u16> = r[..512]
            .chunks(2)
            .map(|c| u16::from_ne_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(red[0], 0);
        assert_eq!(red[255], 65535);
        assert!(red.windows(2).all(|w| w[0] <= w[1]));
        let blue_top = u16::from_ne_bytes([r[r.len() - 2], r[r.len() - 1]]);
        assert!(blue_top < 65535);
    }
}
