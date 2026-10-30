use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncReadExt, AsyncWriteExt, Interest};
use tokio::net::unix::pipe;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1 as device, ext_data_control_manager_v1 as manager,
    ext_data_control_offer_v1 as offer, ext_data_control_source_v1 as source,
};

const RECONNECT: Duration = Duration::from_secs(2);
// A source that never closes its end would otherwise hold the read open forever.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_SIZE: u64 = 128 * 1024 * 1024;

const URI: &str = "text/uri-list";
// Set by password managers on what they copy; it is never kept.
const SECRET: &str = "x-kde-passwordManagerHint";
const TEXT: [&str; 5] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "TEXT",
    "STRING",
];

/// What was copied, in the one type worth keeping.
#[derive(Debug)]
pub struct Selection {
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
enum Request {
    Set(Vec<String>, Arc<[u8]>),
    Clear,
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::UnboundedSender<Request>,
}

impl Handle {
    pub fn text(&self, text: &str) {
        let mimes = TEXT.map(String::from).to_vec();
        let _ = self.tx.send(Request::Set(mimes, text.as_bytes().into()));
    }

    pub fn bytes(&self, mime: &str, data: &[u8]) {
        let _ = self.tx.send(Request::Set(vec![mime.into()], data.into()));
    }

    pub fn clear(&self) {
        let _ = self.tx.send(Request::Clear);
    }
}

pub fn spawn(selections: mpsc::Sender<Selection>) -> Handle {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(rx, selections));
    Handle { tx }
}

async fn run(mut rx: mpsc::UnboundedReceiver<Request>, selections: mpsc::Sender<Selection>) {
    loop {
        match session(&mut rx, &selections).await {
            Ok(()) => return,
            Err(e) => tracing::warn!("data control: {e:#}"),
        }
        tokio::time::sleep(RECONNECT).await;
    }
}

/// The offer outlives its read only until this drops, abort included.
struct Held {
    conn: Connection,
    offer: offer::ExtDataControlOfferV1,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.offer.destroy();
        let _ = self.conn.flush();
    }
}

struct State {
    offers: HashMap<ObjectId, Vec<String>>,
    // The newest selection event since the last look: Some(None) means emptied.
    changed: Option<Option<(offer::ExtDataControlOfferV1, Vec<String>)>>,
    finished: bool,
}

async fn session(
    rx: &mut mpsc::UnboundedReceiver<Request>,
    selections: &mpsc::Sender<Selection>,
) -> Result<()> {
    let conn = Connection::connect_to_env().context("connect")?;
    let (globals, mut queue): (_, EventQueue<State>) = registry_queue_init(&conn)?;
    let qh = queue.handle();

    let manager: Option<manager::ExtDataControlManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    let seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=1, ()).ok();
    let device = match (&manager, &seat) {
        (Some(m), Some(s)) => Some(m.get_data_device(s, &qh, ())),
        _ => {
            tracing::warn!("no ext-data-control-v1, clipboard history is off");
            None
        }
    };

    let mut state = State {
        offers: HashMap::new(),
        changed: None,
        finished: false,
    };
    let mut reading: Option<JoinHandle<()>> = None;

    // SAFETY: the fd is the connection's, which outlives this AsyncFd: both
    // are dropped at the end of this function, the AsyncFd first.
    let fd =
        unsafe { AsyncFd::register_with_interest(conn.as_fd().as_raw_fd(), Interest::READABLE) }
            .map_err(|e| anyhow!("register: {e}"))?;

    loop {
        queue.dispatch_pending(&mut state)?;
        if state.finished {
            bail!("device finished");
        }
        if let Some(changed) = state.changed.take() {
            // Whatever was being read is no longer what's on the clipboard.
            if let Some(old) = reading.take() {
                old.abort();
            }
            if let Some((offer, mimes)) = changed {
                let held = Held {
                    conn: conn.clone(),
                    offer,
                };
                reading = Some(tokio::spawn(receive(held, mimes, selections.clone())));
            }
        }
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
                let (Some(manager), Some(device)) = (&manager, &device) else {
                    continue;
                };
                match req {
                    Request::Set(mimes, data) => {
                        let source = manager.create_data_source(&qh, data);
                        for mime in mimes {
                            source.offer(mime);
                        }
                        device.set_selection(Some(&source));
                    }
                    Request::Clear => device.set_selection(None),
                }
            }
        }
    }
}

