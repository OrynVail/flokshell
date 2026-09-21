import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import Quickshell
import Quickshell.Services.SystemTray
import Quickshell.Widgets
import qs.modules.theme
import qs.modules.services
import qs.modules.components
import qs.config

MouseArea {
    id: root

    required property var bar
    required property SystemTrayItem item
    property int trayItemSize: 20
    property bool isHovered: false

    readonly property var trayIconSource: {
        const iconPath = item.icon.toString();
        if (iconPath.includes("spotify"))
            return Quickshell.iconPath("spotify-client");
        return item.icon;
    }

    acceptedButtons: Qt.LeftButton | Qt.RightButton
    Layout.fillHeight: bar.orientation === "horizontal"
    Layout.fillWidth: bar.orientation === "vertical"
    implicitWidth: trayItemSize
    implicitHeight: trayItemSize

    onClicked: event => {
        switch (event.button) {
        case Qt.LeftButton:
            item.activate();
            break;
        case Qt.RightButton:
            if (item.hasMenu) {
                systrayPopup.toggle();
            }
            break;
        }
        event.accepted = true;
    }

    BarPopup {
        id: systrayPopup
        anchorItem: root
        bar: root.bar

        popupPadding: Styling.tight
        contentHeight: Math.min(header.height + headerRule.height + Styling.tight + itemsColumn.implicitHeight, 420) + popupPadding * 2

        QsMenuOpener {
            id: menuOpener
            menu: root.item.menu
        }

        // Centred on the notch, nothing else says which app this menu is
        RowLayout {
            id: header
            anchors.top: parent.top
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.leftMargin: Styling.tight
            anchors.rightMargin: Styling.tight
            height: Styling.control - Styling.tight
            spacing: Styling.tight

            IconImage {
                source: root.trayIconSource
                implicitWidth: Styling.glyph
                implicitHeight: Styling.glyph
                smooth: true
            }

            Text {
                Layout.fillWidth: true
                text: root.item.title || root.item.id || ""
                color: Colors.overBackground
                font.family: Styling.defaultFont
                font.pixelSize: Styling.fontSize(0)
                font.bold: true
                elide: Text.ElideRight
                verticalAlignment: Text.AlignVCenter
            }
        }

        Separator {
            id: headerRule
            anchors.top: header.bottom
            anchors.left: parent.left
            anchors.right: parent.right
        }

        ScrollView {
            anchors.top: headerRule.bottom
            anchors.topMargin: Styling.tight
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            contentWidth: availableWidth
            clip: true

            ScrollBar.horizontal.policy: ScrollBar.AlwaysOff

            ColumnLayout {
                id: itemsColumn
                width: parent.width
                spacing: 2

                Repeater {
                    model: menuOpener.children ? menuOpener.children.values : []

                    delegate: ColumnLayout {
                        required property var modelData

                        Layout.fillWidth: true
                        spacing: 2

                        property bool submenuExpanded: false

                        SystrayMenuItem {
                            Layout.fillWidth: true

                            textStr: modelData.text || ""
                            iconSource: modelData.icon || ""
                            isImageIcon: iconSource.indexOf("/") !== -1 || iconSource.indexOf(".") !== -1
                            isSeparator: modelData.isSeparator || false
                            hasSubmenu: modelData.hasChildren || false
                            expanded: parent.submenuExpanded
                            buttonType: modelData.buttonType || 0
                            checkState: modelData.checkState || 0

                            onClicked: {
                                if (modelData.hasChildren) {
                                    parent.submenuExpanded = !parent.submenuExpanded;
                                } else {
                                    if (modelData.triggered) {
                                        modelData.triggered();
                                    } else if (modelData.activate) {
                                        modelData.activate();
                                    }
                                    systrayPopup.close();
                                }
                            }
                        }

                        // Submenu children — uses its own QsMenuOpener to trigger lazy loading
                        ColumnLayout {
                            visible: submenuExpanded && modelData.hasChildren
                            Layout.fillWidth: true
                            spacing: 2

                            QsMenuOpener {
                                id: subMenuOpener
                                menu: modelData.hasChildren ? modelData : null
                            }

                            Repeater {
                                model: subMenuOpener.children ? subMenuOpener.children.values : []

                                delegate: SystrayMenuItem {
                                    required property var modelData

                                    Layout.fillWidth: true
                                    depth: 1

                                    textStr: modelData.text || ""
                                    iconSource: modelData.icon || ""
                                    isImageIcon: iconSource.indexOf("/") !== -1 || iconSource.indexOf(".") !== -1
                                    isSeparator: modelData.isSeparator || false
                                    buttonType: modelData.buttonType || 0
                                    checkState: modelData.checkState || 0

                                    onClicked: {
                                        if (modelData.triggered) {
                                            modelData.triggered();
                                        } else if (modelData.activate) {
                                            modelData.activate();
                                        }
                                        systrayPopup.close();
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Circular chip on hover only — the icon otherwise sits bare on whatever
    // surface is behind it.
    StyledRect {
        anchors.centerIn: parent
        width: root.trayItemSize + 12
        height: width
        radius: height / 2
        variant: root.isHovered ? "focus" : "transparent"
    }

    IconImage {
        id: trayIcon
        source: root.trayIconSource
        anchors.centerIn: parent
        width: parent.width
        height: parent.height
        smooth: true
        opacity: root.isHovered ? 1.0 : 0.75

        Behavior on opacity {
            enabled: Config.animDuration > 0
            NumberAnimation {
                duration: Config.animDuration / 2
                easing.type: Easing.OutQuart
            }
        }
    }

    Tinted {
        sourceItem: trayIcon
        anchors.fill: trayIcon
    }

    StyledToolTip {
        show: root.isHovered
        tooltipText: root.item.tooltipTitle || root.item.title
        desciription: root.item.tooltipDescription || ""
    }

    HoverHandler {
        onHoveredChanged: root.isHovered = hovered
    }
}
