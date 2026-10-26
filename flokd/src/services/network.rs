use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::Instant;
use zbus::message::Type as MessageType;
use zbus::wire::{ObjectPath, OwnedObjectPath, OwnedVariant, Type, Variant};
use zbus::{Connection, MatchRule, Message, MessageStream};

use crate::bus::{Bus, Request};

const SERVICE: &str = "network";

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const PROFILE: &str = "org.freedesktop.NetworkManager.Settings.Connection";

// NM reports one change as a burst across several objects; re-read once it is over.
const SETTLE: Duration = Duration::from_millis(200);
const RECONNECT: Duration = Duration::from_secs(5);
// Requests inside the cooldown are held and run when it closes, not dropped.
const SCAN_COOLDOWN: Duration = Duration::from_secs(10);
// Bounds for when the signal that ends a scan or a connect never arrives.
const SCAN_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

const DEVICE_ETHERNET: u32 = 1;
const DEVICE_WIFI: u32 = 2;

const STATE_UNAVAILABLE: u32 = 20;
const STATE_ACTIVATED: u32 = 100;
const STATE_FAILED: u32 = 120;

const REASON_NO_SECRETS: u32 = 7;

const CONNECTIVITY_LIMITED: u32 = 3;

const AP_PRIVACY: u32 = 0x1;
const KEY_PSK: u32 = 0x100;
const KEY_8021X: u32 = 0x200;
const KEY_SAE: u32 = 0x400;
const KEY_OWE: u32 = 0x800;
const KEY_OWE_TM: u32 = 0x1000;

#[derive(Serialize, Default, Clone, PartialEq, Debug)]
pub struct State {
    available: bool,
    enabled: bool,
    // Wifi carrying traffic, not wifi existing: "limited" reads as false.
    wifi: bool,
    ethernet: bool,
    status: &'static str,
    name: String,
    strength: u8,
    scanning: bool,
    scan_queued: bool,
    connecting: Option<String>,
    // Held until the next attempt, so a client connecting late still sees it.
    needs_password: Option<String>,
    networks: Vec<Network>,
}

#[derive(Serialize, Default, Clone, PartialEq, Debug)]
pub struct Network {
    active: bool,
    strength: u8,
    frequency: u32,
    ssid: String,
    bssid: String,
    security: String,
}

#[derive(Default, Clone, Debug)]
struct AccessPoint {
    path: String,
    ssid: String,
    bssid: String,
    strength: u8,
    frequency: u32,
    flags: u32,
    wpa: u32,
    rsn: u32,
}

