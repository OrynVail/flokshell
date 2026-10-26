pragma Singleton
pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import Quickshell.Io

Singleton {
    id: root

    property bool enabled: false
    property bool discovering: false
    property bool connected: false
    property int connectedDevices: 0
    property bool isUpdating: false
    property bool wasEnabledBeforeSleep: false

    readonly property list<BluetoothDevice> devices: []
    property list<var> friendlyDeviceList: []

    property var suspendConnections: Connections {
        target: SuspendManager
        function onPreparingForSleep() {
            root.wasEnabledBeforeSleep = root.enabled;
            if (root.discovering)
                root.stopDiscovery();
        }
        function onWakingUp() {
            if (root.wasEnabledBeforeSleep)
                root.setEnabled(true);
        }
    }

    // flokd owns BlueZ; each message is the whole Bluetooth state.
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
                    if (msg.service === "bluetooth")
                        root.apply(msg.data);
                } catch (e) {
                    console.warn("BluetoothService: bad frame:", e);
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
            service: "bluetooth",
            cmd: cmd
        };
        if (args !== undefined)
            msg.args = args;
        flokd.write(JSON.stringify(msg) + "\n");
        flokd.flush();
    }

    function apply(d) {
        root.enabled = d.enabled;
        root.discovering = d.discovering;
        root.connected = d.connected;
        root.connectedDevices = d.connected_devices;
        root.isUpdating = d.busy;

        const incoming = d.devices || [];
        const rDevices = root.devices;
        for (let i = rDevices.length - 1; i >= 0; i--) {
            const rd = rDevices[i];
            if (!incoming.some(n => n.address === rd.address)) {
                rDevices.splice(i, 1);
                rd.destroy();
            }
        }
        for (const n of incoming) {
            const existing = rDevices.find(rd => rd.address === n.address);
            if (existing)
                existing.lastIpcObject = n;
            else
                rDevices.push(deviceComp.createObject(root, {
                    lastIpcObject: n
                }));
        }

        // Already sorted by flokd.
        root.friendlyDeviceList = incoming.map(n => rDevices.find(rd => rd.address === n.address));
    }

    function initialize() {}

    function updateStatus() {
        send("refresh");
    }

    function updateDevices() {
        send("refresh");
    }

    function setEnabled(value: bool): void {
        if (SuspendManager.isSuspending)
            return;
        send("enable", {
            on: value
        });
    }

    function toggle(): void {
        setEnabled(!enabled);
    }

    function startDiscovery(): void {
        if (enabled && !SuspendManager.isSuspending)
            send("discover", {
                on: true
            });
    }

    function stopDiscovery(): void {
        send("discover", {
            on: false
        });
    }

    function connectDevice(address: string): void {
        send("connect", {
            address: address
        });
    }

    function disconnectDevice(address: string): void {
        send("disconnect", {
            address: address
        });
    }

    function pairDevice(address: string): void {
        send("pair", {
            address: address
        });
    }

    function trustDevice(address: string): void {
        send("trust", {
            address: address
        });
    }

    function removeDevice(address: string): void {
        send("forget", {
            address: address
        });
    }

    Component {
        id: deviceComp
        BluetoothDevice {}
    }
}
