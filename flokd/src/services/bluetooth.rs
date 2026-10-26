use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::Instant;
use zbus::message::Type as MessageType;
use zbus::wire::OwnedObjectPath;
use zbus::{Connection, MatchRule, MessageStream};

use crate::bus::{Bus, Request};
use crate::dbus::{self, Props, sleep_until, take};

const SERVICE: &str = "bluetooth";

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";

// Discovery fires RSSI changes for every device in range.
const SETTLE: Duration = Duration::from_millis(200);
const RECONNECT: Duration = Duration::from_secs(5);
const DISCOVERY: Duration = Duration::from_secs(15);
// Pair waits on whichever agent answers.
const OP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Serialize, Default, Clone, PartialEq, Debug)]
pub struct State {
    available: bool,
    enabled: bool,
    discovering: bool,
    connected: bool,
    connected_devices: usize,
    busy: bool,
    devices: Vec<Device>,
    #[serde(skip)]
    adapter: String,
}

#[derive(Serialize, Default, Clone, PartialEq, Debug)]
pub struct Device {
    address: String,
    name: String,
    icon: String,
    paired: bool,
    trusted: bool,
    connected: bool,
    // -1 without Battery1, which BlueZ only adds while connected.
    battery: i32,
    connecting: bool,
    #[serde(skip)]
    path: String,
}

type Objects = HashMap<String, HashMap<String, Props>>;

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", content = "args", rename_all = "lowercase")]
enum Command {
    Refresh,
    Enable { on: bool },
    Discover { on: bool },
    Connect { address: String },
    Disconnect { address: String },
    Pair { address: String },
    Trust { address: String },
    Forget { address: String },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Op {
    Connect,
    Disconnect,
    Pair,
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

// hci0 only; the shell draws one adapter.
fn read_state(mut objects: Objects) -> State {
    let Some(adapter) = objects
        .iter()
        .filter(|(_, ifaces)| ifaces.contains_key(ADAPTER))
        .map(|(path, _)| path.clone())
        .min()
    else {
        return State::default();
    };

    let mut a = objects
        .get_mut(&adapter)
        .and_then(|i| i.remove(ADAPTER))
        .unwrap_or_default();
    let enabled = take::<bool>(&mut a, "Powered").unwrap_or(false);
    let discovering = take::<bool>(&mut a, "Discovering").unwrap_or(false);

    let mut devices: Vec<Device> = objects
        .into_iter()
        .filter_map(|(path, mut ifaces)| {
            let mut d = ifaces.remove(DEVICE)?;
            let owner = dbus::take_path(&mut d, "Adapter");
            if owner != adapter {
                return None;
            }
            let address: String = take(&mut d, "Address").unwrap_or_default();
            let name = take::<String>(&mut d, "Alias")
                .or_else(|| take(&mut d, "Name"))
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| address.clone());
            let battery = ifaces
                .remove(BATTERY)
                .and_then(|mut b| take::<u8>(&mut b, "Percentage"))
                .map_or(-1, i32::from);
            Some(Device {
                name,
                icon: take::<String>(&mut d, "Icon").unwrap_or_else(|| "bluetooth".into()),
                paired: take(&mut d, "Paired").unwrap_or(false),
                trusted: take(&mut d, "Trusted").unwrap_or(false),
                connected: take(&mut d, "Connected").unwrap_or(false),
                battery,
                address,
                connecting: false,
                path,
            })
        })
        .collect();

    // Stable order, or every re-read would publish.
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.paired.cmp(&a.paired))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.address.cmp(&b.address))
    });

    let connected_devices = devices.iter().filter(|d| d.connected).count();
    State {
        available: true,
        enabled,
        discovering,
        connected: connected_devices > 0,
        connected_devices,
        busy: false,
        devices,
        adapter,
    }
}

// Trusted so the device reconnects on its own later, as before.
async fn connect(conn: Connection, path: String, trusted: bool, paired: bool) -> Result<()> {
    if !trusted {
        dbus::set(&conn, BLUEZ, &path, DEVICE, "Trusted", true).await?;
    }
    if !paired {
        dbus::call(&conn, BLUEZ, &path, DEVICE, "Pair", &()).await?;
    }
    dbus::call(&conn, BLUEZ, &path, DEVICE, "Connect", &()).await?;
    Ok(())
}

