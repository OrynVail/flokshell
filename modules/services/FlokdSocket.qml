import QtQuick
import Quickshell
import Quickshell.Io

// One connection to flokd, filtered to one service's frames.
Socket {
    id: socket

    required property string service

    signal message(var data)
    // The whole frame, for keyed services that need its event and key.
    signal frame(var msg)

    path: (Quickshell.env("XDG_RUNTIME_DIR") || "/run/user/1000") + "/flokd.sock"
    connected: true

    parser: SplitParser {
        splitMarker: "\n"
        onRead: data => {
            if (!data)
                return;
            try {
                const msg = JSON.parse(data);
                if (msg.service !== socket.service)
                    return;
                socket.frame(msg);
                socket.message(msg.data);
            } catch (e) {
                console.warn(`flokd ${socket.service}: bad frame:`, e);
            }
        }
    }

    onConnectionStateChanged: {
        if (!connected)
            reconnect.start();
    }

    property Timer reconnect: Timer {
        interval: 2000
        onTriggered: socket.connected = true
    }

    function send(cmd, args) {
        let msg = {
            service: socket.service,
            cmd: cmd
        };
        if (args !== undefined)
            msg.args = args;
        socket.write(JSON.stringify(msg) + "\n");
        socket.flush();
    }
}