#[derive(Default)]
struct Reading {
    state: State,
    device: Option<String>,
    active_connection: String,
    last_scan: i64,
    aps: Vec<AccessPoint>,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", content = "args", rename_all = "lowercase")]
enum Command {
    Refresh,
    Enable { on: bool },
    Rescan,
    Connect { ssid: String },
    Password { ssid: String, psk: String },
    Disconnect,
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

// Same strings `nmcli -f SECURITY` printed; empty is an open network.
fn security(flags: u32, wpa: u32, rsn: u32) -> String {
    let mut out = Vec::new();
    if flags & AP_PRIVACY != 0 && wpa == 0 && rsn == 0 {
        out.push("WEP");
    }
    if wpa != 0 {
        out.push("WPA1");
    }
    if rsn & (KEY_PSK | KEY_8021X) != 0 {
        out.push("WPA2");
    }
    if rsn & KEY_SAE != 0 {
        out.push("WPA3");
    }
    if rsn & (KEY_OWE | KEY_OWE_TM) != 0 {
        out.push("OWE");
    }
    if (wpa | rsn) & KEY_8021X != 0 {
        out.push("802.1X");
    }
    out.join(" ")
}

// SAE only when the radio offers nothing older; a transition network takes wpa-psk.
fn key_mgmt(ap: &AccessPoint) -> Result<&'static str> {
    if ap.wpa == 0 && ap.rsn & (KEY_PSK | KEY_8021X) == 0 && ap.rsn & KEY_SAE != 0 {
        Ok("sae")
    } else if ap.rsn & KEY_8021X != 0 || ap.wpa & KEY_8021X != 0 {
        bail!("enterprise networks need more than a password")
    } else if ap.wpa == 0 && ap.rsn == 0 {
        bail!("{} takes no password", ap.ssid)
    } else {
        Ok("wpa-psk")
    }
}

fn status(enabled: bool, wifi_state: Option<u32>, connectivity: u32) -> (&'static str, bool) {
    match wifi_state {
        _ if !enabled => ("disabled", false),
        Some(STATE_ACTIVATED) if connectivity == CONNECTIVITY_LIMITED => ("limited", false),
        Some(STATE_ACTIVATED) => ("connected", true),
        Some(40..=90) => ("connecting", false),
        Some(STATE_UNAVAILABLE) => ("disabled", false),
        _ => ("disconnected", false),
    }
}

// One row per SSID; the active radio wins, then the strongest.
fn networks(aps: &[AccessPoint], active: &str) -> Vec<Network> {
    let mut by_ssid: HashMap<&str, Network> = HashMap::new();
    for ap in aps.iter().filter(|ap| !ap.ssid.is_empty()) {
        let net = Network {
            active: ap.path == active,
            strength: ap.strength,
            frequency: ap.frequency,
            ssid: ap.ssid.clone(),
            bssid: ap.bssid.clone(),
            security: security(ap.flags, ap.wpa, ap.rsn),
        };
        match by_ssid.get(ap.ssid.as_str()) {
            Some(seen) if seen.active || (!net.active && seen.strength >= net.strength) => {}
            _ => {
                by_ssid.insert(&ap.ssid, net);
            }
        }
    }
    let mut out: Vec<Network> = by_ssid.into_values().collect();
    out.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.strength.cmp(&a.strength))
            .then_with(|| a.ssid.cmp(&b.ssid))
    });
    out
}

fn strongest<'a>(aps: &'a [AccessPoint], ssid: &str) -> Option<&'a AccessPoint> {
    aps.iter()
        .filter(|ap| ap.ssid == ssid)
        .max_by_key(|ap| ap.strength)
}

type Props = HashMap<String, OwnedVariant>;
type Settings = HashMap<String, HashMap<String, OwnedVariant>>;

fn take<T>(props: &mut Props, key: &str) -> Option<T>
where
    T: TryFrom<OwnedVariant>,
{
    props.remove(key).and_then(|v| T::try_from(v).ok())
}

fn take_path(props: &mut Props, key: &str) -> String {
    take::<OwnedObjectPath>(props, key)
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into())
}

fn value<T>(settings: &Settings, group: &str, key: &str) -> Option<T>
where
    T: TryFrom<OwnedVariant>,
{
    let v = settings.get(group)?.get(key)?.clone();
    T::try_from(v).ok()
}

// The most recently used profile wins, as it did under nmcli.
fn profile_for(settings: &Settings, ssid: &str) -> Option<u64> {
    let kind: String = value(settings, "connection", "type")?;
    let saved: Vec<u8> = value(settings, "802-11-wireless", "ssid")?;
    (kind == "802-11-wireless" && saved == ssid.as_bytes())
        .then(|| value(settings, "connection", "timestamp").unwrap_or(0))
}

// GetSettings returns the deprecated `addresses`/`routes` beside `*-data`; only the latter go back.
fn with_psk(mut settings: Settings, psk: &str, key_mgmt: &str) -> Result<Settings> {
    for ip in ["ipv4", "ipv6"] {
        if let Some(group) = settings.get_mut(ip) {
            group.remove("addresses");
            group.remove("routes");
        }
    }
    let security = settings
        .entry("802-11-wireless-security".into())
        .or_default();
    security
        .entry("key-mgmt".into())
        .or_insert(OwnedVariant::try_from(Variant::from(key_mgmt))?);
    security.insert("psk".into(), OwnedVariant::try_from(Variant::from(psk))?);
    Ok(settings)
}

