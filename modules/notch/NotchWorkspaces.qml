pragma ComponentBehavior: Bound
import QtQuick
import qs.modules.bar.workspaces
import qs.modules.services
import qs.modules.theme
import qs.config

// Workspace indicator for the notch idle row.
//
// Shows the contiguous span between the lowest and highest workspace in use, so
// skipped workspaces stay visible as gaps rather than collapsing and making
// distant workspaces look adjacent. Three tiers: active is a pill, in-use is a
// filled circle, skipped-but-within-span is a faint circle.
//
// Only width animates — height and radius are constant — which keeps the motion
// smooth instead of several springy properties fighting each other.
Item {
    id: root

    required property var screen

    readonly property int slotCount: Math.max(1, Config.workspaces.shown)
    readonly property int dotSize: 10
    readonly property int capsuleWidth: 24
    readonly property int gap: 6

    readonly property var monitor: CompositorService.monitorFor(screen)
    readonly property int activeId: monitor?.activeWorkspace?.id ?? 1
    readonly property int group: Math.floor((activeId - 1) / slotCount)

    function workspaceId(index) {
        return root.group * root.slotCount + index + 1;
    }

    // Span of workspaces worth showing: everything between the lowest and
    // highest that is either occupied or currently active
    readonly property int spanLo: {
        let lo = root.activeId;
        for (let i = 0; i < root.slotCount; i++) {
            const id = root.workspaceId(i);
            if (CompositorData.workspaceOccupationMap[id] && id < lo)
                lo = id;
        }
        return lo;
    }
    readonly property int spanHi: {
        let hi = root.activeId;
        for (let i = 0; i < root.slotCount; i++) {
            const id = root.workspaceId(i);
            if (CompositorData.workspaceOccupationMap[id] && id > hi)
                hi = id;
        }
        return hi;
    }

    // Trailing gap trimmed so the row stays optically centred
    implicitWidth: Math.max(0, slotRow.implicitWidth - gap)
    implicitHeight: dotSize

    WheelHandler {
        acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
        onWheel: event => {
            if (event.angleDelta.y < 0)
                CompositorService.dispatch(`workspace r+1`);
            else if (event.angleDelta.y > 0)
                CompositorService.dispatch(`workspace r-1`);
        }
    }

    Row {
        id: slotRow
        anchors.centerIn: parent
        // Gap lives inside each slot, so collapsed slots leave no phantom space
        spacing: 0

        Repeater {
            model: root.slotCount

            Item {
                id: slot
                required property int index

                readonly property int wsId: root.workspaceId(index)
                readonly property bool isActive: wsId === root.activeId
                readonly property bool isOccupied: CompositorData.workspaceOccupationMap[wsId] ?? false
                readonly property bool shown: wsId >= root.spanLo && wsId <= root.spanHi

                readonly property int bodyWidth: isActive ? root.capsuleWidth : root.dotSize

                width: shown ? bodyWidth + root.gap : 0
                height: root.dotSize
                opacity: shown ? 1 : 0

                Behavior on width {
                    enabled: Config.animDuration > 0
                    NumberAnimation {
                        duration: Config.animDuration
                        easing.type: Easing.OutCubic
                    }
                }

                Behavior on opacity {
                    enabled: Config.animDuration > 0
                    NumberAnimation {
                        duration: Config.animDuration / 2
                        easing.type: Easing.OutCubic
                    }
                }

                Rectangle {
                    id: body
                    width: slot.bodyWidth
                    height: root.dotSize
                    radius: height / 2
                    color: Colors.overBackground

                    // active pill > in use > skipped over
                    opacity: slot.isActive ? 0.85 : (slot.isOccupied ? 0.55 : 0.2)

                    Behavior on width {
                        enabled: Config.animDuration > 0
                        NumberAnimation {
                            duration: Config.animDuration
                            easing.type: Easing.OutCubic
                        }
                    }

                    Behavior on opacity {
                        enabled: Config.animDuration > 0
                        NumberAnimation {
                            duration: Config.animDuration / 2
                            easing.type: Easing.OutCubic
                        }
                    }
                }

                // Taller than the pill so it is comfortable to hit
                MouseArea {
                    width: slot.bodyWidth
                    height: 24
                    anchors.verticalCenter: body.verticalCenter
                    enabled: slot.shown
                    cursorShape: Qt.PointingHandCursor
                    onPressed: CompositorService.dispatch(`workspace ${slot.wsId}`)
                }
            }
        }
    }
}
