use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use futures_util::FutureExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::bus::{Bus, Change, Request};
use crate::hyprland::{self, Instance};

const SERVICE: &str = "compositor";
const RECONNECT: Duration = Duration::from_secs(2);

#[derive(Deserialize, Serialize, Default, Clone, Debug, PartialEq)]
pub struct WorkspaceRef {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct RawClient {
    address: String,
    #[serde(default)]
    mapped: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    at: [i64; 2],
    #[serde(default)]
    size: [i64; 2],
    #[serde(default)]
    workspace: WorkspaceRef,
    #[serde(default)]
    floating: bool,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    monitor: i64,
    #[serde(default)]
    class: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    initial_class: String,
    #[serde(default)]
    initial_title: String,
    #[serde(default)]
    pid: i64,
    #[serde(default)]
    xwayland: bool,
    #[serde(default)]
    fullscreen: i64,
    #[serde(default, rename = "focusHistoryID")]
    focus_history_id: i64,
}

// focusHistoryID is left out on purpose: Hyprland renumbers every window on
// each focus change, which would resend them all. The order rides on `focus`.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Client {
    address: String,
    class: String,
    title: String,
    initial_class: String,
    initial_title: String,
    workspace: WorkspaceRef,
    monitor: i64,
    floating: bool,
    // Covers the screen: internal mode 2, told or not. Maximised (1) does not.
    fullscreen: bool,
    pinned: bool,
    hidden: bool,
    mapped: bool,
    at: [i64; 2],
    size: [i64; 2],
    xwayland: bool,
    pid: i64,
}

impl From<&RawClient> for Client {
    fn from(c: &RawClient) -> Self {
        Self {
            address: c.address.clone(),
            class: c.class.clone(),
            title: c.title.clone(),
            initial_class: c.initial_class.clone(),
            initial_title: c.initial_title.clone(),
            workspace: c.workspace.clone(),
            monitor: c.monitor,
            floating: c.floating,
            fullscreen: c.fullscreen == 2,
            pinned: c.pinned,
            hidden: c.hidden,
            mapped: c.mapped,
            at: c.at,
            size: c.size,
            xwayland: c.xwayland,
            pid: c.pid,
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct Workspace {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    monitor: String,
    #[serde(default, rename(deserialize = "monitorID", serialize = "monitorId"))]
    monitor_id: i64,
    #[serde(default)]
    windows: i64,
    #[serde(
        default,
        rename(deserialize = "hasfullscreen", serialize = "hasFullscreen")
    )]
    has_fullscreen: bool,
    #[serde(default, rename(deserialize = "lastwindow", serialize = "lastWindow"))]
    last_window: String,
    #[serde(
        default,
        rename(deserialize = "ispersistent", serialize = "persistent")
    )]
    persistent: bool,
    #[serde(default, rename = "tiledLayout")]
    tiled_layout: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    id: i64,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    width: i64,
    #[serde(default)]
    height: i64,
    #[serde(default)]
    refresh_rate: f64,
    #[serde(default)]
    x: i64,
    #[serde(default)]
    y: i64,
    #[serde(default = "one")]
    scale: f64,
    #[serde(default)]
    transform: i64,
    #[serde(default)]
    active_workspace: WorkspaceRef,
    #[serde(default)]
    special_workspace: WorkspaceRef,
    #[serde(default)]
    reserved: [i64; 4],
    #[serde(default)]
    disabled: bool,
    #[serde(skip_serializing, default)]
    focused: bool,
}

fn one() -> f64 {
    1.0
}