// NM completes the rest from the access point.
fn new_profile(
    ssid: &str,
    psk: Option<(&str, &str)>,
) -> HashMap<&'static str, HashMap<&'static str, Variant<'static>>> {
    let mut out = HashMap::new();
    out.insert(
        "802-11-wireless",
        HashMap::from([("ssid", Variant::from(ssid.as_bytes().to_vec()))]),
    );
    if let Some((psk, key_mgmt)) = psk {
        out.insert(
            "802-11-wireless-security",
            HashMap::from([
                ("key-mgmt", Variant::from(key_mgmt.to_string())),
                ("psk", Variant::from(psk.to_string())),
            ]),
        );
    }
    out
}

fn path(s: &str) -> Result<ObjectPath<'_>> {
    Ok(ObjectPath::try_from(s)?)
}

async fn call<B>(
    conn: &Connection,
    path: &str,
    iface: &str,
    method: &str,
    body: &B,
) -> Result<Message>
where
    B: Serialize + Type,
{
    conn.call_method(Some(NM), path, Some(iface), method, body)
        .await
        .with_context(|| format!("{iface}.{method} on {path}"))
}

async fn get_all(conn: &Connection, path: &str, iface: &str) -> Result<Props> {
    let reply = call(
        conn,
        path,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &(iface,),
    )
    .await?;
    Ok(reply.body().deserialize()?)
}

async fn read(conn: &Connection) -> Result<Reading> {
    let mut root = get_all(conn, NM_PATH, NM).await?;
    let enabled = take::<bool>(&mut root, "WirelessEnabled").unwrap_or(false);
    let connectivity = take::<u32>(&mut root, "Connectivity").unwrap_or(0);
    let primary = take_path(&mut root, "PrimaryConnection");
    let devices: Vec<OwnedObjectPath> = take(&mut root, "Devices").unwrap_or_default();

    let mut ethernet = false;
    let mut wifi_device = None;
    for dev in &devices {
        let mut props = get_all(conn, dev.as_str(), DEVICE).await?;
        let kind = take::<u32>(&mut props, "DeviceType").unwrap_or(0);
        let state = take::<u32>(&mut props, "State").unwrap_or(0);
        match kind {
            DEVICE_ETHERNET if state == STATE_ACTIVATED => ethernet = true,
            DEVICE_WIFI if wifi_device.is_none() => {
                let active = take_path(&mut props, "ActiveConnection");
                wifi_device = Some((dev.as_str().to_string(), state, active));
            }
            _ => {}
        }
    }

    let mut aps = Vec::new();
    let mut active_ap = "/".to_string();
    let mut last_scan = 0;
    if let Some((dev, _, _)) = &wifi_device {
        let mut props = get_all(conn, dev, WIRELESS).await?;
        active_ap = take_path(&mut props, "ActiveAccessPoint");
        last_scan = take(&mut props, "LastScan").unwrap_or(0);
        let paths: Vec<OwnedObjectPath> = take(&mut props, "AccessPoints").unwrap_or_default();
        for path in &paths {
            let Ok(mut p) = get_all(conn, path.as_str(), ACCESS_POINT).await else {
                continue;
            };
            let ssid: Vec<u8> = take(&mut p, "Ssid").unwrap_or_default();
            aps.push(AccessPoint {
                path: path.as_str().to_string(),
                ssid: String::from_utf8_lossy(&ssid).into_owned(),
                bssid: take(&mut p, "HwAddress").unwrap_or_default(),
                strength: take(&mut p, "Strength").unwrap_or(0),
                frequency: take(&mut p, "Frequency").unwrap_or(0),
                flags: take(&mut p, "Flags").unwrap_or(0),
                wpa: take(&mut p, "WpaFlags").unwrap_or(0),
                rsn: take(&mut p, "RsnFlags").unwrap_or(0),
            });
        }
    }

    let name = if primary == "/" {
        String::new()
    } else {
        get_all(conn, &primary, ACTIVE)
            .await
            .ok()
            .and_then(|mut p| take::<String>(&mut p, "Id"))
            .unwrap_or_default()
    };

    let (status, wifi) = status(
        enabled,
        wifi_device.as_ref().map(|(_, s, _)| *s),
        connectivity,
    );
    let strength = aps
        .iter()
        .find(|ap| ap.path == active_ap)
        .map_or(0, |ap| ap.strength);

    Ok(Reading {
        state: State {
            available: true,
            enabled,
            wifi,
            ethernet,
            status,
            name,
            strength,
            networks: networks(&aps, &active_ap),
            ..State::default()
        },
        active_connection: wifi_device
            .as_ref()
            .map_or_else(|| "/".into(), |(_, _, a)| a.clone()),
        device: wifi_device.map(|(d, _, _)| d),
        last_scan,
        aps,
    })
}

