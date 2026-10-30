pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Io
import qs.modules.globals

QtObject {
    id: root

    signal monitorScreenshotReady(string monitorName, string path)
    signal errorOccurred(string message)
    signal windowListReady(var windows)
    signal monitorsListReady(var monitors)
    signal imageSaved(string path)

    property string captureMode: "normal"
    property string screenshotsDir: ""
    property string finalPath: ""

    property var monitors: []
    property var frozen: []

    // Monitor the last capture belongs to; the preview overlay routes on it
    property string lastCaptureMonitor: ""

    property int selectionX: 0
    property int selectionY: 0
    property int selectionW: 0
    property int selectionH: 0

    property var _ocrLangs: ""

    // flokd keeps the frozen frames in memory, so a crop never re-reads them.
    property FlokdSocket flokd: FlokdSocket {
        service: "capture"
        property real seq: -1
        property string outcome: ""
        onMessage: d => {
            root.screenshotsDir = d.dir;
            const key = JSON.stringify(d.last);
            // The first frame is a replay of whatever happened before we connected.
            if (seq < 0) {
                seq = d.seq;
                outcome = key;
                return;
            }
            if (d.seq !== seq) {
                seq = d.seq;
                root.frozen = d.outputs;
                for (const o of d.outputs)
                    root.monitorScreenshotReady(o.name, o.path);
            }
            if (key === outcome)
                return;
            outcome = key;
            if (!d.last.ok)
                root.errorOccurred(d.last.error);
            else if (d.last.mode === "save") {
                root.finalPath = d.last.path;
                root.imageSaved(d.last.path);
            }
        }
    }

    function initialize() {}

    function freezeScreen() {
        flokd.send("freeze");
        root.fetchWindows();
    }

    function fetchWindows() {
        CompositorService.refresh(() => {
            const monitors = (CompositorService.monitors.values || []).map(m => ({
                id: m.id,
                name: m.name,
                width: m.width,
                height: m.height,
                scale: m.scale,
                refresh_rate: m.refreshRate,
                focused: m.focused,
                x: m.x,
                y: m.y,
                transform: m.transform,
                activeWorkspace: m.activeWorkspace
            }));
            root.monitors = monitors;
            root.monitorsListReady(monitors);
            const visible = monitors.map(m => m.activeWorkspace ? m.activeWorkspace.id : null);
            root.windowListReady((CompositorService.clients.values || []).filter(c => c.pinned || visible.includes(c.workspace.id)).map(c => ({
                id: c.address,
                app_id: c.class,
                title: c.title,
                is_floating: c.floating,
                is_focused: c.is_focused,
                is_fullscreen: c.fullscreen,
                is_hidden: c.hidden,
                workspace_id: c.workspace.id,
                pinned: c.pinned,
                workspace: { id: c.workspace.id },
                at: c.at,
                size: c.size
            })));
        });
    }

    function outputAt(x, y) {
        return root.frozen.find(o => x >= o.x && x < o.x + o.w && y >= o.y && y < o.y + o.h);
    }

    function act(args) {
        flokd.send("act", args);
        release();
    }

    function release() {
        flokd.send("release");
        root.captureMode = "normal";
    }

    function purpose() {
        return root.captureMode === "normal" ? "save" : root.captureMode;
    }

    function processRegion(x, y, w, h) {
        const o = outputAt(x, y);
        root.lastCaptureMonitor = o ? o.name : "";
        act({
            mode: purpose(),
            x: Math.round(x),
            y: Math.round(y),
            w: Math.round(w),
            h: Math.round(h),
            langs: root._ocrLangs
        });
    }

    function processMonitorScreen(monitorName) {
        root.lastCaptureMonitor = monitorName;
        act({
            mode: purpose(),
            output: monitorName,
            langs: root._ocrLangs
        });
    }

    function pickColor(x, y) {
        act({
            mode: "color",
            x: Math.round(x),
            y: Math.round(y)
        });
    }

    function startOcr(langs) {
        root._ocrLangs = langs;
        root.captureMode = "ocr";
        GlobalStates.screenshotToolVisible = true;
    }

    property Process openScreenshotsProcess: Process {
        id: openScreenshotsProcess
        command: ["sh", "-c", "mkdir -p \"$0\" && exec xdg-open \"$0\"", root.screenshotsDir]
    }

    function openScreenshotsFolder() {
        openScreenshotsProcess.running = true;
    }
}
