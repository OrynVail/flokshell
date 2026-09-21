import QtQuick
import QtQuick.Layouts
import qs.modules.theme
import qs.modules.services
import qs.modules.notch
import qs.modules.components
import qs.modules.bar
import qs.modules.bar.clock
import qs.modules.bar.systray
import qs.config

Item {
    id: root
    anchors.top: parent.top
    focus: false

    required property var screen

    // Layout constants
    readonly property int notificationPadding: 16
    readonly property int notificationPaddingBottom: Config.notchTheme === "island" ? 20 : 16
    readonly property int notificationPaddingTop: 8

    // State
    readonly property bool hasActiveNotifications: Notifications.popupList.length > 0
    property bool notchHovered: false
    property bool parentHoverActive: false
    property bool isNavigating: false

    // Position detection
    readonly property string notchPosition: Config.notchPosition ?? "top"
    readonly property bool isBottom: notchPosition === "bottom"

    HoverHandler {
        id: contentHoverHandler
    }

    readonly property bool expandedState: contentHoverHandler.hovered || notchHovered || parentHoverActive || isNavigating

    // Clock and BatteryIndicator were written for the bar; they only read
    // orientation, plus barPosition for popup direction. appletAnchor spans the
    // notch content, so applets centre on it and clear the notifications.
    QtObject {
        id: notchBarStub
        property string orientation: "horizontal"
        property string barPosition: root.notchPosition
        property Item appletAnchor: root
    }

    property real mainRowMargin: 16

    Behavior on mainRowMargin {
        enabled: Config.animDuration > 0
        NumberAnimation {
            duration: Config.animDuration
            easing.type: Easing.OutBack
            easing.overshoot: 1.2
        }
    }

    // Width follows the row's real content, so the notch contracts as
    // workspaces collapse instead of holding a reserved width open
    readonly property real mainRowContentWidth: mainRow.implicitWidth + mainRowMargin * 2
    readonly property real mainRowHeight: Config.showBackground ? (Config.notchTheme === "island" ? 36 : 44) : (Config.notchTheme === "island" ? 36 : 40)
    readonly property real notificationMinWidth: expandedState ? 420 : 320
    readonly property real notificationContainerHeight: notificationView.implicitHeight + notificationPaddingTop + notificationPaddingBottom

    implicitWidth: Math.round(hasActiveNotifications ? Math.max(notificationMinWidth + (notificationPadding * 2), mainRowContentWidth) : mainRowContentWidth)

    implicitHeight: hasActiveNotifications ? mainRowHeight + notificationContainerHeight : mainRowHeight

    Behavior on implicitWidth {
        enabled: Config.animDuration > 0
        NumberAnimation {
            duration: Config.animDuration
            easing.type: Easing.OutBack
            easing.overshoot: 1.2
        }
    }

    Item {
        anchors.fill: parent

        // mainRow: logo leads, then state, then battery. A plain row so the
        // notch hugs its contents.
        RowLayout {
            id: mainRow
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: root.isBottom ? undefined : parent.top
            anchors.bottom: root.isBottom ? parent.bottom : undefined
            height: root.mainRowHeight
            spacing: 8
            z: 2 // Stay above notifications if they ever overlap

            NotchLogo {
                id: notchLogo
                // Logo button has only 4px of internal padding, so it needs an
                // explicit margin to match the other optical gaps
                Layout.rightMargin: 4
                Layout.alignment: Qt.AlignVCenter
            }

            NotchWorkspaces {
                id: workspaces
                screen: root.screen
                Layout.alignment: Qt.AlignVCenter
            }

            Clock {
                id: clockWidget
                bar: notchBarStub
                flat: true
                layerEnabled: false
                Layout.alignment: Qt.AlignVCenter
            }

            // Zero width when empty
            SysTray {
                id: tray
                bar: notchBarStub
                Layout.alignment: Qt.AlignVCenter
            }

            BatteryIndicator {
                id: batteryWidget
                bar: notchBarStub
                flat: true
                layerEnabled: false
                Layout.alignment: Qt.AlignVCenter
            }
        }

        // Notification container with its own padding
        Item {
            id: notificationContainer
            width: parent.width
            height: root.hasActiveNotifications ? root.notificationContainerHeight : 0
            visible: root.hasActiveNotifications

            anchors.top: root.isBottom ? undefined : mainRow.bottom
            anchors.bottom: root.isBottom ? mainRow.top : undefined

            NotchNotificationView {
                id: notificationView
                anchors.fill: parent
                anchors.topMargin: root.notificationPaddingTop
                anchors.leftMargin: root.notificationPadding
                anchors.rightMargin: root.notificationPadding
                anchors.bottomMargin: root.notificationPaddingBottom
                visible: root.hasActiveNotifications
                opacity: visible ? 1 : 0
                notchHovered: root.expandedState
                onIsNavigatingChanged: root.isNavigating = isNavigating

                Behavior on opacity {
                    enabled: Config.animDuration > 0
                    NumberAnimation {
                        duration: Config.animDuration
                        easing.type: Easing.OutQuart
                    }
                }
            }
        }
    }
}