async fn find_profile(conn: &Connection, ssid: &str) -> Result<Option<String>> {
    let reply = call(conn, SETTINGS_PATH, SETTINGS, "ListConnections", &()).await?;
    let paths: Vec<OwnedObjectPath> = reply.body().deserialize()?;
    let mut best: Option<(u64, String)> = None;
    for p in &paths {
        let Ok(reply) = call(conn, p.as_str(), PROFILE, "GetSettings", &()).await else {
            continue;
        };
        let settings: Settings = reply.body().deserialize()?;
        if let Some(used) = profile_for(&settings, ssid)
            && best.as_ref().is_none_or(|(b, _)| used > *b)
        {
            best = Some((used, p.as_str().to_string()));
        }
    }
    Ok(best.map(|(_, p)| p))
}

struct Scan {
    in_flight: Option<Instant>,
    baseline: i64,
    last_request: Option<Instant>,
    queued_until: Option<Instant>,
}

struct Pending {
    ssid: String,
    device: String,
    // Removed if the attempt fails, as nmcli did.
    created: Option<String>,
    deadline: Instant,
}

pub struct Service {
    bus: Bus,
    reading: Option<Reading>,
    published: Option<State>,
    scan: Scan,
    pending: Option<Pending>,
    needs_password: Option<String>,
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

impl Service {
    pub fn new(bus: Bus) -> Self {
        Self {
            bus,
            reading: None,
            published: None,
            scan: Scan {
                in_flight: None,
                baseline: 0,
                last_request: None,
                queued_until: None,
            },
            pending: None,
            needs_password: None,
        }
    }

    pub async fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        loop {
            if let Err(e) = self.session(&mut requests).await {
                tracing::error!("system bus lost, retrying: {e:#}");
            }
            self.reading = None;
            self.pending = None;
            self.scan.in_flight = None;
            self.publish();
            tokio::time::sleep(RECONNECT).await;
        }
    }

