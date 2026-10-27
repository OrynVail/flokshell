pragma Singleton

import QtQuick
import Quickshell

Singleton {
    id: root

    // What was asked for, which survives restarts; flokd reports whether it took.
    property bool wanted: StateService.get("nightLight", false)
    property bool active: false
    property bool available: true

    function toggle() {
        root.wanted = !root.wanted;
        if (StateService.initialized)
            StateService.set("nightLight", root.wanted);
        root.push();
    }

    function push() {
        flokd.send("set", {
            on: root.wanted
        });
    }

    Connections {
        target: StateService
        function onStateLoaded() {
            root.wanted = StateService.get("nightLight", false);
            root.push();
        }
    }

    FlokdSocket {
        id: flokd
        service: "nightlight"
        onMessage: d => {
            root.active = d.active;
            root.available = d.available;
        }
        onConnectionStateChanged: {
            if (connected)
                root.push();
        }
    }
}