async fn receive(held: Held, mimes: Vec<String>, out: mpsc::Sender<Selection>) {
    let Some(mime) = pick(&mimes) else { return };
    match read(&held, mime).await {
        Ok(bytes) => {
            let selection = Selection {
                mime: mime.to_string(),
                bytes,
            };
            let _ = out.send(selection).await;
        }
        Err(e) => tracing::warn!(mime, "clipboard read: {e:#}"),
    }
}

async fn read(held: &Held, mime: &str) -> Result<Vec<u8>> {
    let (tx, rx) = pipe::pipe()?;
    held.offer.receive(mime.to_string(), tx.as_fd());
    held.conn.flush()?;
    // Ours has to close too, or the read never sees the end.
    drop(tx);

    let mut bytes = Vec::new();
    let n = timeout(READ_TIMEOUT, rx.take(MAX_SIZE + 1).read_to_end(&mut bytes))
        .await
        .context("timed out")??;
    if n as u64 > MAX_SIZE {
        bail!("over {MAX_SIZE} bytes");
    }
    Ok(bytes)
}

async fn serve(fd: OwnedFd, data: Arc<[u8]>) -> Result<()> {
    let mut pipe = pipe::Sender::from_owned_fd(fd)?;
    timeout(WRITE_TIMEOUT, pipe.write_all(&data))
        .await
        .context("timed out")??;
    Ok(())
}

/// The one type to keep from what a source offers: files, then images, then
/// text. None for anything a password manager marked as secret.
pub fn pick(mimes: &[String]) -> Option<&str> {
    let has = |m: &str| mimes.iter().find(|o| *o == m).map(String::as_str);
    if has(SECRET).is_some() {
        return None;
    }
    has(URI)
        .or_else(|| {
            mimes
                .iter()
                .find(|m| m.starts_with("image/"))
                .map(String::as_str)
        })
        .or_else(|| TEXT.iter().find_map(|m| has(m)))
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<manager::ExtDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &manager::ExtDataControlManagerV1,
        _: manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<device::ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &device::ExtDataControlDeviceV1,
        event: device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            device::Event::DataOffer { id } => {
                state.offers.insert(id.id(), Vec::new());
            }
            device::Event::Selection { id } => {
                let next = id.map(|o| {
                    let mimes = state.offers.remove(&o.id()).unwrap_or_default();
                    (o, mimes)
                });
                // Two in one dispatch: the first was never going to be read.
                if let Some(Some((old, _))) = state.changed.replace(next) {
                    old.destroy();
                }
            }
            device::Event::PrimarySelection { id: Some(o) } => {
                state.offers.remove(&o.id());
                o.destroy();
            }
            device::Event::Finished => state.finished = true,
            _ => {}
        }
    }

    event_created_child!(State, device::ExtDataControlDeviceV1, [
        device::EVT_DATA_OFFER_OPCODE => (offer::ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<offer::ExtDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &offer::ExtDataControlOfferV1,
        event: offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let offer::Event::Offer { mime_type } = event
            && let Some(mimes) = state.offers.get_mut(&proxy.id())
        {
            mimes.push(mime_type);
        }
    }
}

impl Dispatch<source::ExtDataControlSourceV1, Arc<[u8]>> for State {
    fn event(
        _: &mut Self,
        proxy: &source::ExtDataControlSourceV1,
        event: source::Event,
        data: &Arc<[u8]>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            source::Event::Send { fd, .. } => {
                let data = data.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve(fd, data).await {
                        tracing::debug!("clipboard paste: {e:#}");
                    }
                });
            }
            source::Event::Cancelled => proxy.destroy(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mimes(list: &[&str]) -> Vec<String> {
        list.iter().map(|m| m.to_string()).collect()
    }

    #[test]
    fn files_win_over_images_and_images_over_text() {
        let all = mimes(&["text/plain", "image/png", "text/uri-list"]);
        assert_eq!(pick(&all), Some("text/uri-list"));
        assert_eq!(
            pick(&mimes(&["text/plain", "image/png"])),
            Some("image/png")
        );
    }

    #[test]
    fn prefers_the_utf8_text() {
        let all = mimes(&["STRING", "text/plain", "text/plain;charset=utf-8"]);
        assert_eq!(pick(&all), Some("text/plain;charset=utf-8"));
        assert_eq!(pick(&mimes(&["UTF8_STRING"])), Some("UTF8_STRING"));
    }

    #[test]
    fn nothing_from_a_password_manager() {
        let all = mimes(&["text/plain", "x-kde-passwordManagerHint"]);
        assert_eq!(pick(&all), None);
    }

    #[test]
    fn nothing_worth_keeping() {
        assert_eq!(pick(&mimes(&["application/x-foo"])), None);
        assert_eq!(pick(&[]), None);
    }
}
