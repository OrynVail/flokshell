pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io

Singleton {
    id: root

    property string usageFilePath: Quickshell.env("HOME") + "/.cache/flokshell/usage.json"

    // { appId: { count, lastUsed } }
    property var usageData: ({})
    property bool dataLoaded: false

    signal usageDataReady

    readonly property int maxBoostScore: 200
    readonly property int dayInMs: 86400000

    FileView {
        id: usageFile
        path: root.usageFilePath
        onLoaded: root.loadUsageData()
        onLoadFailed: error => {
            if (error === FileViewError.FileNotFound)
                setText("{}");
            root.ready({});
        }
    }

    function ready(data) {
        root.usageData = data;
        root.dataLoaded = true;
        root.usageDataReady();
    }

    function loadUsageData() {
        try {
            const text = usageFile.text();
            ready(text && text.trim() !== "" ? JSON.parse(text) : {});
        } catch (e) {
            console.warn("UsageTracker: Failed to parse usage.json:", e);
            ready({});
        }
    }

    function recordUsage(appId) {
        if (!appId)
            return;

        const now = Date.now();
        if (usageData[appId]) {
            usageData[appId].count++;
            usageData[appId].lastUsed = now;
        } else {
            usageData[appId] = {
                count: 1,
                lastUsed: now
            };
        }
        usageData = usageData;
        usageFile.setText(JSON.stringify(usageData, null, 2));
    }

    // Recency boost decaying over a week, plus log-scaled frequency.
    function getUsageScore(appId) {
        if (!appId || !usageData[appId])
            return 0;

        const data = usageData[appId];
        const days = (Date.now() - data.lastUsed) / dayInMs;
        return maxBoostScore * Math.exp(-days / 7) + Math.log(data.count + 1) * 20;
    }
}