    async fn session(&mut self, requests: &mut mpsc::Receiver<Request>) -> Result<()> {
        let conn = Connection::system().await.context("connect system bus")?;

        // Bus-side matches outlive an NM restart; the owner stream reports one.
        let changes = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .path_namespace(NM_PATH)
            .build()?;
        let owner = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender("org.freedesktop.DBus")
            .member("NameOwnerChanged")
            .arg(0, NM)
            .build()?;
        let mut changes = MessageStream::for_match_rule(changes, &conn, None).await?;
        let mut owner = MessageStream::for_match_rule(owner, &conn, None).await?;

        self.reread(&conn).await;
        let mut due: Option<Instant> = None;

        loop {
            let scan_timeout = self.scan.in_flight.map(|at| at + SCAN_TIMEOUT);
            let connect_timeout = self.pending.as_ref().map(|p| p.deadline);
            tokio::select! {
                msg = changes.next() => {
                    let msg = msg.ok_or_else(|| anyhow!("signal stream ended"))??;
                    self.signal(&conn, &msg).await;
                    due.get_or_insert_with(|| Instant::now() + SETTLE);
                }
                msg = owner.next() => {
                    msg.ok_or_else(|| anyhow!("owner stream ended"))??;
                    tracing::info!("NetworkManager owner changed");
                    due.get_or_insert_with(|| Instant::now() + SETTLE);
                }
                Some(req) = requests.recv() => {
                    match Command::try_from(req) {
                        Ok(cmd) => self.command(&conn, cmd).await,
                        Err(e) => tracing::warn!("bad command: {e}"),
                    }
                }
                _ = sleep_until(due) => {
                    due = None;
                    self.reread(&conn).await;
                }
                _ = sleep_until(self.scan.queued_until) => {
                    self.scan.queued_until = None;
                    self.rescan(&conn).await;
                }
                _ = sleep_until(scan_timeout) => {
                    tracing::warn!("scan never reported back");
                    self.scan.in_flight = None;
                    self.publish();
                }
                _ = sleep_until(connect_timeout) => {
                    if let Some(p) = self.pending.take() {
                        tracing::warn!(ssid = %p.ssid, "connect never settled");
                    }
                    self.publish();
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
            Command::Enable { on } => self.enable(conn, on).await,
            Command::Rescan => {
                self.request_rescan(conn).await;
                Ok(())
            }
            Command::Connect { ssid } => self.connect(conn, &ssid, None).await,
            Command::Password { ssid, psk } => self.connect(conn, &ssid, Some(&psk)).await,
            Command::Disconnect => self.disconnect(conn).await,
        };
        if let Err(e) = result {
            tracing::warn!("{e:#}");
        }
        self.publish();
    }

    async fn enable(&mut self, conn: &Connection, on: bool) -> Result<()> {
        call(
            conn,
            NM_PATH,
            "org.freedesktop.DBus.Properties",
            "Set",
            &(NM, "WirelessEnabled", Variant::from(on)),
        )
        .await?;
        Ok(())
    }

    async fn request_rescan(&mut self, conn: &Connection) {
        if self.scan.in_flight.is_some() || self.scan.queued_until.is_some() {
            return;
        }
        if let Some(last) = self.scan.last_request
            && last.elapsed() < SCAN_COOLDOWN
        {
            self.scan.queued_until = Some(last + SCAN_COOLDOWN);
            return;
        }
        self.rescan(conn).await;
    }

    async fn rescan(&mut self, conn: &Connection) {
        let Some(reading) = &self.reading else { return };
        let Some(device) = reading.device.clone() else {
            return;
        };
        self.scan.baseline = reading.last_scan;
        self.scan.last_request = Some(Instant::now());
        let options: HashMap<&str, Variant> = HashMap::new();
        match call(conn, &device, WIRELESS, "RequestScan", &(options,)).await {
            Ok(_) => self.scan.in_flight = Some(Instant::now()),
            Err(e) => tracing::warn!("{e:#}"),
        }
        self.publish();
    }

    async fn connect(&mut self, conn: &Connection, ssid: &str, psk: Option<&str>) -> Result<()> {
        let reading = self
            .reading
            .as_ref()
            .context("NetworkManager not read yet")?;
        let device = reading.device.clone().context("no wifi device")?;
        let ap = strongest(&reading.aps, ssid).cloned();
        self.needs_password = None;

        let psk = match psk {
            Some(psk) => {
                let ap = ap
                    .as_ref()
                    .with_context(|| format!("{ssid} is out of range"))?;
                Some((psk, key_mgmt(ap)?))
            }
            None => None,
        };

        let mut created = None;
        match (find_profile(conn, ssid).await?, psk) {
            (Some(profile), psk) => {
                if let Some((psk, key_mgmt)) = psk {
                    let reply = call(conn, &profile, PROFILE, "GetSettings", &()).await?;
                    let settings = with_psk(reply.body().deserialize()?, psk, key_mgmt)?;
                    call(conn, &profile, PROFILE, "Update", &(settings,)).await?;
                }
                call(
                    conn,
                    NM_PATH,
                    NM,
                    "ActivateConnection",
                    &(path(&profile)?, path(&device)?, path("/")?),
                )
                .await?;
            }
            (None, psk) => {
                let ap_path = ap.as_ref().map_or("/", |ap| ap.path.as_str());
                let reply = call(
                    conn,
                    NM_PATH,
                    NM,
                    "AddAndActivateConnection",
                    &(new_profile(ssid, psk), path(&device)?, path(ap_path)?),
                )
                .await?;
                let (profile, _active): (OwnedObjectPath, OwnedObjectPath) =
                    reply.body().deserialize()?;
                created = Some(profile.as_str().to_string());
            }
        }

        self.pending = Some(Pending {
            ssid: ssid.to_string(),
            device,
            created,
            deadline: Instant::now() + CONNECT_TIMEOUT,
        });
        Ok(())
    }

    async fn disconnect(&mut self, conn: &Connection) -> Result<()> {
        let reading = self
            .reading
            .as_ref()
            .context("NetworkManager not read yet")?;
        if reading.active_connection == "/" {
            return Ok(());
        }
        let device = reading.device.as_deref().context("no wifi device")?;
        // DeactivateConnection blocks one profile and autoconnect raises the next.
        call(conn, device, DEVICE, "Disconnect", &()).await?;
        Ok(())
    }

    // The only place NM says why an activation failed; no property keeps it.
    async fn signal(&mut self, conn: &Connection, msg: &Message) {
        let header = msg.header();
        if header.interface().map(|i| i.as_str()) != Some(DEVICE)
            || header.member().map(|m| m.as_str()) != Some("StateChanged")
        {
            return;
        }
        let Some(pending) = &self.pending else { return };
        if header.path().map(|p| p.as_str()) != Some(pending.device.as_str()) {
            return;
        }
        let Ok((new, _old, reason)) = msg.body().deserialize::<(u32, u32, u32)>() else {
            return;
        };

        match new {
            STATE_ACTIVATED => self.pending = None,
            STATE_FAILED => {
                let Some(p) = self.pending.take() else { return };
                if reason == REASON_NO_SECRETS {
                    self.needs_password = Some(p.ssid.clone());
                } else {
                    tracing::warn!(ssid = %p.ssid, reason, "connect failed");
                }
                if let Some(profile) = p.created
                    && let Err(e) = call(conn, &profile, PROFILE, "Delete", &()).await
                {
                    tracing::warn!("{e:#}");
                }
            }
            _ => return,
        }
        self.publish();
    }

    async fn reread(&mut self, conn: &Connection) {
        match read(conn).await {
            Ok(reading) => {
                if self.scan.in_flight.is_some() && reading.last_scan != self.scan.baseline {
                    self.scan.in_flight = None;
                }
                self.reading = Some(reading);
            }
            Err(e) => {
                tracing::warn!("NetworkManager unreadable: {e:#}");
                self.reading = None;
            }
        }
        self.publish();
    }

    // Most re-reads change nothing the shell draws; those stay off the socket.
    fn publish(&mut self) {
        let mut state = match &self.reading {
            Some(r) => r.state.clone(),
            None => State {
                status: "disabled",
                ..State::default()
            },
        };
        state.scanning = self.scan.in_flight.is_some();
        state.scan_queued = self.scan.queued_until.is_some();
        state.connecting = self.pending.as_ref().map(|p| p.ssid.clone());
        state.needs_password = self.needs_password.clone();

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

    fn ap(path: &str, ssid: &str, strength: u8) -> AccessPoint {
        AccessPoint {
            path: path.into(),
            ssid: ssid.into(),
            bssid: format!("AA:BB:CC:DD:EE:{:02X}", strength),
            strength,
            frequency: 2412,
            ..AccessPoint::default()
        }
    }

    fn secured(wpa: u32, rsn: u32) -> AccessPoint {
        AccessPoint {
            ssid: "home".into(),
            flags: AP_PRIVACY,
            wpa,
            rsn,
            ..AccessPoint::default()
        }
    }

    fn request(json: &str) -> Request {
        serde_json::from_str(json).unwrap()
    }

    fn ov(v: Variant) -> OwnedVariant {
        OwnedVariant::try_from(v).unwrap()
    }

    fn profile(kind: &str, ssid: &str, timestamp: u64) -> Settings {
        HashMap::from([
            (
                "connection".to_string(),
                HashMap::from([
                    ("type".to_string(), ov(Variant::from(kind))),
                    ("timestamp".to_string(), OwnedVariant::from(timestamp)),
                ]),
            ),
            (
                "802-11-wireless".to_string(),
                HashMap::from([(
                    "ssid".to_string(),
                    ov(Variant::from(ssid.as_bytes().to_vec())),
                )]),
            ),
        ])
    }

    #[test]
    fn security_matches_nmcli() {
        assert_eq!(security(0, 0, 0), "");
        assert_eq!(security(AP_PRIVACY, 0, 0), "WEP");
        assert_eq!(security(AP_PRIVACY, 0, KEY_PSK | 0x8), "WPA2");
        assert_eq!(security(AP_PRIVACY, KEY_PSK, KEY_PSK), "WPA1 WPA2");
        assert_eq!(security(AP_PRIVACY, 0, KEY_PSK | KEY_SAE), "WPA2 WPA3");
        assert_eq!(security(AP_PRIVACY, 0, KEY_8021X), "WPA2 802.1X");
        assert_eq!(security(0, 0, KEY_OWE_TM), "OWE");
    }

    #[test]
    fn status_follows_the_wifi_device() {
        assert_eq!(status(false, Some(STATE_ACTIVATED), 4), ("disabled", false));
        assert_eq!(status(true, Some(STATE_ACTIVATED), 4), ("connected", true));
        assert_eq!(
            status(true, Some(STATE_ACTIVATED), CONNECTIVITY_LIMITED),
            ("limited", false)
        );
        assert_eq!(status(true, Some(50), 4), ("connecting", false));
        assert_eq!(
            status(true, Some(STATE_UNAVAILABLE), 1),
            ("disabled", false)
        );
        assert_eq!(status(true, Some(30), 1), ("disconnected", false));
        assert_eq!(status(true, Some(110), 1), ("disconnected", false));
        assert_eq!(status(true, None, 4), ("disconnected", false));
    }

    #[test]
    fn one_row_per_ssid_and_the_active_radio_wins() {
        let aps = [
            ap("/ap/1", "home", 40),
            ap("/ap/2", "home", 90),
            ap("/ap/3", "cafe", 70),
            ap("/ap/4", "", 99),
        ];
        let nets = networks(&aps, "/ap/1");
        let rows: Vec<_> = nets
            .iter()
            .map(|n| (n.ssid.as_str(), n.strength, n.active))
            .collect();
        assert_eq!(rows, [("home", 40, true), ("cafe", 70, false)]);
    }

    #[test]
    fn without_an_active_radio_the_strongest_wins() {
        let aps = [
            ap("/ap/1", "home", 40),
            ap("/ap/2", "home", 90),
            ap("/ap/3", "cafe", 70),
        ];
        let nets = networks(&aps, "/");
        let rows: Vec<_> = nets.iter().map(|n| (n.ssid.as_str(), n.strength)).collect();
        assert_eq!(rows, [("home", 90), ("cafe", 70)]);
        assert_eq!(strongest(&aps, "home").unwrap().path, "/ap/2");
        assert!(strongest(&aps, "gone").is_none());
    }

    #[test]
    fn state_serialises_with_the_names_the_facade_reads() {
        let v = serde_json::to_value(State {
            status: "connected",
            ..State::default()
        })
        .unwrap();
        for key in [
            "available",
            "enabled",
            "wifi",
            "ethernet",
            "status",
            "name",
            "strength",
            "scanning",
            "scan_queued",
            "connecting",
            "needs_password",
            "networks",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn commands_parse_with_and_without_args() {
        let parse = |json| Command::try_from(request(json));
        assert_eq!(parse(r#"{"cmd":"rescan"}"#).unwrap(), Command::Rescan);
        assert_eq!(
            parse(r#"{"cmd":"disconnect","args":null}"#).unwrap(),
            Command::Disconnect
        );
        assert_eq!(
            parse(r#"{"cmd":"enable","args":{"on":false}}"#).unwrap(),
            Command::Enable { on: false }
        );
        assert_eq!(
            parse(r#"{"cmd":"password","args":{"ssid":"home","psk":"hunter2"}}"#).unwrap(),
            Command::Password {
                ssid: "home".into(),
                psk: "hunter2".into()
            }
        );
        assert!(parse(r#"{"cmd":"connect"}"#).is_err());
        assert!(parse(r#"{"cmd":"connect","args":{}}"#).is_err());
        assert!(parse(r#"{"cmd":"reboot"}"#).is_err());
    }

    #[test]
    fn key_mgmt_follows_what_the_radio_offers() {
        assert_eq!(key_mgmt(&secured(0, KEY_PSK)).unwrap(), "wpa-psk");
        assert_eq!(key_mgmt(&secured(KEY_PSK, KEY_PSK)).unwrap(), "wpa-psk");
        assert_eq!(key_mgmt(&secured(0, KEY_PSK | KEY_SAE)).unwrap(), "wpa-psk");
        assert_eq!(key_mgmt(&secured(0, KEY_SAE)).unwrap(), "sae");
        assert!(key_mgmt(&secured(0, KEY_8021X)).is_err());
        assert!(key_mgmt(&secured(0, 0)).is_err());
    }

    #[test]
    fn profiles_match_on_ssid_bytes_and_type() {
        assert_eq!(
            profile_for(&profile("802-11-wireless", "home", 42), "home"),
            Some(42)
        );
        assert_eq!(
            profile_for(&profile("802-11-wireless", "home", 42), "Home"),
            None
        );
        assert_eq!(
            profile_for(&profile("802-3-ethernet", "home", 42), "home"),
            None
        );
        assert_eq!(profile_for(&Settings::new(), "home"), None);
    }

    #[test]
    fn a_new_password_keeps_the_profile_and_drops_deprecated_keys() {
        let mut settings = profile("802-11-wireless", "home", 1);
        settings.insert(
            "ipv4".into(),
            HashMap::from([
                ("method".to_string(), ov(Variant::from("auto"))),
                ("addresses".to_string(), OwnedVariant::from(0u32)),
                ("routes".to_string(), OwnedVariant::from(0u32)),
            ]),
        );
        let out = with_psk(settings, "hunter2", "wpa-psk").unwrap();
        assert_eq!(
            value::<String>(&out, "802-11-wireless-security", "psk").unwrap(),
            "hunter2"
        );
        assert_eq!(
            value::<String>(&out, "802-11-wireless-security", "key-mgmt").unwrap(),
            "wpa-psk"
        );
        assert_eq!(value::<String>(&out, "ipv4", "method").unwrap(), "auto");
        assert!(!out["ipv4"].contains_key("addresses"));
        assert!(!out["ipv4"].contains_key("routes"));
        assert_eq!(profile_for(&out, "home"), Some(1));
    }

    #[test]
    fn an_existing_key_mgmt_is_not_overwritten() {
        let mut settings = profile("802-11-wireless", "home", 1);
        settings.insert(
            "802-11-wireless-security".into(),
            HashMap::from([("key-mgmt".to_string(), ov(Variant::from("sae")))]),
        );
        let out = with_psk(settings, "hunter2", "wpa-psk").unwrap();
        assert_eq!(
            value::<String>(&out, "802-11-wireless-security", "key-mgmt").unwrap(),
            "sae"
        );
    }

    #[test]
    fn new_profiles_carry_security_only_with_a_password() {
        let open = new_profile("cafe", None);
        assert!(open.contains_key("802-11-wireless"));
        assert!(!open.contains_key("802-11-wireless-security"));
        let locked = new_profile("home", Some(("hunter2", "wpa-psk")));
        assert!(locked.contains_key("802-11-wireless-security"));
    }
}
