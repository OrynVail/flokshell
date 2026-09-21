pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import QtQuick.Effects
import Quickshell
import Quickshell.Wayland
import qs.modules.services
import qs.modules.theme
import qs.modules.components
import qs.config

// Base for the notch applets
PopupWindow {
    id: root

    // Used only when the bar supplies no appletAnchor
    required property Item anchorItem
    // Required: the bar panel for position detection
    required property var bar

    // Content to display inside the popup
    default property alias contentData: contentContainer.data

    // Visual configuration
    property int popupPadding: 8
    property int visualMargin: 8  // Distance from bar
    property int shadowMargin: 16  // Extra margin for shadow
    property string variant: "popup"  // StyledRect variant for background

    // Behavior configuration
    property bool closeOnFocusLost: true

    // Logical open state (changes immediately, not after animation)
    property bool isOpen: false

    // Signal emitted when popup is closed externally (click outside)
    signal closedExternally

    // Animation state
    property real popupOpacity: 0
    property real popupScale: 0.9

    // Bar position detection
    readonly property string barPosition: bar?.barPosition ?? "top"
    readonly property bool barAtTop: barPosition === "top"
    readonly property bool barAtBottom: barPosition === "bottom"
    readonly property bool barAtLeft: barPosition === "left"
    readonly property bool barAtRight: barPosition === "right"
    readonly property bool barVertical: barAtLeft || barAtRight

    // Total size including shadow margin
    readonly property int totalWidth: contentWidth + shadowMargin * 2
    readonly property int totalHeight: contentHeight + shadowMargin * 2
    property int contentWidth: Styling.applet
    property int contentHeight: 150

    // Applets centre on the notch, not on the icon that opened them
    readonly property Item positionAnchor: (bar && bar.appletAnchor) ? bar.appletAnchor : anchorItem

    implicitWidth: totalWidth
    implicitHeight: totalHeight

    // Frame detection
    readonly property bool frameEnabled: Config.frame?.enabled ?? false
    readonly property int frameThickness: Config.frame?.thickness ?? 0
    readonly property int effectiveFrameOffset: frameEnabled ? frameThickness : 0

    anchor.item: positionAnchor
    anchor.rect.x: {
        if (barVertical) {
            if (barAtLeft)
                return positionAnchor.width + visualMargin + effectiveFrameOffset - shadowMargin;
            return -totalWidth + shadowMargin - visualMargin - effectiveFrameOffset;
        }
        return (positionAnchor.width - totalWidth) / 2;
    }
    anchor.rect.y: {
        if (barVertical) {
            return (positionAnchor.height - totalHeight) / 2;
        }
        if (barAtTop)
            return positionAnchor.height + visualMargin + effectiveFrameOffset - shadowMargin;
        return -totalHeight + shadowMargin - visualMargin - effectiveFrameOffset;
    }
    anchor.rect.width: 0
    anchor.rect.height: 0

    color: "transparent"
    visible: false

    // Focus grab for click-outside-to-close behavior
    property bool focusActive: false

    FocusGrab {
        id: focusGrab
        active: root.visible && root.focusActive
        windows: [root]

        onCleared: {
            if (root.closeOnFocusLost && root.isOpen) {
                root.closedExternally();
                root.close();
            }
        }
    }

    // Animation behaviors
    Behavior on popupOpacity {
        enabled: Config.animDuration > 0
        NumberAnimation {
            duration: Config.animDuration
            easing.type: Easing.OutCubic
        }
    }

    Behavior on popupScale {
        enabled: Config.animDuration > 0
        NumberAnimation {
            duration: Config.animDuration
            easing.type: Easing.OutCubic
        }
    }

    // Main content wrapper
    Item {
        id: popupContainer
        anchors.fill: parent
        anchors.margins: root.shadowMargin
        opacity: root.popupOpacity
        scale: root.popupScale
        transformOrigin: {
            if (root.barAtTop)
                return Item.Top;
            if (root.barAtBottom)
                return Item.Bottom;
            if (root.barAtLeft)
                return Item.Left;
            if (root.barAtRight)
                return Item.Right;
            return Item.Center;
        }

        StyledRect {
            id: background
            anchors.fill: parent
            variant: root.variant
            enableShadow: true
            radius: Styling.radius(8)

            Item {
                id: contentContainer
                anchors.fill: parent
                anchors.margins: root.popupPadding
            }
        }
    }

    function open() {
        if (visible)
            return;

        Visibilities.openApplet(root);

        isOpen = true;

        popupOpacity = 0;
        popupScale = 0.9;
        visible = true;

        Qt.callLater(() => {
            popupOpacity = 1;
            popupScale = 1;
            focusActive = true;
        });
    }

    function close() {
        if (!visible)
            return;

        Visibilities.closeApplet(root);

        isOpen = false;
        focusActive = false;

        popupOpacity = 0;
        popupScale = 0.9;

        closeTimer.restart();
    }

    function toggle() {
        if (visible) {
            close();
        } else {
            open();
        }
    }

    Timer {
        id: closeTimer
        interval: Config.animDuration > 0 ? Config.animDuration + 50 : 50
        onTriggered: {
            root.visible = false;
        }
    }
}