async fn read(conn: &Connection) -> Result<State> {
    let reply = dbus::call(
        conn,
        BLUEZ,
        "/",
        "org.freedesktop.DBus.ObjectManager",
        "GetManagedObjects",
        &(),
    )
    .await?;
    let objects: HashMap<OwnedObjectPath, HashMap<String, Props>> = reply.body().deserialize()?;
    Ok(read_state(
        objects
            .into_iter()
            .map(|(path, ifaces)| (path.as_str().to_string(), ifaces))
            .collect(),
    ))
}

pub struct Service {
    bus: Bus,
    reading: Option<State>,
    published: Option<State>,
    ops: HashMap<String, Op>,
    discover_until: Option<Instant>,
    done_tx: mpsc::Sender<(String, Op, Result<()>)>,
    done_rx: mpsc::Receiver<(String, Op, Result<()>)>,
}

impl Service {
    pub fn new(bus: Bus) -> Self {
        let (done_tx, done_rx) = mpsc::channel(8);
        Self {
            bus,
            reading: None,
            published: None,
            ops: HashMap::new(),
            discover_until: None,
            done_tx,
            done_rx,
        }
    }

    pub async fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        loop {
            if let Err(e) = self.session(&mut requests).await {
                tracing::error!("system bus lost, retrying: {e:#}");
            }
            self.reading = None;
            self.discover_until = None;
            self.publish();
            tokio::time::sleep(RECONNECT).await;
        }
    }

    async fn session(&mut self, requests: &mut mpsc::Receiver<Request>) -> Result<()> {
        let conn = Connection::system().await.context("connect system bus")?;

        // Devices come and go as ObjectManager signals on "/", outside /org/bluez.
        let props = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .path_namespace("/org/bluez")
            .build()?;
        let objects = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .interface("org.freedesktop.DBus.ObjectManager")
            .path("/")
            .build()?;
        let mut props = MessageStream::for_match_rule(props, &conn, None).await?;
        let mut objects = MessageStream::for_match_rule(objects, &conn, None).await?;
        let mut owner = dbus::owner_changes(&conn, BLUEZ).await?;

        self.reread(&conn).await;
        let mut due: Option<Instant> = None;

        loop {
            tokio::select! {
                msg = props.next() => {
                    msg.ok_or_else(|| anyhow!("signal stream ended"))??;
                    due.get_or_insert_with(|| Instant::now() + SETTLE);
                }
                msg = objects.next() => {
                    msg.ok_or_else(|| anyhow!("object stream ended"))??;
                    due.get_or_insert_with(|| Instant::now() + SETTLE);
                }
                msg = owner.next() => {
                    msg.ok_or_else(|| anyhow!("owner stream ended"))??;
                    tracing::info!("BlueZ owner changed");
                    due.get_or_insert_with(|| Instant::now() + SETTLE);
                }
                Some(req) = requests.recv() => match Command::try_from(req) {
                    Ok(cmd) => self.command(&conn, cmd).await,
                    Err(e) => tracing::warn!("bad command: {e}"),
                },
                Some((address, op, result)) = self.done_rx.recv() => {
                    if let Err(e) = result {
                        tracing::warn!(%address, ?op, "{e:#}");
                    }
                    self.ops.remove(&address);
                    self.publish();
                }
                _ = sleep_until(due) => {
                    due = None;
                    self.reread(&conn).await;
                }
                _ = sleep_until(self.discover_until) => {
                    self.discover_until = None;
                    self.discover(&conn, false).await;
                }
            }
        }
    }

    async fn command(&mut self, conn: &Connection, cmd: Command) {
        tracing::debug!(?cmd, "command");
        let result = match cmd {
            Command::Refresh => {
                self.reread(conn).await;
                Ok(())
            }
            Command::Enable { on } => match self.adapter() {
                Some(adapter) => dbus::set(conn, BLUEZ, &adapter, ADAPTER, "Powered", on).await,
                None => Err(anyhow!("no adapter")),
            },
            Command::Discover { on } => {
                self.discover(conn, on).await;
                Ok(())
            }
            Command::Connect { address } => self.spawn(conn, &address, Op::Connect),
            Command::Disconnect { address } => self.spawn(conn, &address, Op::Disconnect),
            Command::Pair { address } => self.spawn(conn, &address, Op::Pair),
            Command::Trust { address } => match self.device(&address) {
                Some(d) => dbus::set(conn, BLUEZ, &d.path, DEVICE, "Trusted", true).await,
                None => Err(anyhow!("unknown device {address}")),
            },
            Command::Forget { address } => self.forget(conn, &address).await,
        };
        if let Err(e) = result {
            tracing::warn!("{e:#}");
        }
        self.publish();
    }

    async fn forget(&self, conn: &Connection, address: &str) -> Result<()> {
        let adapter = self.adapter().context("no adapter")?;
        let d = self
            .device(address)
            .with_context(|| format!("unknown device {address}"))?;
        dbus::call(
            conn,
            BLUEZ,
            &adapter,
            ADAPTER,
            "RemoveDevice",
            &(dbus::path(&d.path)?,),
        )
        .await?;
        Ok(())
    }

    fn adapter(&self) -> Option<String> {
        self.reading
            .as_ref()
            .filter(|r| r.available)
            .map(|r| r.adapter.clone())
    }

    fn device(&self, address: &str) -> Option<&Device> {
        self.reading
            .as_ref()?
            .devices
            .iter()
            .find(|d| d.address == address)
    }

    // Connect and Pair block until the device answers; inline they would stall updates.
    fn spawn(&mut self, conn: &Connection, address: &str, op: Op) -> Result<()> {
        if self.ops.contains_key(address) {
            return Ok(());
        }
        let d = self
            .device(address)
            .with_context(|| format!("unknown device {address}"))?
            .clone();
        let conn = conn.clone();
        let done = self.done_tx.clone();
        let address = address.to_string();
        self.ops.insert(address.clone(), op);
        tokio::spawn(async move {
            let work = async {
                match op {
                    Op::Connect => connect(conn, d.path, d.trusted, d.paired).await,
                    Op::Pair => dbus::call(&conn, BLUEZ, &d.path, DEVICE, "Pair", &())
                        .await
                        .map(drop),
                    Op::Disconnect => dbus::call(&conn, BLUEZ, &d.path, DEVICE, "Disconnect", &())
                        .await
                        .map(drop),
                }
            };
            let result = tokio::time::timeout(OP_TIMEOUT, work)
                .await
                .unwrap_or_else(|_| Err(anyhow!("timed out")));
            let _ = done.send((address, op, result)).await;
        });
        Ok(())
    }

    async fn discover(&mut self, conn: &Connection, on: bool) {
        let Some(adapter) = self.adapter() else {
            return;
        };
        let method = if on {
            "StartDiscovery"
        } else {
            "StopDiscovery"
        };
        match dbus::call(conn, BLUEZ, &adapter, ADAPTER, method, &()).await {
            Ok(_) => self.discover_until = on.then(|| Instant::now() + DISCOVERY),
            Err(e) => tracing::debug!("{e:#}"),
        }
    }

    async fn reread(&mut self, conn: &Connection) {
        match read(conn).await {
            Ok(state) => self.reading = Some(state),
            Err(e) => {
                tracing::warn!("BlueZ unreadable: {e:#}");
                self.reading = None;
            }
        }
        self.publish();
    }

    fn publish(&mut self) {
        let mut state = self.reading.clone().unwrap_or_default();
        for d in &mut state.devices {
            d.connecting = matches!(self.ops.get(&d.address), Some(Op::Connect | Op::Pair));
        }
        state.busy = !self.ops.is_empty();

        if self.published.as_ref() == Some(&state) {
            return;
        }
        match serde_json::to_value(&state) {
            Ok(v) => self.bus.publish(SERVICE, &v),
            Err(e) => tracing::error!("serialise failed: {e}"),
        }
        self.published = Some(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::wire::{OwnedVariant, Variant};

    fn ov<'a>(v: impl Into<Variant<'a>>) -> OwnedVariant {
        OwnedVariant::try_from(v.into()).unwrap()
    }

    fn adapter(powered: bool) -> HashMap<String, Props> {
        HashMap::from([(
            ADAPTER.to_string(),
            HashMap::from([
                ("Powered".to_string(), ov(powered)),
                ("Discovering".to_string(), ov(false)),
            ]),
        )])
    }

    fn device(
        adapter: &str,
        address: &str,
        alias: &str,
        connected: bool,
        paired: bool,
    ) -> HashMap<String, Props> {
        HashMap::from([(
            DEVICE.to_string(),
            HashMap::from([
                (
                    "Adapter".to_string(),
                    OwnedVariant::from(zbus::wire::ObjectPath::try_from(adapter).unwrap()),
                ),
                ("Address".to_string(), ov(address)),
                ("Alias".to_string(), ov(alias)),
                ("Icon".to_string(), ov("audio-headset")),
                ("Connected".to_string(), ov(connected)),
                ("Paired".to_string(), ov(paired)),
                ("Trusted".to_string(), ov(paired)),
            ]),
        )])
    }

    #[test]
    fn no_adapter_is_unavailable() {
        assert_eq!(read_state(Objects::new()), State::default());
    }

    #[test]
    fn devices_sort_connected_then_paired_then_name() {
        let objects = Objects::from([
            ("/org/bluez/hci0".to_string(), adapter(true)),
            (
                "/org/bluez/hci0/dev_1".to_string(),
                device("/org/bluez/hci0", "AA:01", "zebra", false, true),
            ),
            (
                "/org/bluez/hci0/dev_2".to_string(),
                device("/org/bluez/hci0", "AA:02", "Mouse", false, false),
            ),
            (
                "/org/bluez/hci0/dev_3".to_string(),
                device("/org/bluez/hci0", "AA:03", "buds", true, true),
            ),
            (
                "/org/bluez/hci0/dev_4".to_string(),
                device("/org/bluez/hci0", "AA:04", "apple", false, true),
            ),
        ]);
        let s = read_state(objects);
        let names: Vec<_> = s.devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["buds", "apple", "zebra", "Mouse"]);
        assert!(s.available && s.enabled && s.connected);
        assert_eq!(s.connected_devices, 1);
    }

    #[test]
    fn only_the_first_adapter_counts() {
        let objects = Objects::from([
            ("/org/bluez/hci0".to_string(), adapter(true)),
            ("/org/bluez/hci1".to_string(), adapter(false)),
            (
                "/org/bluez/hci0/dev_1".to_string(),
                device("/org/bluez/hci0", "AA:01", "buds", false, true),
            ),
            (
                "/org/bluez/hci1/dev_2".to_string(),
                device("/org/bluez/hci1", "AA:02", "other", true, true),
            ),
        ]);
        let s = read_state(objects);
        assert!(s.enabled);
        assert_eq!(s.devices.len(), 1);
        assert!(!s.connected);
    }

    #[test]
    fn battery_only_with_battery1_and_names_fall_back() {
        let mut dev = device("/org/bluez/hci0", "AA:01", "", true, true);
        dev.insert(
            BATTERY.to_string(),
            HashMap::from([("Percentage".to_string(), OwnedVariant::from(80u8))]),
        );
        let objects = Objects::from([
            ("/org/bluez/hci0".to_string(), adapter(true)),
            ("/org/bluez/hci0/dev_1".to_string(), dev),
            (
                "/org/bluez/hci0/dev_2".to_string(),
                device("/org/bluez/hci0", "AA:02", "mouse", false, true),
            ),
        ]);
        let s = read_state(objects);
        assert_eq!(
            (s.devices[0].name.as_str(), s.devices[0].battery),
            ("AA:01", 80)
        );
        assert_eq!(s.devices[1].battery, -1);
    }

    #[test]
    fn state_serialises_with_the_names_the_facade_reads() {
        let v = serde_json::to_value(State::default()).unwrap();
        for key in [
            "available",
            "enabled",
            "discovering",
            "connected",
            "connected_devices",
            "busy",
            "devices",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert!(v.get("adapter").is_none());
        let d = serde_json::to_value(Device::default()).unwrap();
        assert!(d.get("connecting").is_some());
        assert!(d.get("path").is_none());
    }

    #[test]
    fn commands_parse_with_and_without_args() {
        let parse = |json: &str| Command::try_from(serde_json::from_str::<Request>(json).unwrap());
        assert_eq!(parse(r#"{"cmd":"refresh"}"#).unwrap(), Command::Refresh);
        assert_eq!(
            parse(r#"{"cmd":"discover","args":{"on":true}}"#).unwrap(),
            Command::Discover { on: true }
        );
        assert_eq!(
            parse(r#"{"cmd":"forget","args":{"address":"AA:01"}}"#).unwrap(),
            Command::Forget {
                address: "AA:01".into()
            }
        );
        assert!(parse(r#"{"cmd":"connect"}"#).is_err());
        assert!(parse(r#"{"cmd":"scan"}"#).is_err());
    }

    #[test]
    fn devices_keep_their_object_path() {
        let objects = Objects::from([
            ("/org/bluez/hci0".to_string(), adapter(true)),
            (
                "/org/bluez/hci0/dev_1".to_string(),
                device("/org/bluez/hci0", "AA:01", "buds", false, true),
            ),
        ]);
        let s = read_state(objects);
        assert_eq!(s.adapter, "/org/bluez/hci0");
        assert_eq!(s.devices[0].path, "/org/bluez/hci0/dev_1");
    }
}
