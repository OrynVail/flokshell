use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use zbus::message::Type as MessageType;
use zbus::{Connection, MatchRule, MessageStream};

use crate::bus::{Bus, Request};
use crate::dbus::{self, sleep_until};
use crate::ddc;

const SERVICE: &str = "brightness";

// Outputs change in bursts on hotplug and wake; displays answer DDC late after either.
const RESCAN_DEBOUNCE: Duration = Duration::from_secs(1);
const WAKE_DELAY: Duration = Duration::from_secs(2);

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct State {
    monitors: Vec<Monitor>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct Monitor {
    name: String,
    kind: String,
    value: u32,
    max: u32,
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    // Written through logind: the sysfs file is root's.
    Backlight { device: String },
    Ddc { bus: u32 },
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", content = "args", rename_all = "lowercase")]
enum Command {
    Set { name: String, value: u32 },
    Rescan,
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
    state: State,
    targets: HashMap<String, Target>,
    writers: HashMap<String, watch::Sender<u16>>,
    watchers: Vec<JoinHandle<()>>,
}

impl Service {
    pub fn new(bus: Bus) -> Self {
        Self {
            bus,
            state: State::default(),
            targets: HashMap::new(),
            writers: HashMap::new(),
            watchers: Vec::new(),
        }
    }

    pub async fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        let conn = match Connection::system().await {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!("no system bus, backlight is read-only: {e}");
                None
            }
        };
        let mut sleeps = match &conn {
            Some(c) => sleep_signals(c).await.ok(),
            None => None,
        };

        let (changed_tx, mut changed) = mpsc::channel(16);
        let mut rescan_at = Some(Instant::now());

        loop {
            tokio::select! {
                _ = sleep_until(rescan_at) => {
                    rescan_at = None;
                    self.rescan(&changed_tx).await;
                }
                Some(req) = requests.recv() => match Command::try_from(req) {
                    Ok(Command::Set { name, value }) => self.set(conn.as_ref(), &name, value).await,
                    Ok(Command::Rescan) => rescan_at = Some(Instant::now() + RESCAN_DEBOUNCE),
                    Err(e) => tracing::warn!("bad command: {e}"),
                },
                Some(msg) = next(&mut sleeps) => {
                    if let Ok(false) = msg.body().deserialize::<bool>() {
                        rescan_at = Some(Instant::now() + WAKE_DELAY);
                    }
                }
                Some((name, value)) = changed.recv() => {
                    if let Some(m) = self.state.monitors.iter_mut().find(|m| m.name == name)
                        && m.value != value
                    {
                        m.value = value;
                        self.publish();
                    }
                }
            }
        }
    }

    async fn rescan(&mut self, changed: &mpsc::Sender<(String, u32)>) {
        let found = match tokio::task::spawn_blocking(discover).await {
            Ok(found) => found,
            Err(e) => {
                tracing::error!("discovery panicked: {e}");
                return;
            }
        };

        for w in self.watchers.drain(..) {
            w.abort();
        }
        self.writers.clear();
        self.targets.clear();

        let mut monitors = Vec::new();
        for (monitor, target) in found {
            match &target {
                Target::Backlight { device } => {
                    let (name, device, tx) =
                        (monitor.name.clone(), device.clone(), changed.clone());
                    self.watchers.push(tokio::spawn(async move {
                        if let Err(e) = watch_backlight(&name, &device, tx).await {
                            tracing::warn!(device, "backlight changes will go unseen: {e:#}");
                        }
                    }));
                }
                Target::Ddc { bus } => {
                    let (tx, rx) = watch::channel(monitor.value as u16);
                    self.watchers.push(tokio::spawn(write_ddc(*bus, rx)));
                    self.writers.insert(monitor.name.clone(), tx);
                }
            }
            tracing::info!(name = %monitor.name, ?target, value = monitor.value, max = monitor.max, "brightness control");
            self.targets.insert(monitor.name.clone(), target);
            monitors.push(monitor);
        }

        self.state.monitors = monitors;
        self.publish();
    }

    async fn set(&mut self, conn: Option<&Connection>, name: &str, value: u32) {
        let Some(m) = self.state.monitors.iter_mut().find(|m| m.name == name) else {
            tracing::debug!(name, "no such monitor");
            return;
        };
        let value = value.min(m.max);
        m.value = value;

        match self.targets.get(name) {
            Some(Target::Backlight { device }) => {
                let Some(conn) = conn else { return };
                if let Err(e) = dbus::call(
                    conn,
                    "org.freedesktop.login1",
                    "/org/freedesktop/login1/session/auto",
                    "org.freedesktop.login1.Session",
                    "SetBrightness",
                    &("backlight", device.as_str(), value),
                )
                .await
                {
                    tracing::warn!("{e:#}");
                }
            }
            Some(Target::Ddc { .. }) => {
                if let Some(tx) = self.writers.get(name) {
                    tx.send_replace(value as u16);
                }
            }
            None => {}
        }
        // The value asked for, not the one reached: a DDC display never says.
        self.publish();
    }

    fn publish(&self) {
        match serde_json::to_value(&self.state) {
            Ok(v) => self.bus.publish(SERVICE, &v),
            Err(e) => tracing::error!("serialise failed: {e}"),
        }
    }
}

