pragma Singleton

import QtQuick
import Quickshell

Singleton {
    id: root

    property bool toggled: false
    property bool initialized: false

    function push() {
        flokd.send("gamemode", { on: root.toggled });
    }

    function run(on) {
        toggled = on;
        StateService.set("gameMode", on);
        push();
    }

    function toggle() {
        run(!toggled);
    }

    Connections {
        target: StateService
        function onStateLoaded() {
            root.initialized = true;
            root.toggled = StateService.get("gameMode", false);
            root.push();
        }
    }

    // flokd holds it across config reloads; a restarted flokd hears it again here.
    FlokdSocket {
        id: flokd
        service: "compositor"
        onConnectionStateChanged: {
            if (connected && root.initialized)
                root.push();
        }
    }
}
