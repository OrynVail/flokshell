use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};
use zbus::OwnedFd;
use zbus::message::Type as MessageType;
use zbus::{Connection, MatchRule, MessageStream};

use crate::bus::{Bus, Request};
use crate::wayland::{self, Note};
use crate::{config, dbus};

const SERVICE: &str = "idle";
const LOGIND: &str = "org.freedesktop.login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
// logind waits this long at most on a delay lock anyway (InhibitDelayMaxSec).
const BEFORE_SLEEP_LIMIT: Duration = Duration::from_secs(4);

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct State {
    caffeine: bool,
    // Between PrepareForSleep(true) and (false).
    suspended: bool,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
struct Listener {
    #[serde(default = "default_timeout")]
    timeout: u32,
    #[serde(default, rename = "onTimeout")]
    on_timeout: String,
    #[serde(default, rename = "onResume")]
    on_resume: String,
}

fn default_timeout() -> u32 {
    60
}

#[derive(Debug, Clone, PartialEq)]
struct Settings {
    lock: String,
    before_sleep: String,
    after_sleep: String,
    listeners: Vec<Listener>,
}

fn settings(system: &Value) -> Settings {
    let idle = &system["idle"];
    let general = &idle["general"];
    let cmd = |key: &str, default: &str| {
        general[key]
            .as_str()
            .map_or_else(|| default.to_string(), str::to_string)
    };
    Settings {
        lock: cmd("lock_cmd", "flok lock"),
        before_sleep: cmd("before_sleep_cmd", "loginctl lock-session"),
        after_sleep: cmd("after_sleep_cmd", "flok screen on"),
        listeners: serde_json::from_value(idle["listeners"].clone()).unwrap_or_default(),
    }
}

// One notification per distinct threshold, in milliseconds.
fn timeouts(listeners: &[Listener]) -> Vec<u32> {
    listeners
        .iter()
        .map(|l| l.timeout.max(1).saturating_mul(1000))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub struct Service {
    bus: Bus,
    wayland: wayland::Handle,
    notes: broadcast::Receiver<Note>,
    state: State,
    settings: Settings,
    // Indices into settings.listeners that have run on_timeout and owe an on_resume.
    fired: Vec<usize>,
}

impl Service {
    pub fn new(bus: Bus, wayland: wayland::Handle, notes: broadcast::Receiver<Note>) -> Self {
        Self {
            bus,
            wayland,
            notes,
            state: State::default(),
            settings: settings(&Value::Null),
            fired: Vec::new(),
        }
    }

    pub async fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        let mut config_changed = match config::watch() {
            Ok(rx) => rx,
            Err(e) => {
                tracing::warn!("config watch unavailable, idle changes need a restart: {e}");
                mpsc::channel(1).1
            }
        };
        self.reload().await;
        self.publish();

        let conn = Connection::system().await.ok();
        let mut _sleep_lock = None;
        let (mut sleeps, mut locks) = match &conn {
            Some(c) => {
                _sleep_lock = take_sleep_lock(c).await;
                (
                    signals(c, MANAGER, "PrepareForSleep", None).await.ok(),
                    match session_path(c).await {
                        Ok(path) => {
                            signals(c, "org.freedesktop.login1.Session", "Lock", Some(&path))
                                .await
                                .ok()
                        }
                        Err(e) => {
                            tracing::warn!("no login session, lock requests unheard: {e:#}");
                            None
                        }
                    },
                )
            }
            None => {
                tracing::warn!("no system bus, sleep and lock hooks are off");
                (None, None)
            }
        };

        loop {
            tokio::select! {
                Some(req) = requests.recv() => match (req.cmd.as_str(), req.args.get("on").and_then(Value::as_bool)) {
                    ("caffeine", Some(on)) => self.caffeine(on),
                    (cmd, _) => tracing::warn!("bad command: {cmd}"),
                },
                note = self.notes.recv() => match note {
                    Ok(Note::Idle { timeout, idle }) => self.idle(timeout, idle),
                    // A new connection starts every notification afresh.
                    Ok(Note::Connected(_)) => self.resume_all(),
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                Some(()) = config_changed.recv() => self.reload().await,
                Some(msg) = next(&mut sleeps) => {
                    let Ok(going) = msg.body().deserialize::<bool>() else { continue };
                    self.state.suspended = going;
                    self.publish();
                    if going {
                        // Lock first, then let logind proceed: the screen must
                        // be locked before the lid opens again.
                        let cmd = self.settings.before_sleep.clone();
                        let _ = tokio::time::timeout(BEFORE_SLEEP_LIMIT, run_wait(&cmd)).await;
                        _sleep_lock = None;
                    } else {
                        spawn(&self.settings.after_sleep);
                        if let Some(c) = &conn {
                            _sleep_lock = take_sleep_lock(c).await;
                        }
                    }
                }
                Some(_) = next(&mut locks) => spawn(&self.settings.lock),
            }
        }
    }

    async fn reload(&mut self) {
        let next = settings(&config::read("system.json").await);
        if next == self.settings {
            return;
        }
        if next.listeners != self.settings.listeners {
            self.resume_all();
        }
        self.wayland
            .send(wayland::Request::Idle(timeouts(&next.listeners)));
        self.settings = next;
    }

    fn idle(&mut self, timeout_ms: u32, idle: bool) {
        let matching: Vec<usize> = self
            .settings
            .listeners
            .iter()
            .enumerate()
            .filter(|(_, l)| l.timeout.max(1).saturating_mul(1000) == timeout_ms)
            .map(|(i, _)| i)
            .collect();
        for i in matching {
            let l = &self.settings.listeners[i];
            if idle && !self.state.caffeine && !self.fired.contains(&i) {
                tracing::info!(timeout = l.timeout, "idle: {}", l.on_timeout);
                spawn(&l.on_timeout);
                self.fired.push(i);
            } else if !idle && let Some(pos) = self.fired.iter().position(|&f| f == i) {
                tracing::info!(timeout = l.timeout, "resume: {}", l.on_resume);
                spawn(&l.on_resume);
                self.fired.remove(pos);
            }
        }
    }

    // Undo the latest first, as if the user had come back.
    fn resume_all(&mut self) {
        for i in self.fired.drain(..).rev() {
            if let Some(l) = self.settings.listeners.get(i) {
                spawn(&l.on_resume);
            }
        }
    }

    fn caffeine(&mut self, on: bool) {
        if on {
            self.resume_all();
        }
        self.state.caffeine = on;
        self.publish();
    }

    fn publish(&self) {
        match serde_json::to_value(&self.state) {
            Ok(v) => self.bus.publish(SERVICE, &v),
            Err(e) => tracing::error!("serialise failed: {e}"),
        }
    }
}

fn spawn(cmd: &str) {
    if cmd.trim().is_empty() {
        return;
    }
    if let Err(e) = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .spawn()
    {
        tracing::warn!(cmd, "spawn failed: {e}");
    }
}

async fn run_wait(cmd: &str) {
    if cmd.trim().is_empty() {
        return;
    }
    match tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .status()
        .await
    {
        Ok(s) if !s.success() => tracing::warn!(cmd, "exited {s}"),
        Err(e) => tracing::warn!(cmd, "spawn failed: {e}"),
        _ => {}
    }
}

async fn next(stream: &mut Option<MessageStream>) -> Option<zbus::Message> {
    match stream {
        Some(s) => s.next().await?.ok(),
        None => std::future::pending().await,
    }
}

async fn signals(
    conn: &Connection,
    iface: &str,
    member: &str,
    path: Option<&str>,
) -> Result<MessageStream> {
    let mut rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(LOGIND)
        .interface(iface)
        .member(member);
    if let Some(p) = path {
        rule = rule.path(p.to_string());
    }
    Ok(MessageStream::for_match_rule(rule.build()?, conn, None).await?)
}

// "auto" from a user service resolves to the user's display session.
async fn session_path(conn: &Connection) -> Result<String> {
    let reply = dbus::call(
        conn,
        LOGIND,
        "/org/freedesktop/login1",
        MANAGER,
        "GetSession",
        &("auto",),
    )
    .await?;
    let path: zbus::OwnedObjectPath = reply.body().deserialize().context("GetSession reply")?;
    Ok(path.as_str().to_string())
}

// Held while awake. Dropping it is what tells logind the before-sleep work is done.
async fn take_sleep_lock(conn: &Connection) -> Option<OwnedFd> {
    let reply = dbus::call(
        conn,
        LOGIND,
        "/org/freedesktop/login1",
        MANAGER,
        "Inhibit",
        &(
            "sleep",
            "flokshell",
            "Lock the screen before sleep",
            "delay",
        ),
    )
    .await
    .inspect_err(|e| tracing::warn!("no sleep delay lock: {e:#}"))
    .ok()?;
    reply.body().deserialize::<OwnedFd>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_fall_back_per_field() {
        let s = settings(&serde_json::json!({
            "idle": {
                "general": {"lock_cmd": "swaylock"},
                "listeners": [
                    {"timeout": 150, "onTimeout": "dim", "onResume": "undim"},
                    {"onTimeout": "x"}
                ]
            }
        }));
        assert_eq!(s.lock, "swaylock");
        assert_eq!(s.before_sleep, "loginctl lock-session");
        assert_eq!(s.listeners[0].on_resume, "undim");
        assert_eq!(s.listeners[1].timeout, 60);
        assert!(settings(&Value::Null).listeners.is_empty());
    }

    #[test]
    fn shared_thresholds_need_one_notification() {
        let l = |t| Listener {
            timeout: t,
            on_timeout: String::new(),
            on_resume: String::new(),
        };
        assert_eq!(
            timeouts(&[l(300), l(150), l(300), l(0)]),
            [1000, 150_000, 300_000]
        );
    }
}
