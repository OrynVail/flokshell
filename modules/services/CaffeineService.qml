pragma Singleton

import QtQuick
import Quickshell

Singleton {
    id: root

    property bool inhibit: false

    function toggleInhibit() {
        inhibit = !inhibit;
    }

    onInhibitChanged: {
        if (StateService.initialized)
            StateService.set("caffeine", inhibit);
        push();
    }

    function push() {
        flokd.send("caffeine", {
            on: root.inhibit
        });
    }

    Connections {
        target: StateService
        function onStateLoaded() {
            root.inhibit = StateService.get("caffeine", false);
        }
    }

    FlokdSocket {
        id: flokd
        service: "idle"
        onConnectionStateChanged: {
            if (connected)
                root.push();
        }
    }
}
