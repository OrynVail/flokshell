import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import Quickshell.Services.Mpris
import qs.modules.theme
import qs.modules.components
import qs.modules.globals
import qs.modules.services
import qs.config
import "calendar"

// Three columns on one surface, divided by hairlines rather than nested cards.
// The five toggles sit on the calendar; that grouping is what sets them apart.
// Nothing scrolls.
Rectangle {
    id: root
    color: "transparent"
    implicitWidth: 800
    implicitHeight: 530

    property int leftPanelWidth: 0

    readonly property int gutter: Styling.gutter
    readonly property int railHeight: Styling.control

    readonly property int mediaWidth: 216     // the player card, disc plus breathing room
    readonly property int calendarWidth: 263  // seven day cells, scaled to the body

    // Five toggles spanning the calendar exactly, at its own spacing
    readonly property int toggleSpacing: 4
    readonly property int toggleSize: Math.round((calendarWidth - toggleSpacing * 4) / 5)

    // Bar + gutter + knob == one media column, so the groups align to the edges
    readonly property int sliderWidth: mediaWidth - gutter - railHeight

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // ── Body ────────────────────────────────────────────────────────────
        RowLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 0

            // Now playing
            FullPlayer {
                Layout.fillWidth: false
                Layout.preferredWidth: root.mediaWidth
                Layout.maximumWidth: root.mediaWidth
                Layout.fillHeight: true
                Layout.rightMargin: root.gutter
            }

            Separator {
                vert: true
                Layout.fillHeight: true
            }

            // Toggles over the calendar
            ColumnLayout {
                Layout.fillWidth: false
                Layout.preferredWidth: root.calendarWidth
                Layout.maximumWidth: root.calendarWidth
                Layout.fillHeight: true
                Layout.leftMargin: root.gutter
                Layout.rightMargin: root.gutter
                spacing: Styling.tight

                QuickControls {
                    id: quickControls
                    buttonSize: root.toggleSize
                    spacing: root.toggleSpacing
                    panelWidth: root.calendarWidth
                    Layout.fillWidth: true
                    Layout.preferredHeight: root.toggleSize
                    z: 2 // the drawer hangs over the calendar
                }

                // Takes the rest of the column; the grid scales to fit
                Calendar {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                }
            }

            Separator {
                vert: true
                Layout.fillHeight: true
            }

            NotificationHistory {
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.leftMargin: root.gutter
            }
        }

        Separator {
            Layout.fillWidth: true
            Layout.topMargin: Styling.tight
        }


        // ── Control rail ────────────────────────────────────────────────────
        // A band, not part of the column grid. Sound left, mic mirrored right,
        // brightness alone in the dead centre of the window.
        Item {
            Layout.fillWidth: true
            Layout.preferredHeight: root.railHeight
            Layout.topMargin: Styling.tight

            // Sound: bar then knob
            RowLayout {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                spacing: root.gutter

                StyledSlider {
                    id: volumeSlider
                    // Sized by the layout; its own fillWidth default would stretch it
                    resizeParent: false
                    Layout.fillWidth: false
                    Layout.preferredWidth: root.sliderWidth
                    Layout.preferredHeight: root.railHeight
                    Layout.alignment: Qt.AlignVCenter
                    icon: ""
                    iconClickable: false
                    scroll: true
                    smoothDrag: true
                    progressColor: Audio.sink?.audio?.muted ? Colors.outline : Styling.srItem("overprimary")

                    value: Audio.sink?.audio?.volume ?? 0

                    onValueChanged: {
                        if (Audio.sink?.audio && Math.abs(Audio.sink.audio.volume - value) > 0.0001)
                            Audio.sink.audio.volume = value;
                    }

                    // Dragging assigns value and breaks the binding above; restore it
                    Connections {
                        target: Audio.sink?.audio ?? null
                        ignoreUnknownSignals: true
                        function onVolumeChanged() {
                            if (!volumeSlider.isDragging)
                                volumeSlider.value = Audio.sink.audio.volume;
                        }
                    }
                }

                CircularControl {
                    id: volumeControl
                    flat: true
                    Layout.preferredWidth: root.railHeight
                    Layout.preferredHeight: root.railHeight
                    Layout.alignment: Qt.AlignVCenter

                    icon: {
                        if (Audio.sink?.audio?.muted)
                            return Icons.speakerSlash;
                        const vol = Audio.sink?.audio?.volume ?? 0;
                        if (vol < 0.01)
                            return Icons.speakerX;
                        if (vol < 0.19)
                            return Icons.speakerNone;
                        if (vol < 0.49)
                            return Icons.speakerLow;
                        return Icons.speakerHigh;
                    }
                    value: Audio.sink?.audio?.volume ?? 0
                    accentColor: Audio.sink?.audio?.muted ? Colors.outline : Styling.srItem("overprimary")
                    isToggleable: true
                    isToggled: !(Audio.sink?.audio?.muted ?? false)

                    onControlValueChanged: newValue => {
                        if (Audio.sink?.audio)
                            Audio.sink.audio.volume = newValue;
                    }

                    onToggled: {
                        if (Audio.sink?.audio)
                            Audio.sink.audio.muted = !Audio.sink.audio.muted;
                    }
                }
            }

            // Brightness: knob only, dead centre of the window
            CircularControl {
                id: brightnessControl
                flat: true
                width: root.railHeight
                height: root.railHeight
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.verticalCenter: parent.verticalCenter

                icon: Icons.sun
                value: brightnessValue
                accentColor: Styling.srItem("overprimary")
                isToggleable: true
                isToggled: Brightness.syncBrightness

                property real brightnessValue: 0
                property var currentMonitor: {
                    if (Brightness.monitors.length > 0) {
                        let focusedName = CompositorService.focusedMonitor?.name ?? "";
                        let found = null;
                        for (let i = 0; i < Brightness.monitors.length; i++) {
                            let mon = Brightness.monitors[i];
                            if (mon && mon.screen && mon.screen.name === focusedName) {
                                found = mon;
                                break;
                            }
                        }
                        return found || Brightness.monitors[0];
                    }
                    return null;
                }

                Component.onCompleted: {
                    if (currentMonitor && currentMonitor.ready)
                        brightnessValue = currentMonitor.brightness;
                }

                // Mirrors brightness across every monitor, as it did before
                onToggled: Brightness.syncBrightness = !Brightness.syncBrightness

                onControlValueChanged: newValue => {
                    brightnessValue = newValue;
                    if (Brightness.syncBrightness) {
                        for (let i = 0; i < Brightness.monitors.length; i++) {
                            let mon = Brightness.monitors[i];
                            if (mon && mon.ready)
                                mon.setBrightness(newValue);
                        }
                    } else if (currentMonitor && currentMonitor.ready) {
                        currentMonitor.setBrightness(newValue);
                    }
                }

                Connections {
                    target: brightnessControl.currentMonitor
                    ignoreUnknownSignals: true
                    function onBrightnessChanged() {
                        if (brightnessControl.currentMonitor && brightnessControl.currentMonitor.ready)
                            brightnessControl.brightnessValue = brightnessControl.currentMonitor.brightness;
                    }
                    function onReadyChanged() {
                        if (brightnessControl.currentMonitor && brightnessControl.currentMonitor.ready)
                            brightnessControl.brightnessValue = brightnessControl.currentMonitor.brightness;
                    }
                }
            }

            // Mic: knob then bar, mirroring the sound group
            RowLayout {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                spacing: root.gutter

                CircularControl {
                    id: micControl
                    flat: true
                    Layout.preferredWidth: root.railHeight
                    Layout.preferredHeight: root.railHeight
                    Layout.alignment: Qt.AlignVCenter

                    icon: Audio.source?.audio?.muted ? Icons.micSlash : Icons.mic
                    value: Audio.source?.audio?.volume ?? 0
                    accentColor: Audio.source?.audio?.muted ? Colors.outline : Styling.srItem("overprimary")
                    isToggleable: true
                    isToggled: !(Audio.source?.audio?.muted ?? false)

                    onControlValueChanged: newValue => {
                        if (Audio.source?.audio)
                            Audio.source.audio.volume = newValue;
                    }

                    onToggled: {
                        if (Audio.source?.audio)
                            Audio.source.audio.muted = !Audio.source.audio.muted;
                    }
                }

                StyledSlider {
                    id: micSlider
                    resizeParent: false
                    Layout.fillWidth: false
                    Layout.preferredWidth: root.sliderWidth
                    Layout.preferredHeight: root.railHeight
                    Layout.alignment: Qt.AlignVCenter
                    icon: ""
                    iconClickable: false
                    scroll: true
                    smoothDrag: true
                    progressColor: Audio.source?.audio?.muted ? Colors.outline : Styling.srItem("overprimary")

                    value: Audio.source?.audio?.volume ?? 0

                    onValueChanged: {
                        if (Audio.source?.audio && Math.abs(Audio.source.audio.volume - value) > 0.0001)
                            Audio.source.audio.volume = value;
                    }

                    Connections {
                        target: Audio.source?.audio ?? null
                        ignoreUnknownSignals: true
                        function onVolumeChanged() {
                            if (!micSlider.isDragging)
                                micSlider.value = Audio.source.audio.volume;
                        }
                    }
                }
            }
        }
    }
}
