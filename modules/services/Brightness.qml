pragma Singleton
pragma ComponentBehavior: Bound

import Quickshell
import Quickshell.Io
import qs.modules.services
import QtQuick

Singleton {
    id: root

    signal brightnessChanged(real value, var screen)

    // flokd's view, by output name: { kind, value, max }
    property var controls: ({})
    readonly property list<BrightnessMonitor> monitors: Quickshell.screens.map(screen => monitorComp.createObject(root, {
            screen
        }))

    property bool syncBrightness: StateService.get("syncBrightness", false)

    onSyncBrightnessChanged: {
        if (StateService.initialized) {
            StateService.set("syncBrightness", syncBrightness);
        }
    }

    Connections {
        target: StateService
        function onStateLoaded() {
            root.syncBrightness = StateService.get("syncBrightness", false);
        }
    }

    function isInternalScreen(screen: ShellScreen): bool {
        if (!screen || !screen.name)
            return false;
        const lower = screen.name.toLowerCase();
        return lower.includes("edp") || lower.includes("lvds") || lower.includes("dsi");
    }

    function getMonitorForScreen(screen: ShellScreen): var {
        return monitors.find(m => m.screen === screen);
    }

    function increaseBrightness(): void {
        const focusedName = AxctlService.focusedMonitor.name;
        const monitor = monitors.find(m => focusedName === m.screen.name);
        if (monitor)
            monitor.setBrightness(monitor.brightness + 0.05);
    }

    function decreaseBrightness(): void {
        const focusedName = AxctlService.focusedMonitor.name;
        const monitor = monitors.find(m => focusedName === m.screen.name);
        if (monitor)
            monitor.setBrightness(monitor.brightness - 0.05);
    }

    reloadableId: "brightness"

    // flokd finds outputs itself but only hears of hotplug from here.
    onMonitorsChanged: send("rescan")

    FlokdSocket {
        id: flokd
        service: "brightness"
        onMessage: d => root.apply(d)
    }

    function send(cmd, args) {
        flokd.send(cmd, args);
    }

    function apply(d) {
        let next = {};
        for (const m of d.monitors || [])
            next[m.name] = m;
        root.controls = next;
    }

    component BrightnessMonitor: QtObject {
        id: monitor

        required property ShellScreen screen
        readonly property var control: root.controls[screen?.name ?? ""] ?? null
        readonly property bool useBrightnessctl: control?.kind === "backlight"
        readonly property bool isDdc: control?.kind === "ddc"
        readonly property int rawMaxBrightness: control?.max ?? 100
        property real brightness
        property bool ready: false

        // Echoes of a drag lag the slider and must not pull it back.
        property real heldUntil: 0

        onControlChanged: {
            if (!control) {
                ready = false;
                return;
            }
            const value = control.value / control.max;
            if (!ready) {
                brightness = value;
                ready = true;
            } else if (Date.now() >= heldUntil && Math.round(brightness * control.max) !== control.value) {
                brightness = value;
            }
        }

        onBrightnessChanged: {
            if (monitor.ready)
                root.brightnessChanged(monitor.brightness, monitor.screen);
        }

        function setBrightness(value: real): void {
            if (!control)
                return;
            value = Math.max(0.01, Math.min(1, value));
            heldUntil = Date.now() + 500;
            brightness = value;
            root.send("set", {
                name: screen.name,
                value: Math.round(value * control.max)
            });
        }
    }

    Component {
        id: monitorComp

        BrightnessMonitor {}
    }

    IpcHandler {
        target: "brightness"

        function increment() {
            root.increaseBrightness();
        }

        function decrement() {
            root.decreaseBrightness();
        }

        function list(): string {
            return root.monitors.filter(m => m.ready).map(m => `${m.screen.name}:${Math.round(m.brightness * 100)}`).join("\n");
        }

        function set(value: real, monitorName: string) {
            if (!monitorName || monitorName === "") {
                for (let i = 0; i < root.monitors.length; ++i) {
                    const mon = root.monitors[i];
                    if (mon && mon.ready)
                        mon.setBrightness(value);
                }
            } else {
                const monitor = root.monitors.find(m => m.screen.name === monitorName);
                if (monitor && monitor.ready)
                    monitor.setBrightness(value);
                else
                    console.warn("Monitor not found or not ready:", monitorName);
            }
        }

        function adjust(delta: real, monitorName: string) {
            if (!monitorName || monitorName === "") {
                for (let i = 0; i < root.monitors.length; ++i) {
                    const mon = root.monitors[i];
                    if (mon && mon.ready)
                        mon.setBrightness(mon.brightness + delta);
                }
            } else {
                const monitor = root.monitors.find(m => m.screen.name === monitorName);
                if (monitor && monitor.ready)
                    monitor.setBrightness(monitor.brightness + delta);
                else
                    console.warn("Monitor not found or not ready:", monitorName);
            }
        }
    }
}
