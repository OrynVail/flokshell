pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io
import qs.config
import qs.modules.globals
pragma ComponentBehavior: Bound

Singleton {
    id: root

    // CPU metrics
    property real cpuUsage: 0.0
    property string cpuModel: ""
    property int cpuTemp: -1

    // RAM metrics, in KiB
    property real ramUsage: 0.0
    property real ramTotal: 0
    property real ramUsed: 0
    property real ramAvailable: 0

    // GPU metrics
    property var gpuUsages: []
    property var gpuVendors: []
    property var gpuNames: []
    property int gpuCount: 0
    property bool gpuDetected: false
    property var gpuTemps: []

    // Legacy single GPU properties
    property real gpuUsage: gpuUsages.length > 0 ? gpuUsages[0] : 0.0
    property string gpuVendor: gpuVendors.length > 0 ? gpuVendors[0] : "unknown"
    property int gpuTemp: gpuTemps.length > 0 ? gpuTemps[0] : -1

    // Disk metrics
    property var diskUsage: ({})
    property var diskTypes: ({})
    property var validDisks: []

    // History data
    property var cpuHistory: []
    property var ramHistory: []
    property var gpuHistories: []
    property var cpuTempHistory: []
    property var gpuTempHistories: []
    property int maxHistoryPoints: 50
    property int totalDataPoints: 0

    property int updateInterval: 2000

    readonly property bool watching: GlobalStates.dashboardOpen && GlobalStates.dashboardCurrentTab === 2 && validDisks.length > 0
    property real lastSeq: -1

    onWatchingChanged: watching ? watch() : send("stop")
    onValidDisksChanged: if (watching) watch()
    onUpdateIntervalChanged: if (watching) watch()

    FlokdSocket {
        id: flokd
        service: "resources"
        onMessage: d => root.apply(d)
        onConnectionStateChanged: {
            if (connected && root.watching)
                root.watch();
        }
    }

    function send(cmd, args) {
        flokd.send(cmd, args);
    }

    function watch() {
        send("watch", {
            interval_ms: root.updateInterval,
            disks: root.validDisks
        });
    }

    function apply(d) {
        root.cpuModel = d.cpu_model;
        root.cpuUsage = d.cpu_usage;
        root.cpuTemp = d.cpu_temp_c;

        root.ramTotal = d.ram_total / 1024;
        root.ramUsed = d.ram_used / 1024;
        root.ramAvailable = d.ram_available / 1024;
        root.ramUsage = d.ram_total > 0 ? d.ram_used * 100 / d.ram_total : 0;

        const gpus = d.gpus || [];
        root.gpuNames = gpus.map(g => g.name);
        root.gpuVendors = gpus.map(g => g.vendor);
        root.gpuUsages = gpus.map(g => g.usage);
        root.gpuTemps = gpus.map(g => g.temp_c);
        root.gpuCount = gpus.length;
        root.gpuDetected = gpus.length > 0;

        let usage = {};
        let types = {};
        for (const disk of d.disks || []) {
            usage[disk.mount] = disk.usage;
            types[disk.mount] = disk.kind;
        }
        root.diskUsage = usage;
        root.diskTypes = types;

        const fresh = root.lastSeq >= 0 && d.seq !== root.lastSeq;
        root.lastSeq = d.seq;
        if (fresh && root.watching) {
            root.updateHistory();
            root.watch();
        }
    }

    Component.onCompleted: validateDisks()

    Connections {
        target: Config.system
        function onDisksChanged() { root.validateDisks(); }
    }

    property bool configReady: Config.initialLoadComplete
    onConfigReadyChanged: if (configReady) validateDisks()

    function validateDisks() {
        const configuredDisks = Config.system.disks || ["/"];
        let newValidDisks = [];
        for (let i = 0; i < configuredDisks.length; i++) {
            const disk = configuredDisks[i];
            if (disk && typeof disk === 'string' && disk.trim() !== '') {
                newValidDisks.push(disk.trim());
            }
        }
        if (newValidDisks.length === 0) newValidDisks = ["/"];
        validDisks = newValidDisks;
    }

    function updateHistory() {
        totalDataPoints++;

        const pushHistory = (arr, val) => {
            let next = arr.slice();
            next.push(val);
            if (next.length > maxHistoryPoints) next.shift();
            return next;
        };

        cpuHistory = pushHistory(cpuHistory, cpuUsage / 100);
        cpuTempHistory = pushHistory(cpuTempHistory, cpuTemp);
        ramHistory = pushHistory(ramHistory, ramUsage / 100);

        if (gpuDetected && gpuCount > 0) {
            let newGpuHistories = gpuHistories.slice();
            let newGpuTempHistories = gpuTempHistories.slice();

            while (newGpuHistories.length < gpuCount) newGpuHistories.push([]);
            while (newGpuTempHistories.length < gpuCount) newGpuTempHistories.push([]);

            for (let i = 0; i < gpuCount; i++) {
                newGpuHistories[i] = pushHistory(newGpuHistories[i], (gpuUsages[i] || 0) / 100);
                newGpuTempHistories[i] = pushHistory(newGpuTempHistories[i], (gpuTemps[i] ?? -1));
            }

            gpuHistories = newGpuHistories;
            gpuTempHistories = newGpuTempHistories;
        }
    }
}
