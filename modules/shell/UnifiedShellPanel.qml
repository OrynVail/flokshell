import QtQuick
import QtQuick.Effects
import Quickshell
import Quickshell.Wayland
import qs.modules.bar
import qs.modules.bar.workspaces
import qs.modules.notch
import qs.modules.dock
import qs.modules.frame
import qs.modules.services
import qs.modules.globals
import qs.modules.components
import qs.config

PanelWindow {
    id: unifiedPanel

    required property ShellScreen targetScreen
    screen: targetScreen

    anchors {
        top: true
        bottom: true
        left: true
        right: true
    }

    color: "transparent"

    // Dynamic keyboard focus: Exclusive when a notch module is open (so text fields work),
    // None otherwise (so compositor receives normal input).
    WlrLayershell.keyboardFocus: {
        if (notchContent.screenNotchOpen) {
            return WlrKeyboardFocus.Exclusive;
        }
        return WlrKeyboardFocus.None;
    }
    WlrLayershell.namespace: "flokshell"
    WlrLayershell.layer: WlrLayer.Overlay
    exclusionMode: ExclusionMode.Ignore

    // Whether we need to capture full-screen input for click-outside detection.
    // True when notch modules are open OR any FocusGrab is active (e.g., BarPopups).
    readonly property bool needsFullScreenInput: notchContent.screenNotchOpen || FocusGrabManager.hasActiveGrab

    readonly property bool dockEnabled: {
        if (!Config.dockReady) return false;
        if (!(Config.dock.enabled ?? false) || (Config.dock.theme ?? "default") === "integrated")
            return false;
        const list = Config.dock.screenList;
        return (!list || list.length === 0 || list.indexOf(targetScreen.name) !== -1);
    }

    readonly property alias dockPosition: dockContent.position
    readonly property alias dockPinned: dockContent.pinned
    readonly property bool dockReveal: dockEnabled && dockContent.reveal
    readonly property alias dockFullscreen: dockContent.activeWindowFullscreen
    readonly property int dockHeight: dockContent.dockSize + dockContent.totalMargin

    readonly property string notchPosition: Config.notchPosition ?? "top"
    readonly property alias notchReservedHeight: notchContent.reservedHeight
    readonly property alias notchHoverActive: notchContent.hoverActive
    readonly property alias notchOpen: notchContent.screenNotchOpen
    readonly property alias notchReveal: notchContent.reveal

    // Generic names for external compatibility (Visibilities expects these on the panel object)
    readonly property alias reveal: notchContent.reveal
    readonly property alias hoverActive: notchContent.hoverActive

    readonly property bool unifiedEffectActive: false // Flag to notify children to disable internal borders

    readonly property var compositorMonitor: CompositorService.monitorFor(targetScreen)
    readonly property bool hasFullscreenWindow: {
        if (!compositorMonitor)
            return false;

        const activeWorkspaceId = compositorMonitor.activeWorkspace.id;
        const monId = compositorMonitor.id;

        // Check active toplevel first (fast path)
        const toplevel = ToplevelManager.activeToplevel;
        if (toplevel && toplevel.fullscreen && CompositorService.focusedMonitor.id === monId) {
            return true;
        }

        // Check all windows on this monitor (robust path)
        const wins = CompositorData.windowList;
        for (let i = 0; i < wins.length; i++) {
            if (wins[i].monitor === monId && wins[i].fullscreen && wins[i].workspace.id === activeWorkspaceId) {
                return true;
            }
        }
        return false;
    }

    Component.onCompleted: {
        // barPanels is still the registry key other modules look this panel up by
        Visibilities.registerBarPanel(screen.name, unifiedPanel);
        Visibilities.registerNotchPanel(screen.name, unifiedPanel);
        Visibilities.registerDockPanel(screen.name, dockContent);
        Visibilities.registerNotch(screen.name, notchContent.notchContainerRef);
        Visibilities.registerDock(screen.name, dockContent);
    }

    Component.onDestruction: {
        Visibilities.unregisterBarPanel(screen.name);
        Visibilities.unregisterNotchPanel(screen.name);
        Visibilities.unregisterDockPanel(screen.name);
        Visibilities.unregisterNotch(screen.name);
        Visibilities.unregisterDock(screen.name);
    }

    // Full-screen mask item (used when modules/popups are open)
    Item {
        id: fullScreenMask
        anchors.fill: parent
    }

    // Mask Region Logic
    // When a module or popup is open, expand to full-screen to capture click-outside.
    // Otherwise, restrict input to Bar, Notch, and Dock hitboxes only.
    mask: Region {
        // Full-screen capture when any module/popup is open
        item: unifiedPanel.needsFullScreenInput ? fullScreenMask : null
        regions: [
            Region {
                item: notchContent.notchHitbox
            },
            Region {
                // Only include the dock hitbox if the dock is actually enabled and visible on this screen.
                item: dockContent.visible ? dockContent.dockHitbox : null
            }
        ]
    }

    // Focus Grab for Notch — registers with FocusGrabManager for click-outside coordination
    FocusGrab {
        id: focusGrab
        windows: [unifiedPanel]
        active: notchContent.screenNotchOpen

        onCleared: {
            Visibilities.setActiveModule("");
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // CLICK-OUTSIDE BACKDROP
    // ═══════════════════════════════════════════════════════════════

    // Transparent backdrop that captures clicks on empty areas when modules/popups are open.
    // z: -1 ensures it's below all visual content (bar, notch, dock).
    MouseArea {
        id: backdropArea
        anchors.fill: parent
        visible: unifiedPanel.needsFullScreenInput
        z: -1

        onClicked: {
            FocusGrabManager.clearTopGrab();
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // VISUAL CONTENT
    // ═══════════════════════════════════════════════════════════════

    Item {
        id: visualContent
        anchors.fill: parent

        layer.enabled: true
        layer.effect: Shadow {}

        ScreenFrameContent {
            id: frameContent
            anchors.fill: parent
            targetScreen: unifiedPanel.targetScreen
            hasFullscreenWindow: unifiedPanel.hasFullscreenWindow
            z: 1
        }

        DockContent {
            id: dockContent
            unifiedEffectActive: unifiedPanel.unifiedEffectActive
            anchors.fill: parent
            screen: unifiedPanel.targetScreen
            z: 3
            visible: unifiedPanel.dockEnabled
        }

        NotchContent {
            id: notchContent
            unifiedEffectActive: unifiedPanel.unifiedEffectActive
            anchors.fill: parent
            screen: unifiedPanel.targetScreen
            z: 4
        }


    }
}
