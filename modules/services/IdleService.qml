pragma Singleton

import QtQuick
import Quickshell

// flokd runs the idle listeners and the lock and sleep hooks from system.json;
// the shell only needs to hear about suspend.
Singleton {
    id: root

    property bool suspended: false

    onSuspendedChanged: suspended ? SuspendManager.onPrepareForSleep() : SuspendManager.onWakingUp()

    FlokdSocket {
        id: flokd
        service: "idle"
        onMessage: d => root.suspended = d.suspended
    }
}
