pragma Singleton

import QtQuick
import Quickshell

// flokd owns the recording, so it outlives a shell reload.
QtObject {
    id: root

    property bool isRecording: false
    property string duration: ""
    property string lastError: ""
    property bool canRecordDirectly: true
    property string videosDir: ""
    property double started: 0

    property FlokdSocket flokd: FlokdSocket {
        service: "recorder"
        onMessage: d => {
            root.isRecording = d.recording;
            root.started = d.started;
            root.canRecordDirectly = d.direct;
            root.videosDir = d.dir;
            root.lastError = d.error;
            root.tick();
        }
    }

    property Timer clock: Timer {
        interval: 1000
        repeat: true
        running: root.isRecording
        onTriggered: root.tick()
    }

    function tick() {
        if (!isRecording) {
            duration = "";
            return;
        }
        const total = Math.max(0, Math.floor((Date.now() - started) / 1000));
        const pad = n => String(n).padStart(2, "0");
        const hours = Math.floor(total / 3600);
        const clock = `${pad(Math.floor(total / 60) % 60)}:${pad(total % 60)}`;
        duration = hours > 0 ? `${hours}:${clock}` : clock;
    }

    function toggleRecording() {
        if (isRecording)
            flokd.send("stop");
        else
            startRecording(false, false, "portal", "");
    }

    function startRecording(recordAudioOutput, recordAudioInput, mode, regionStr) {
        flokd.send("start", {
            mode: mode,
            region: regionStr,
            output: recordAudioOutput,
            input: recordAudioInput
        });
    }

    function openRecordingsFolder() {
        flokd.send("open");
    }
}
