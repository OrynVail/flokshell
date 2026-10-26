import QtQuick

QtObject {
    id: root

    property var lastIpcObject: ({})

    readonly property string address: lastIpcObject.address ?? ""
    readonly property string name: lastIpcObject.name ?? "Unknown"
    readonly property string icon: lastIpcObject.icon || "bluetooth"
    readonly property bool paired: lastIpcObject.paired ?? false
    readonly property bool connected: lastIpcObject.connected ?? false
    readonly property bool trusted: lastIpcObject.trusted ?? false
    readonly property int battery: lastIpcObject.battery ?? -1
    readonly property bool batteryAvailable: battery >= 0
    readonly property bool connecting: lastIpcObject.connecting ?? false

    function connect() {
        BluetoothService.connectDevice(address);
    }

    function disconnect() {
        BluetoothService.disconnectDevice(address);
    }

    function pair() {
        BluetoothService.pairDevice(address);
    }

    function trust() {
        BluetoothService.trustDevice(address);
    }

    function forget() {
        BluetoothService.removeDevice(address);
    }
}
