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

    property var _activeWorkspaceIds: []
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
        monitorsProcess.running = true;
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

    // Process for fetching monitors
    property Process monitorsProcess: Process {
        id: monitorsProcess
        command: ["axctl", "monitor", "list"]
        stdout: StdioCollector {}
        onExited: exitCode => {
            if (exitCode === 0) {
                try {
                    var rawMonitors = JSON.parse(monitorsProcess.stdout.text)
                    var normalized = rawMonitors.map(m => ({
                        id: m.id,
                        name: m.name,
                        width: m.width,
                        height: m.height,
                        scale: m.scale,
                        refresh_rate: m.refresh_rate,
                        focused: m.is_focused,
                        x: m.metadata ? m.metadata.x : 0,
                        y: m.metadata ? m.metadata.y : 0,
                        transform: m.metadata ? m.metadata.transform : 0,
                        activeWorkspace: m.metadata ? { id: m.metadata.active_workspace } : null
                    }))
                    root.monitors = normalized;
                    var ids = []
                    for (var i = 0; i < normalized.length; i++) {
                        if (normalized[i].activeWorkspace) {
                            ids.push(normalized[i].activeWorkspace.id)
                        }
                    }
                    root._activeWorkspaceIds = ids
                    clientsProcess.running = true

                    root.monitorsListReady(normalized)
                } catch (e) {
                    console.warn("Screenshot: Failed to parse monitors: " + e.message)
                    root.errorOccurred("Failed to parse monitors")
                }
            } else {
                console.warn("Screenshot: Failed to fetch monitors")
                root.errorOccurred("Failed to fetch monitors")
            }
        }
    }

    // Process for fetching windows
    property Process clientsProcess: Process {
        id: clientsProcess
        command: ["axctl", "window", "list"]
        stdout: StdioCollector {}
        onExited: exitCode => {
            if (exitCode === 0) {
                try {
                    var allClients = JSON.parse(clientsProcess.stdout.text)
                    var activeIds = root._activeWorkspaceIds
                    var normalizedClients = allClients.map(c => ({
                        id: c.id,
                        app_id: c.app_id,
                        title: c.title,
                        is_floating: c.is_floating,
                        is_focused: c.is_focused,
                        is_fullscreen: c.is_fullscreen,
                        is_hidden: c.is_hidden,
                        workspace_id: c.workspace_id,
                        pinned: c.metadata ? c.metadata.pinned : false,
                        workspace: { id: c.workspace_id },
                        at: [c.metadata ? c.metadata.x : 0, c.metadata ? c.metadata.y : 0],
                        size: [c.metadata ? c.metadata.width : 0, c.metadata ? c.metadata.height : 0]
                    }))
                    
                    var filteredClients = normalizedClients.filter(c => {
                        return c.pinned || (activeIds.length > 0 && activeIds.includes(c.workspace.id))
                    })
                    root.windowListReady(filteredClients)
                } catch (e) {
                    console.warn("Screenshot: Error processing windows: " + e.message)
                }
            }
        }
    }

    property Process openScreenshotsProcess: Process {
        id: openScreenshotsProcess
        command: ["sh", "-c", "mkdir -p \"$0\" && exec xdg-open \"$0\"", root.screenshotsDir]
    }

    function openScreenshotsFolder() {
        openScreenshotsProcess.running = true;
    }
}