async fn next(stream: &mut Option<MessageStream>) -> Option<zbus::Message> {
    match stream {
        Some(s) => s.next().await?.ok(),
        None => std::future::pending().await,
    }
}

async fn sleep_signals(conn: &Connection) -> Result<MessageStream> {
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface("org.freedesktop.login1.Manager")
        .member("PrepareForSleep")
        .build()?;
    Ok(MessageStream::for_match_rule(rule, conn, None).await?)
}

// Latest-wins: a drag outpaces any display, which skips to where the slider is.
async fn write_ddc(bus: u32, mut rx: watch::Receiver<u16>) {
    while rx.changed().await.is_ok() {
        let value = *rx.borrow_and_update();
        match tokio::task::spawn_blocking(move || ddc::set_vcp(bus, ddc::BRIGHTNESS, value)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(bus, "ddc write failed: {e:#}"),
            Err(e) => tracing::error!("ddc write panicked: {e}"),
        }
        tokio::time::sleep(ddc::SETTLE).await;
    }
}

// actual_brightness is notified on every write, hotkeys included, and a read
// re-arms it. Its value reads 0 on some panels; the level comes from brightness.
async fn watch_backlight(name: &str, device: &str, tx: mpsc::Sender<(String, u32)>) -> Result<()> {
    let base = Path::new("/sys/class/backlight").join(device);
    let file = File::open(base.join("actual_brightness"))?;
    // SAFETY: a File owns its fd and keeps it for as long as the AsyncFd owns the File.
    let fd = unsafe { AsyncFd::register_with_interest(file, Interest::PRIORITY) }
        .map_err(|e| anyhow!("register: {e}"))?;
    rearm(fd.get_ref())?;
    loop {
        let mut ready = fd.ready(Interest::PRIORITY).await?;
        rearm(fd.get_ref())?;
        ready.clear_ready();
        let Some(value) = read_file_u32(&base.join("brightness")) else {
            continue;
        };
        if tx.send((name.to_string(), value)).await.is_err() {
            return Ok(());
        }
    }
}

fn rearm(f: &File) -> Result<()> {
    let mut buf = [0u8; 32];
    f.read_at(&mut buf, 0)?;
    Ok(())
}

fn parse_u32(s: &str) -> Option<u32> {
    s.trim().parse().ok()
}

fn read_file_u32(p: &Path) -> Option<u32> {
    parse_u32(&std::fs::read_to_string(p).ok()?)
}

fn output_name(connector: &str) -> Option<&str> {
    let (card, name) = connector.split_once('-')?;
    let index = card.strip_prefix("card")?;
    (!index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()) && !name.is_empty())
        .then_some(name)
}

fn is_internal(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["edp", "lvds", "dsi"].iter().any(|p| n.starts_with(p))
}

// What the kernel documents for userspace: firmware, then platform, then raw.
fn type_rank(kind: &str) -> u8 {
    match kind {
        "firmware" => 0,
        "platform" => 1,
        _ => 2,
    }
}

fn discover() -> Vec<(Monitor, Target)> {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    let mut connectors: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|c| output_name(c).is_some())
        .filter(|c| {
            std::fs::read_to_string(format!("/sys/class/drm/{c}/status"))
                .is_ok_and(|s| s.trim() == "connected")
        })
        .collect();
    connectors.sort();

    let mut used = HashSet::new();
    let mut found = Vec::new();
    for connector in connectors {
        let name = output_name(&connector).unwrap_or_default().to_string();
        let result = if is_internal(&name) {
            backlight(&connector, &name)
        } else {
            external(&connector, &name, &mut used)
        };
        match result {
            Ok(f) => found.push(f),
            Err(e) => tracing::info!(name, "no brightness control: {e:#}"),
        }
    }
    found
}

fn backlight(connector: &str, name: &str) -> Result<(Monitor, Target)> {
    let conn_path = std::fs::canonicalize(format!("/sys/class/drm/{connector}")).ok();
    let mut candidates: Vec<(bool, u8, String)> = std::fs::read_dir("/sys/class/backlight")
        .context("no backlight class")?
        .filter_map(|e| {
            let e = e.ok()?;
            let device = e.file_name().into_string().ok()?;
            let linked = conn_path.is_some()
                && std::fs::canonicalize(e.path().join("device")).ok() == conn_path;
            let kind = std::fs::read_to_string(e.path().join("type")).unwrap_or_default();
            Some((!linked, type_rank(kind.trim()), device))
        })
        .collect();
    candidates.sort();
    let (_, _, device) = candidates
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no backlight device"))?;

    let base = Path::new("/sys/class/backlight").join(&device);
    let max = read_file_u32(&base.join("max_brightness"))
        .filter(|&m| m > 0)
        .context("no max_brightness")?;
    let value = read_file_u32(&base.join("brightness")).context("no brightness")?;
    Ok((
        Monitor {
            name: name.into(),
            kind: "backlight".into(),
            value,
            max,
        },
        Target::Backlight { device },
    ))
}

