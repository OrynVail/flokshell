pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Io
import qs.modules.globals

Singleton {
    id: root

    property var focusedMonitor: null
    property var focusedWorkspace: null
    property var focusedClient: null

    property QtObject clients: QtObject {
        property var values: []
    }

    property QtObject monitors: QtObject {
        property var values: []
    }

    property QtObject workspaces: QtObject {
        property var values: []
    }

    property string configPath: (Quickshell.env("XDG_DATA_HOME") || (Quickshell.env("HOME") + "/.local/share")) + "/flokshell/axctl.toml"

    function dispatch(command) {
        if (command)
            flokd.send("dispatch", { cmd: command });
    }

    function monitorFor(screen) {
        if (!screen) return null;
        let screenName = screen.name || screen;
        let values = root.monitors.values || [];
        for (let i = 0; i < values.length; i++) {
            if (values[i].name === screenName) return values[i];
        }
        return null;
    }

    // Raw items by key as flokd sends them; a burst lands here and is only
    // turned into the shapes below on commit.
    property var raw: ({ client: {}, workspace: {}, monitor: {} })
    property var focus: ({ monitor: "", workspace: 0, client: "", history: [] })
    property var touched: ({ client: {}, workspace: {}, monitor: {}, focus: false })
    // Built objects, reused while their raw item and focus are unchanged, so a
    // burst only replaces what it touched.
    property var built: ({ client: {}, workspace: {}, monitor: {} })

    function take(msg) {
        if (msg.event === "reset") {
            raw = { client: {}, workspace: {}, monitor: {} };
            built = { client: {}, workspace: {}, monitor: {} };
            touched = { client: {}, workspace: {}, monitor: {}, focus: true };
        } else if (msg.event === "upsert" || msg.event === "remove") {
            if (msg.key === "focus") {
                if (msg.data) focus = msg.data;
                touched.focus = true;
                return;
            }
            let slash = msg.key.indexOf("/");
            let kind = msg.key.substring(0, slash);
            let id = msg.key.substring(slash + 1);
            if (!raw[kind]) return;
            if (msg.event === "upsert")
                raw[kind][id] = msg.data;
            else
                delete raw[kind][id];
            touched[kind][id] = true;
        } else if (msg.event === "commit") {
            commit();
        }
    }

    function commit() {
        let changed = { client: false, workspace: false, monitor: false };
        for (let kind of ["client", "workspace", "monitor"]) {
            for (let id in built[kind]) {
                if (!raw[kind][id]) {
                    delete built[kind][id];
                    changed[kind] = true;
                }
            }
        }

        let rank = {};
        (focus.history || []).forEach((a, i) => rank[a] = i);
        for (let id in raw.client) {
            let c = raw.client[id];
            let prev = built.client[id];
            let isFocused = c.address === focus.client;
            let historyId = rank[c.address] !== undefined ? rank[c.address] : 999999;
            if (!touched.client[id] && prev && prev.is_focused === isFocused && prev.focusHistoryID === historyId)
                continue;
            built.client[id] = {
                address: c.address,
                class: c.class,
                title: c.title,
                initialClass: c.initialClass,
                initialTitle: c.initialTitle,
                workspace: c.workspace,
                monitor: c.monitor,
                floating: c.floating,
                fullscreen: c.fullscreen,
                pinned: c.pinned,
                hidden: c.hidden,
                mapped: c.mapped,
                at: c.at,
                size: c.size,
                xwayland: c.xwayland,
                pid: c.pid,
                is_focused: isFocused,
                focusHistoryID: historyId
            };
            changed.client = true;
        }

        for (let id in raw.workspace) {
            let w = raw.workspace[id];
            let prev = built.workspace[id];
            let active = w.id === focus.workspace;
            if (!touched.workspace[id] && prev && prev.active === active)
                continue;
            built.workspace[id] = {
                id: w.id,
                name: w.name,
                monitor: w.monitor,
                monitorId: w.monitorId,
                active: active,
                windows: w.windows,
                hasFullscreen: w.hasFullscreen,
                lastWindow: w.lastWindow,
                tiledLayout: w.tiledLayout
            };
            changed.workspace = true;
        }

        for (let id in raw.monitor) {
            let m = raw.monitor[id];
            let prev = built.monitor[id];
            let isFocused = m.name === focus.monitor;
            if (!touched.monitor[id] && prev && prev.focused === isFocused)
                continue;
            built.monitor[id] = {
                id: m.id,
                name: m.name,
                description: m.description,
                focused: isFocused,
                width: m.width,
                height: m.height,
                refreshRate: m.refreshRate,
                scale: m.scale,
                x: m.x,
                y: m.y,
                transform: m.transform,
                reserved: m.reserved,
                activeWorkspace: m.activeWorkspace,
                specialWorkspace: m.specialWorkspace
            };
            changed.monitor = true;
        }

        if (changed.client)
            clients.values = Object.values(built.client);
        if (changed.workspace)
            workspaces.values = Object.values(built.workspace).sort((a, b) => a.id - b.id);
        if (changed.monitor)
            monitors.values = Object.values(built.monitor).sort((a, b) => a.id - b.id);

        let fc = built.client[focus.client] || null;
        if (fc !== focusedClient) focusedClient = fc;
        let fw = built.workspace[String(focus.workspace)] || null;
        if (fw !== focusedWorkspace) focusedWorkspace = fw;
        let fm = built.monitor[focus.monitor] || null;
        if (fm !== focusedMonitor) focusedMonitor = fm;

        touched = { client: {}, workspace: {}, monitor: {}, focus: false };
    }

    FlokdSocket {
        id: flokd
        service: "compositor"
        onFrame: msg => root.take(msg)
    }

    // Hyprland announces no geometry changes, and the overview is what reads
    // window positions and sizes; fetch them fresh as it opens.
    Connections {
        target: GlobalStates
        function onOverviewOpenChanged() {
            if (GlobalStates.overviewOpen)
                flokd.send("refresh");
        }
    }

    property Process ensureConfigDir: Process {
        command: ["mkdir", "-p", (Quickshell.env("XDG_DATA_HOME") || (Quickshell.env("HOME") + "/.local/share")) + "/flokshell"]
        running: true
    }

    // Only the config writers and the CLI callers still need it.
    property Process axctlProcess: Process {
        command: ["axctl", "-c", root.configPath, "daemon"]
        running: true
        onExited: code => console.warn("axctl daemon exited with code:", code)
    }

    Component.onDestruction: axctlProcess.running = false
}
