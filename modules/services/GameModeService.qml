pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Io

Singleton {
    id: root

    property bool toggled: false
    property bool initialized: false

    readonly property string keywords: "keyword animations:enabled 0; keyword decoration:shadow:enabled 0; keyword decoration:blur:enabled 0; keyword general:gaps_in 0; keyword general:gaps_out 0; keyword general:border_size 1; keyword decoration:rounding 0"

    // axctl until the compositor moves into flokd.
    property Process apply: Process {
        property bool target: false
        onExited: code => {
            if (code !== 0)
                return;
            root.toggled = target;
            StateService.set("gameMode", target);
        }
    }

    function run(on) {
        apply.target = on;
        apply.command = on ? ["axctl", "config", "apply", root.keywords] : ["axctl", "config", "reload"];
        apply.running = true;
    }

    function toggle() {
        run(!toggled);
    }

    Connections {
        target: StateService
        function onStateLoaded() {
            root.initialized = true;
            if (StateService.get("gameMode", false))
                root.run(true);
        }
    }
}