fn external(connector: &str, name: &str, used: &mut HashSet<u32>) -> Result<(Monitor, Target)> {
    let bus = linked_bus(connector)
        .or_else(|| edid_bus(connector, used))
        .ok_or_else(|| anyhow!("no i2c bus answers with this display's EDID"))?;
    let (value, max) = ddc::get_vcp(bus, ddc::BRIGHTNESS).with_context(|| format!("i2c-{bus}"))?;
    if max == 0 {
        return Err(anyhow!("display reports a brightness range of zero"));
    }
    used.insert(bus);
    Ok((
        Monitor {
            name: name.into(),
            kind: "ddc".into(),
            value: value.into(),
            max: max.into(),
        },
        Target::Ddc { bus },
    ))
}

fn bus_number(p: &Path) -> Option<u32> {
    p.file_name()?.to_str()?.strip_prefix("i2c-")?.parse().ok()
}

// i915 and amdgpu link the bus: ddc for HDMI, an AUX child for DP.
fn linked_bus(connector: &str) -> Option<u32> {
    let dir = PathBuf::from(format!("/sys/class/drm/{connector}"));
    if let Some(n) = std::fs::canonicalize(dir.join("ddc"))
        .ok()
        .as_deref()
        .and_then(bus_number)
    {
        return Some(n);
    }
    std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| bus_number(&e.ok()?.path()))
        .min()
}

// NVIDIA links neither. Only the card's own adapters are probed, never a
// touchpad's or the SMBus.
fn edid_bus(connector: &str, used: &HashSet<u32>) -> Option<u32> {
    let edid = std::fs::read(format!("/sys/class/drm/{connector}/edid")).ok()?;
    let edid = edid.get(..128)?;
    let card = connector.split_once('-')?.0;
    let card_dev = std::fs::canonicalize(format!("/sys/class/drm/{card}/device")).ok()?;

    let mut buses: Vec<u32> = std::fs::read_dir("/sys/class/i2c-dev")
        .ok()?
        .filter_map(|e| {
            let e = e.ok()?;
            let n = bus_number(&e.path())?;
            let dev = std::fs::canonicalize(e.path().join("device")).ok()?;
            (dev.parent() == Some(card_dev.as_path())).then_some(n)
        })
        .filter(|n| !used.contains(n))
        .collect();
    buses.sort();
    buses
        .into_iter()
        .find(|&n| ddc::edid(n).is_ok_and(|e| e[..] == *edid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_names_drop_the_card() {
        assert_eq!(output_name("card1-eDP-1"), Some("eDP-1"));
        assert_eq!(output_name("card0-HDMI-A-1"), Some("HDMI-A-1"));
        assert_eq!(output_name("card12-DP-3"), Some("DP-3"));
        assert_eq!(output_name("card0"), None);
        assert_eq!(output_name("renderD128"), None);
        assert_eq!(output_name("card-DP-1"), None);
        assert_eq!(output_name("version"), None);
    }

    #[test]
    fn internal_panels_by_connector_type() {
        assert!(is_internal("eDP-1"));
        assert!(is_internal("LVDS-1"));
        assert!(is_internal("DSI-1"));
        assert!(!is_internal("DP-3"));
        assert!(!is_internal("HDMI-A-1"));
    }

    #[test]
    fn firmware_backlights_outrank_raw() {
        let mut c = vec![
            (true, type_rank("raw"), "intel_backlight"),
            (true, type_rank("firmware"), "acpi_video0"),
        ];
        c.sort();
        assert_eq!(c[0].2, "acpi_video0");

        c.push((false, type_rank("raw"), "nvidia_0"));
        c.sort();
        assert_eq!(c[0].2, "nvidia_0");
    }

    #[test]
    fn bus_numbers_from_adapter_paths() {
        assert_eq!(bus_number(Path::new("/sys/class/i2c-dev/i2c-16")), Some(16));
        assert_eq!(
            bus_number(Path::new("/sys/devices/x/card1-eDP-1/i2c-9")),
            Some(9)
        );
        assert_eq!(bus_number(Path::new("/sys/devices/x/ddc")), None);
    }

    #[test]
    fn commands_parse() {
        let set = Request {
            cmd: "set".into(),
            args: serde_json::json!({"name": "DP-3", "value": 60}),
        };
        assert_eq!(
            Command::try_from(set).unwrap(),
            Command::Set {
                name: "DP-3".into(),
                value: 60
            }
        );
        let rescan = Request {
            cmd: "rescan".into(),
            args: serde_json::Value::Null,
        };
        assert_eq!(Command::try_from(rescan).unwrap(), Command::Rescan);
    }
}
