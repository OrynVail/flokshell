pragma Singleton
pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import Quickshell.Io

Singleton {
    id: root

    property bool wifi: false
    property bool ethernet: false

    property bool wifiEnabled: false
    property bool wifiScanning: false
    property bool scanQueued: false
    readonly property bool scanBusy: wifiScanning || scanQueued

    property WifiAccessPoint wifiConnectTarget: null
    readonly property bool isUpdating: wifiConnectTarget !== null
    readonly property bool wifiConnecting: isUpdating || wifiStatus === "connecting"
    property bool wasEnabledBeforeSleep: false

    property var suspendConnections: Connections {
        target: SuspendManager
        function onPreparingForSleep() {
            root.wasEnabledBeforeSleep = root.wifiEnabled;
        }
        function onWakingUp() {
            if (root.wasEnabledBeforeSleep) {
                root.enableWifi(true);
            }
        }
    }

    readonly property list<WifiAccessPoint> wifiNetworks: []
    property WifiAccessPoint active: null

    property string wifiStatus: "disconnected"
    property string networkName: ""
    property int networkStrength: 0

    property list<var> friendlyWifiNetworks: []

    property string lastNeedsPassword: ""

    // flokd owns NetworkManager; each message is the whole network state.
    readonly property string socketPath: (Quickshell.env("XDG_RUNTIME_DIR") || "/run/user/1000") + "/flokd.sock"

    Socket {
        id: flokd
        path: root.socketPath
        connected: true

        parser: SplitParser {
            splitMarker: "\n"
            onRead: data => {
                if (!data)
                    return;
                try {
                    let msg = JSON.parse(data);
                    if (msg.service === "network")
                        root.apply(msg.data);
                } catch (e) {
                    console.warn("NetworkService: bad frame:", e);
                }
            }
        }

        onConnectionStateChanged: {
            if (!connected)
                reconnect.start();
        }
    }

    Timer {
        id: reconnect
        interval: 2000
        repeat: false
        onTriggered: flokd.connected = true
    }

    function send(cmd, args) {
        let msg = {
            service: "network",
            cmd: cmd
        };
        if (args !== undefined)
            msg.args = args;
        flokd.write(JSON.stringify(msg) + "\n");
        flokd.flush();
    }

    function apply(d) {
        root.wifiEnabled = d.enabled;
        root.wifi = d.wifi;
        root.ethernet = d.ethernet;
        root.wifiStatus = d.status;
        root.networkName = d.name;
        root.networkStrength = d.strength;
        root.wifiScanning = d.scanning;
        root.scanQueued = d.scan_queued;

        // flokd sends one row per SSID, so the SSID is the identity.
        const incoming = d.networks || [];
        const rNetworks = root.wifiNetworks;
        for (let i = rNetworks.length - 1; i >= 0; i--) {
            const rn = rNetworks[i];
            if (!incoming.some(n => n.ssid === rn.ssid)) {
                rNetworks.splice(i, 1);
                rn.destroy();
            }
        }
        for (const n of incoming) {
            const existing = rNetworks.find(rn => rn.ssid === n.ssid);
            if (existing)
                existing.lastIpcObject = n;
            else
                rNetworks.push(apComp.createObject(root, {
                    lastIpcObject: n
                }));
        }

        root.wifiConnectTarget = d.connecting ? rNetworks.find(rn => rn.ssid === d.connecting) ?? null : null;

        const needs = d.needs_password ?? "";
        if (needs !== root.lastNeedsPassword) {
            root.lastNeedsPassword = needs;
            const target = rNetworks.find(rn => rn.ssid === needs);
            if (target)
                target.askingPassword = true;
        }

        updateFriendlyList();
    }

    function updateActive() {
        for (let i = 0; i < wifiNetworks.length; i++) {
            if (wifiNetworks[i].active) {
                active = wifiNetworks[i];
                return;
            }
        }
        active = null;
    }

    function updateFriendlyList() {
        friendlyWifiNetworks = [...wifiNetworks].sort((a, b) => {
            if (a.active && !b.active)
                return -1;
            if (!a.active && b.active)
                return 1;
            return b.strength - a.strength;
        });
        updateActive();
    }

    function update() {
        send("refresh");
    }

    function enableWifi(enabled = true): void {
        send("enable", {
            on: enabled
        });
    }

    function toggleWifi(): void {
        enableWifi(!wifiEnabled);
    }

    function rescanWifi(): void {
        if (scanBusy)
            return;
        send("rescan");
    }

    function connectToWifiNetwork(accessPoint: WifiAccessPoint): void {
        accessPoint.askingPassword = false;
        send("connect", {
            ssid: accessPoint.ssid
        });
    }

    function disconnectWifiNetwork(): void {
        if (active)
            send("disconnect");
    }

    function changePassword(network: WifiAccessPoint, password: string): void {
        network.askingPassword = false;
        send("password", {
            ssid: network.ssid,
            psk: password
        });
    }

    function openPublicWifiPortal() {
        Quickshell.execDetached(["xdg-open", "https://nmcheck.gnome.org/"]);
    }

    Component {
        id: apComp
        WifiAccessPoint {}
    }
}