impl Monitor {
    // Logical size, which is what layout coordinates are in. Odd transforms
    // are the quarter turns.
    fn logical(&self) -> (f64, f64) {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (w, h) = (self.width as f64 / scale, self.height as f64 / scale);
        if self.transform % 2 == 1 {
            (h, w)
        } else {
            (w, h)
        }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct Focus {
    monitor: String,
    workspace: i64,
    client: String,
    // Most recent first.
    history: Vec<String>,
}

#[derive(Default, Clone, Copy, Debug, PartialEq)]
struct Dirty {
    clients: bool,
    workspaces: bool,
    monitors: bool,
}

impl Dirty {
    const ALL: Dirty = Dirty {
        clients: true,
        workspaces: true,
        monitors: true,
    };

    fn any(self) -> bool {
        self.clients || self.workspaces || self.monitors
    }

    fn merge(&mut self, o: Dirty) {
        self.clients |= o.clients;
        self.workspaces |= o.workspaces;
        self.monitors |= o.monitors;
    }
}

#[derive(Debug, PartialEq)]
enum Event {
    Title { address: String, title: String },
    Dirty(Dirty),
    Ignore,
}

const fn d(clients: bool, workspaces: bool, monitors: bool) -> Event {
    Event::Dirty(Dirty {
        clients,
        workspaces,
        monitors,
    })
}

// What an event line can have changed. The payload is only trusted for titles,
// which can arrive many times a second; everything else is refetched, so a
// payload format change costs nothing. Names not listed here refetch it all.
fn classify(line: &str) -> Event {
    let (name, data) = line.split_once(">>").unwrap_or((line, ""));
    match name {
        "windowtitlev2" => match data.split_once(',') {
            Some((addr, title)) => Event::Title {
                address: format!("0x{}", addr.trim_start_matches("0x")),
                title: title.to_string(),
            },
            None => d(true, false, false),
        },
        "activewindowv2" | "changefloatingmode" | "pin" | "minimized" | "togglegroup"
        | "moveintogroup" | "moveoutofgroup" => d(true, false, false),
        "openwindow" | "closewindow" | "movewindowv2" | "fullscreen" => d(true, true, false),
        "workspacev2" | "focusedmonv2" | "createworkspacev2" | "destroyworkspacev2"
        | "moveworkspacev2" | "renameworkspace" | "activespecialv2" => d(false, true, true),
        "monitoraddedv2" | "monitorremovedv2" | "configreloaded" => Event::Dirty(Dirty::ALL),
        // v1 twins of the events above, sent alongside them.
        "windowtitle" | "activewindow" | "movewindow" | "workspace" | "focusedmon"
        | "createworkspace" | "destroyworkspace" | "moveworkspace" | "activespecial"
        | "monitoradded" | "monitorremoved" => Event::Ignore,
        "activelayout" | "submap" | "screencast" | "screencastv2" | "bell" | "urgent" | "openlayer"
        | "closelayer" | "lockgroups" | "ignoregrouplock" | "custom" => Event::Ignore,
        _ => {
            tracing::debug!(event = name, "unrecognised, refetching everything");
            Event::Dirty(Dirty::ALL)
        }
    }
}

// Items whose serialised form changed, and keys that are gone.
fn diff(old: &BTreeMap<String, Value>, new: &BTreeMap<String, Value>) -> Vec<Change> {
    let mut out: Vec<Change> = new
        .iter()
        .filter(|(k, v)| old.get(*k) != Some(*v))
        .map(|(k, v)| Change::Upsert(k.clone(), v.clone()))
        .collect();
    out.extend(
        old.keys()
            .filter(|k| !new.contains_key(*k))
            .map(|k| Change::Remove(k.clone())),
    );
    out
}

fn keyed<T: Serialize>(
    prefix: &str,
    items: impl Iterator<Item = (String, T)>,
) -> BTreeMap<String, Value> {
    items
        .filter_map(|(k, v)| Some((format!("{prefix}/{k}"), serde_json::to_value(v).ok()?)))
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
enum Coord {
    Percent(f64),
    Pixels(i64),
}

#[derive(Debug, Clone, PartialEq)]
enum Action {
    Workspace(String),
    Focus(String),
    Close(String),
    MoveSilent {
        workspace: String,
        address: Option<String>,
    },
    MoveExact {
        x: Coord,
        y: Coord,
        address: String,
    },
    ToggleSpecial(Option<String>),
}

fn address(target: &str) -> Option<String> {
    let addr = target.trim().strip_prefix("address:")?.trim();
    let hex = addr.strip_prefix("0x")?;
    (!hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| addr.to_string())
}

fn coord(s: &str) -> Option<Coord> {
    match s.strip_suffix('%') {
        Some(p) => p
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite())
            .map(Coord::Percent),
        None => s.parse().ok().map(Coord::Pixels),
    }
}

// Workspace selectors as Hyprland takes them: ids, r+1, e-1, name:foo, special:x.
fn selector(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && !s.chars().any(|c| c.is_control() || c == ',')).then(|| s.to_string())
}

// The legacy dispatch strings the shell has always sent. Anything else is
// refused: there is no fallback that runs it as a command.
fn parse(cmd: &str) -> Option<Action> {
    let cmd = cmd.trim();
    let (name, rest) = cmd.split_once(char::is_whitespace).unwrap_or((cmd, ""));
    let rest = rest.trim();
    match name {
        "workspace" => selector(rest).map(Action::Workspace),
        "focuswindow" => address(rest).map(Action::Focus),
        "closewindow" => address(rest).map(Action::Close),
        "movetoworkspacesilent" => {
            let (ws, target) = match rest.split_once(',') {
                Some((ws, t)) => (ws, Some(address(t)?)),
                None => (rest, None),
            };
            Some(Action::MoveSilent {
                workspace: selector(ws)?,
                address: target,
            })
        }
        "movewindowpixel" => {
            let (pos, target) = rest.split_once(',')?;
            let mut it = pos.split_whitespace();
            if it.next()? != "exact" {
                return None;
            }
            let (x, y) = (coord(it.next()?)?, coord(it.next()?)?);
            if it.next().is_some() {
                return None;
            }
            Some(Action::MoveExact {
                x,
                y,
                address: address(target)?,
            })
        }
        "togglespecialworkspace" => Some(Action::ToggleSpecial(
            (!rest.is_empty()).then(|| selector(rest)).flatten(),
        )),
        _ => None,
    }
}

fn lua_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn px(c: &Coord, origin: i64, span: f64) -> i64 {
    match c {
        Coord::Pixels(p) => *p,
        Coord::Percent(p) => origin + (span * p / 100.0).round() as i64,
    }
}

// Percentages are of the window's monitor, as legacy `exact` read them; Lua
// takes layout pixels only, so they are resolved here.
fn render(
    action: &Action,
    lua: bool,
    monitor_of: impl Fn(&str) -> Option<Monitor>,
) -> Result<String> {
    let target = |a: &str| format!("address:{a}");
    Ok(match (action, lua) {
        (Action::Workspace(ws), true) => format!("hl.dsp.focus({{ workspace = {} }})", lua_str(ws)),
        (Action::Workspace(ws), false) => format!("workspace {ws}"),
        // The shell focuses windows it draws elsewhere (the overview's previews);
        // a warp would throw the pointer off the thing being hovered or dragged.
        (Action::Focus(a), true) => format!(
            "function() local w = hl.get_config(\"cursor.no_warps\"); \
             hl.config({{ cursor = {{ no_warps = true }} }}); \
             hl.dispatch(hl.dsp.focus({{ window = {} }})); \
             hl.config({{ cursor = {{ no_warps = w }} }}) end",
            lua_str(&target(a))
        ),
        (Action::Focus(a), false) => format!("focuswindow {}", target(a)),
        (Action::Close(a), true) => {
            format!(
                "hl.dsp.window.close({{ window = {} }})",
                lua_str(&target(a))
            )
        }
        (Action::Close(a), false) => format!("closewindow {}", target(a)),
        (Action::MoveSilent { workspace, address }, true) => format!(
            "hl.dsp.window.move({{ workspace = {}, follow = false{} }})",
            lua_str(workspace),
            address
                .as_deref()
                .map(|a| format!(", window = {}", lua_str(&target(a))))
                .unwrap_or_default()
        ),
        (Action::MoveSilent { workspace, address }, false) => match address {
            Some(a) => format!("movetoworkspacesilent {workspace},{}", target(a)),
            None => format!("movetoworkspacesilent {workspace}"),
        },
        (Action::MoveExact { x, y, address }, lua) => {
            let (ox, oy, w, h) = match monitor_of(address) {
                Some(m) => {
                    let (w, h) = m.logical();
                    (m.x, m.y, w, h)
                }
                None if matches!((x, y), (Coord::Pixels(_), Coord::Pixels(_))) => (0, 0, 0.0, 0.0),
                None => bail!("no monitor for {address}"),
            };
            let (x, y) = (px(x, ox, w), px(y, oy, h));
            if lua {
                format!(
                    "hl.dsp.window.move({{ x = {x}, y = {y}, window = {} }})",
                    lua_str(&target(address))
                )
            } else {
                format!("movewindowpixel exact {x} {y},{}", target(address))
            }
        }
        (Action::ToggleSpecial(name), true) => match name {
            Some(n) => format!("hl.dsp.workspace.toggle_special({})", lua_str(n)),
            None => "hl.dsp.workspace.toggle_special()".to_string(),
        },
        (Action::ToggleSpecial(name), false) => match name {
            Some(n) => format!("togglespecialworkspace {n}"),
            None => "togglespecialworkspace".to_string(),
        },
    })
}

impl Action {
    fn address(&self) -> Option<&str> {
        match self {
            Action::Focus(a) | Action::Close(a) | Action::MoveExact { address: a, .. } => Some(a),
            Action::MoveSilent { address, .. } => address.as_deref(),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Cache {
    clients: BTreeMap<String, Value>,
    workspaces: BTreeMap<String, Value>,
    monitors: BTreeMap<String, Value>,
    focus: BTreeMap<String, Value>,
    monitor_list: Vec<Monitor>,
    client_monitor: BTreeMap<String, i64>,
}

pub struct Service {
    bus: Bus,
    cache: Cache,
}

impl Service {
    pub fn new(bus: Bus) -> Self {
        Self {
            bus,
            cache: Cache::default(),
        }
    }

    pub async fn run(mut self, mut requests: mpsc::Receiver<Request>) {
        loop {
            match hyprland::discover().await {
                Some(instance) => {
                    tracing::info!(instance = %instance.dir().display(), "hyprland");
                    if let Err(e) = self.session(&instance, &mut requests).await {
                        tracing::warn!("hyprland: {e:#}");
                    }
                }
                None => tracing::debug!("no hyprland instance"),
            }
            let deadline = tokio::time::sleep(RECONNECT);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    _ = &mut deadline => break,
                    req = requests.recv() => match req {
                        Some(r) => tracing::debug!(cmd = %r.cmd, "no compositor, dropped"),
                        None => return,
                    },
                }
            }
        }
    }

    async fn session(
        &mut self,
        hypr: &Instance,
        requests: &mut mpsc::Receiver<Request>,
    ) -> Result<()> {
        let lua = hyprland::speaks_lua(&hypr.json::<Value>("version").await?);
        let mut events = BufReader::new(hypr.events().await?).lines();
        // Everything, against whatever survived from the last instance.
        self.refresh(hypr, Dirty::ALL).await?;

        loop {
            let mut dirty = Dirty::default();
            let mut titles = Vec::new();
            tokio::select! {
                line = events.next_line() => {
                    let Some(line) = line? else { bail!("event socket closed") };
                    self.take(&line, &mut dirty, &mut titles);
                    // Drain what has already arrived, so a burst costs one refetch.
                    while let Some(next) = events.next_line().now_or_never() {
                        let Some(line) = next? else { bail!("event socket closed") };
                        self.take(&line, &mut dirty, &mut titles);
                    }
                }
                req = requests.recv() => {
                    let Some(req) = req else { return Ok(()) };
                    dirty = self.handle(hypr, lua, req).await;
                }
            }
            if !titles.is_empty() && !dirty.clients {
                self.patch_titles(titles);
            }
            if dirty.any() {
                self.refresh(hypr, dirty).await?;
            }
        }
    }

    fn take(&self, line: &str, dirty: &mut Dirty, titles: &mut Vec<(String, String)>) {
        match classify(line) {
            Event::Title { address, title } => titles.push((address, title)),
            Event::Dirty(d) => dirty.merge(d),
            Event::Ignore => {}
        }
    }

    fn patch_titles(&mut self, titles: Vec<(String, String)>) {
        let mut next = self.cache.clients.clone();
        for (address, title) in titles {
            if let Some(Value::Object(c)) = next.get_mut(&format!("client/{address}")) {
                c.insert("title".into(), Value::String(title));
            }
        }
        let changes = diff(&self.cache.clients, &next);
        self.cache.clients = next;
        self.bus.apply(SERVICE, &changes);
    }

    // The focus line is rebuilt every time: it is two small requests, and
    // it is the one thing every burst can touch.
    async fn refresh(&mut self, hypr: &Instance, dirty: Dirty) -> Result<()> {
        let (clients, workspaces, monitors, active_ws, active_win) = tokio::join!(
            async {
                match dirty.clients {
                    true => hypr.json::<Vec<RawClient>>("clients").await.map(Some),
                    false => Ok(None),
                }
            },
            async {
                match dirty.workspaces {
                    true => hypr.json::<Vec<Workspace>>("workspaces").await.map(Some),
                    false => Ok(None),
                }
            },
            async {
                match dirty.monitors {
                    true => hypr.json::<Vec<Monitor>>("monitors").await.map(Some),
                    false => Ok(None),
                }
            },
            hypr.json::<Value>("activeworkspace"),
            hypr.json::<Value>("activewindow"),
        );

        let mut changes = Vec::new();
        let mut history = None;
        if let Some(mut list) = clients? {
            list.sort_by_key(|c| c.focus_history_id);
            history = Some(list.iter().map(|c| c.address.clone()).collect::<Vec<_>>());
            self.cache.client_monitor = list
                .iter()
                .map(|c| (c.address.clone(), c.monitor))
                .collect();
            let next = keyed(
                "client",
                list.iter().map(|c| (c.address.clone(), Client::from(c))),
            );
            changes.extend(diff(&self.cache.clients, &next));
            self.cache.clients = next;
        }
        if let Some(list) = workspaces? {
            let next = keyed("workspace", list.into_iter().map(|w| (w.id.to_string(), w)));
            changes.extend(diff(&self.cache.workspaces, &next));
            self.cache.workspaces = next;
        }
        if let Some(list) = monitors? {
            let next = keyed("monitor", list.iter().map(|m| (m.name.clone(), m.clone())));
            changes.extend(diff(&self.cache.monitors, &next));
            self.cache.monitors = next;
            self.cache.monitor_list = list;
        }

        let previous = self.cache.focus.get("focus");
        let focus = Focus {
            monitor: self
                .cache
                .monitor_list
                .iter()
                .find(|m| m.focused)
                .map(|m| m.name.clone())
                .unwrap_or_default(),
            workspace: active_ws?["id"].as_i64().unwrap_or(0),
            client: active_win?["address"].as_str().unwrap_or("").to_string(),
            history: history.unwrap_or_else(|| {
                previous
                    .and_then(|f| serde_json::from_value(f["history"].clone()).ok())
                    .unwrap_or_default()
            }),
        };
        let next: BTreeMap<String, Value> = serde_json::to_value(focus)
            .ok()
            .map(|v| ("focus".to_string(), v))
            .into_iter()
            .collect();
        changes.extend(diff(&self.cache.focus, &next));
        self.cache.focus = next;

        self.bus.apply(SERVICE, &changes);
        Ok(())
    }

    // Whatever a dispatch moved shows up through events, except geometry,
    // which Hyprland never announces; the refetch after it is what keeps an
    // open overview honest.
    async fn handle(&mut self, hypr: &Instance, lua: bool, req: Request) -> Dirty {
        match req.cmd.as_str() {
            "refresh" => Dirty::ALL,
            "dispatch" => {
                let raw = req.args["cmd"].as_str().unwrap_or("");
                if let Err(e) = self.dispatch(hypr, lua, raw).await {
                    tracing::warn!(cmd = raw, "dispatch refused: {e:#}");
                }
                Dirty::ALL
            }
            other => {
                tracing::warn!(cmd = other, "unknown command");
                Dirty::default()
            }
        }
    }

    async fn dispatch(&self, hypr: &Instance, lua: bool, raw: &str) -> Result<()> {
        let action = parse(raw).ok_or_else(|| anyhow!("not a dispatch the shell sends"))?;
        // An address Hyprland cannot resolve may still answer "ok".
        if let Some(a) = action.address()
            && !self.cache.clients.contains_key(&format!("client/{a}"))
        {
            bail!("no window {a}");
        }
        let monitor_of = |a: &str| {
            let id = *self.cache.client_monitor.get(a)?;
            self.cache.monitor_list.iter().find(|m| m.id == id).cloned()
        };
        hypr.dispatch(&render(&action, lua, monitor_of)?).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn monitor(x: i64, w: i64, h: i64, scale: f64, transform: i64) -> Monitor {
        serde_json::from_value(json!({
            "id": 1, "name": "DP-1", "width": w, "height": h,
            "x": x, "y": 0, "scale": scale, "transform": transform
        }))
        .unwrap()
    }

    #[test]
    fn titles_are_patched_from_the_payload() {
        assert_eq!(
            classify("windowtitlev2>>5952cb9baea0,vim, a, b"),
            Event::Title {
                address: "0x5952cb9baea0".into(),
                title: "vim, a, b".into()
            }
        );
    }

    #[test]
    fn events_mark_what_they_can_change() {
        assert_eq!(classify("activewindowv2>>abc"), d(true, false, false));
        assert_eq!(classify("openwindow>>abc,1,kitty,t"), d(true, true, false));
        assert_eq!(classify("workspacev2>>2,2"), d(false, true, true));
        assert_eq!(
            classify("monitorremovedv2>>1,DP-1,desc"),
            Event::Dirty(Dirty::ALL)
        );
        assert_eq!(classify("workspace>>2"), Event::Ignore);
        assert_eq!(classify("activelayout>>kbd,us"), Event::Ignore);
    }

    #[test]
    fn unknown_events_refetch_everything() {
        assert_eq!(classify("somethingnew>>x"), Event::Dirty(Dirty::ALL));
        assert_eq!(classify(""), Event::Dirty(Dirty::ALL));
    }

    #[test]
    fn diff_sends_only_what_changed() {
        let old: BTreeMap<_, _> = [("a".into(), json!(1)), ("b".into(), json!(2))].into();
        let new: BTreeMap<_, _> = [("b".into(), json!(3)), ("c".into(), json!(4))].into();
        assert_eq!(
            diff(&old, &new),
            vec![
                Change::Upsert("b".into(), json!(3)),
                Change::Upsert("c".into(), json!(4)),
                Change::Remove("a".into()),
            ]
        );
        assert!(diff(&new, &new).is_empty());
    }

    #[test]
    fn clients_parse_hyprland_json() {
        let raw: Vec<RawClient> = serde_json::from_value(json!([{
            "address": "0xabc", "mapped": true, "hidden": false, "at": [10, 20],
            "size": [300, 200], "workspace": {"id": 3, "name": "3"}, "floating": true,
            "monitor": 1, "class": "kitty", "title": "t", "initialClass": "kitty",
            "initialTitle": "t", "pid": 42, "xwayland": false, "pinned": false,
            "fullscreen": 1, "fullscreenClient": 0, "focusHistoryID": 2,
            "grouped": [], "tags": [], "stableId": "1"
        }]))
        .unwrap();
        let c = Client::from(&raw[0]);
        assert_eq!(c.workspace.id, 3);
        assert_eq!(c.at, [10, 20]);
        assert!(!c.fullscreen, "maximised is not fullscreen");
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("focusHistoryID").is_none());
        assert_eq!(v["initialClass"], "kitty");
    }

    #[test]
    fn fullscreen_is_covering_the_screen() {
        for (mode, want) in [(0, false), (1, false), (2, true)] {
            let raw: RawClient =
                serde_json::from_value(json!({"address": "0x1", "fullscreen": mode})).unwrap();
            assert_eq!(Client::from(&raw).fullscreen, want);
        }
    }

    #[test]
    fn workspaces_and_monitors_parse_hyprland_json() {
        let w: Workspace = serde_json::from_value(json!({
            "id": 1, "name": "1", "monitor": "DP-3", "monitorID": 0, "windows": 2,
            "hasfullscreen": false, "lastwindow": "0x1", "lastwindowtitle": "x",
            "ispersistent": true, "tiledLayout": "dwindle"
        }))
        .unwrap();
        assert_eq!(w.windows, 2);
        let v = serde_json::to_value(&w).unwrap();
        assert_eq!(v["monitorId"], 0);
        assert_eq!(v["hasFullscreen"], false);

        let m: Monitor = serde_json::from_value(json!({
            "id": 0, "name": "DP-3", "width": 1920, "height": 1080, "refreshRate": 165.0,
            "x": 0, "y": 0, "activeWorkspace": {"id": 1, "name": "1"},
            "specialWorkspace": {"id": 0, "name": ""}, "reserved": [0, 0, 0, 0],
            "scale": 1, "transform": 0, "focused": true, "availableModes": []
        }))
        .unwrap();
        assert!(m.focused);
        assert!(serde_json::to_value(&m).unwrap().get("focused").is_none());
    }

    #[test]
    fn logical_size_follows_scale_and_rotation() {
        assert_eq!(monitor(0, 2560, 1440, 1.25, 0).logical(), (2048.0, 1152.0));
        assert_eq!(monitor(0, 1920, 1080, 1.0, 1).logical(), (1080.0, 1920.0));
    }

    #[test]
    fn parses_what_the_shell_sends() {
        assert_eq!(parse("workspace 3"), Some(Action::Workspace("3".into())));
        assert_eq!(
            parse("workspace r+1"),
            Some(Action::Workspace("r+1".into()))
        );
        assert_eq!(
            parse("focuswindow address:0xAbC1"),
            Some(Action::Focus("0xAbC1".into()))
        );
        assert_eq!(
            parse("movetoworkspacesilent 4, address:0xabc"),
            Some(Action::MoveSilent {
                workspace: "4".into(),
                address: Some("0xabc".into())
            })
        );
        assert_eq!(
            parse("movewindowpixel exact 50% 25%, address:0xabc"),
            Some(Action::MoveExact {
                x: Coord::Percent(50.0),
                y: Coord::Percent(25.0),
                address: "0xabc".into()
            })
        );
        assert_eq!(
            parse("togglespecialworkspace"),
            Some(Action::ToggleSpecial(None))
        );
    }

    #[test]
    fn refuses_anything_else() {
        assert_eq!(parse("exec rm -rf ~"), None);
        assert_eq!(parse("focuswindow address:0xzz"), None);
        assert_eq!(parse("focuswindow class:kitty"), None);
        assert_eq!(parse("closewindow"), None);
        assert_eq!(parse("workspace "), None);
        assert_eq!(parse("workspace 1,2"), None);
        assert_eq!(parse("movewindowpixel 10 10, address:0xabc"), None);
        assert_eq!(parse("movewindowpixel exact 1 2 3, address:0xabc"), None);
        assert_eq!(parse("movewindowpixel exact NaN% 2, address:0xabc"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn renders_lua_and_legacy() {
        let none = |_: &str| None;
        let ws = Action::Workspace("r+1".into());
        assert_eq!(
            render(&ws, true, none).unwrap(),
            r#"hl.dsp.focus({ workspace = "r+1" })"#
        );
        assert_eq!(render(&ws, false, none).unwrap(), "workspace r+1");
        let mv = parse("movetoworkspacesilent 4, address:0xabc").unwrap();
        assert_eq!(
            render(&mv, true, none).unwrap(),
            r#"hl.dsp.window.move({ workspace = "4", follow = false, window = "address:0xabc" })"#
        );
        assert_eq!(
            render(&mv, false, none).unwrap(),
            "movetoworkspacesilent 4,address:0xabc"
        );
        let close = Action::Close("0xabc".into());
        assert_eq!(
            render(&close, true, none).unwrap(),
            r#"hl.dsp.window.close({ window = "address:0xabc" })"#
        );
    }

    #[test]
    fn focus_never_warps_the_pointer() {
        let out = render(&Action::Focus("0xabc".into()), true, |_: &str| None).unwrap();
        assert!(out.starts_with("function() local w = hl.get_config(\"cursor.no_warps\");"));
        assert!(out.contains(r#"hl.dispatch(hl.dsp.focus({ window = "address:0xabc" }))"#));
        assert!(out.ends_with("hl.config({ cursor = { no_warps = w } }) end"));
    }

    #[test]
    fn percentages_resolve_against_the_window_monitor() {
        let mv = parse("movewindowpixel exact 50% 25%, address:0xabc").unwrap();
        let second = |_: &str| Some(monitor(1920, 2560, 1440, 1.25, 0));
        assert_eq!(
            render(&mv, true, second).unwrap(),
            r#"hl.dsp.window.move({ x = 2944, y = 288, window = "address:0xabc" })"#
        );
        assert_eq!(
            render(&mv, false, second).unwrap(),
            "movewindowpixel exact 2944 288,address:0xabc"
        );
        assert!(render(&mv, true, |_: &str| None).is_err());
    }

    #[test]
    fn lua_strings_cannot_escape() {
        assert_eq!(lua_str(r#"a"b\c"#), r#""a\"b\\c""#);
        let ws = Action::Workspace(r#"name:x") hl.dsp.exit() --"#.into());
        let out = render(&ws, true, |_: &str| None).unwrap();
        assert_eq!(
            out,
            r#"hl.dsp.focus({ workspace = "name:x\") hl.dsp.exit() --" })"#
        );
    }

    // Against the running compositor, read-only: cargo test live -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn live() {
        let bus = Bus::new();
        let mut rx = bus.subscribe();
        let (_tx, requests) = mpsc::channel(1);
        let task = tokio::spawn(Service::new(bus.clone()).run(requests));
        let start = std::time::Instant::now();
        let mut first = None;
        while start.elapsed() < Duration::from_secs(8) {
            if let Ok(Ok(msg)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
                let lines = msg.lines().count();
                let at = start.elapsed();
                first.get_or_insert(at);
                println!("{:>6.1?} burst: {lines} lines, {} bytes", at, msg.len());
                for l in msg.lines() {
                    let v: Value = serde_json::from_str(l).unwrap();
                    println!("         {} {}", v["event"], v["key"].as_str().unwrap_or(""));
                }
            }
        }
        task.abort();
        assert!(first.is_some(), "nothing published");
    }
}