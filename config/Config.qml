pragma Singleton
pragma ComponentBehavior: Bound
import QtQuick
import Quickshell
import Quickshell.Io
import qs.modules.globals
import qs.modules.theme
import qs.modules.services as Services
import "defaults/theme.js" as ThemeDefaults
import "defaults/frame.js" as FrameDefaults
import "defaults/workspaces.js" as WorkspacesDefaults
import "defaults/overview.js" as OverviewDefaults
import "defaults/notch.js" as NotchDefaults
import "defaults/compositor.js" as CompositorDefaults
import "KeybindActions.js" as KeybindActions
import "defaults/performance.js" as PerformanceDefaults
import "defaults/weather.js" as WeatherDefaults
import "defaults/desktop.js" as DesktopDefaults
import "defaults/lockscreen.js" as LockscreenDefaults
import "defaults/prefix.js" as PrefixDefaults
import "defaults/system.js" as SystemDefaults
import "defaults/dock.js" as DockDefaults
import "defaults/media.js" as MediaDefaults
import "ConfigValidator.js" as ConfigValidator

Singleton {
    id: root

    property string version: "0.0.0"

    FileView {
        id: versionFile
        path: Qt.resolvedUrl("../version").toString().replace("file://", "")
        onLoaded: root.version = text().trim()
    }

    property string configDir: (Quickshell.env("XDG_CONFIG_HOME") || (Quickshell.env("HOME") + "/.config")) + "/flokshell/config"
    property string keybindsPath: (Quickshell.env("XDG_CONFIG_HOME") || (Quickshell.env("HOME") + "/.config")) + "/flokshell/binds.json"
    property string presetDir: Qt.resolvedUrl("../assets/presets/Flokshell Default").toString().replace("file://", "")

    property bool pauseAutoSave: false

    // Module init status
    property bool themeReady: false
    property bool frameReady: false
    property bool workspacesReady: false
    property bool overviewReady: false
    property bool notchReady: false
    property bool compositorReady: false
    property bool performanceReady: false
    property bool weatherReady: false
    property bool desktopReady: false
    property bool lockscreenReady: false
    property bool prefixReady: false
    property bool systemReady: false
    property bool dockReady: false
    property bool mediaReady: false
    property bool keybindsInitialLoadComplete: false

    property bool initialLoadComplete: themeReady && frameReady && workspacesReady && overviewReady && notchReady && compositorReady && performanceReady && weatherReady && desktopReady && lockscreenReady && prefixReady && systemReady && dockReady && mediaReady

    // Compatibility aliases
    property alias loader: themeLoader
    property alias keybindsLoader: keybindsLoader

    // ============================================
    // BATCH INITIALIZATION
    // ============================================
    // Ensure config directory exists and copy preset files if missing
    Process {
        id: ensureConfigDir
        running: true
        // Paths as arguments, never spliced into the script.
        command: ["sh", "-c", 'mkdir -p "$2" && for n in theme frame workspaces overview notch compositor performance desktop lockscreen dock system media; do cp -n "$1/$n.json" "$2/$n.json" 2>/dev/null; done; true', "sh", root.presetDir, root.configDir]
    }

    // Auto-migrate hyprland.json → compositor.json for existing users
    Process {
        id: migrateCompositorConfig
        running: true
        command: ["sh", "-c", 'test -f "$1/hyprland.json" && ! test -f "$1/compositor.json" && mv "$1/hyprland.json" "$1/compositor.json"; true', "sh", root.configDir]
    }

    // ============================================
    // THEME MODULE
    // ============================================
    FileView {
        id: themeLoader
        path: root.configDir + "/theme.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.themeReady) {
                validateModule("theme", themeLoader, ThemeDefaults.data, () => {
                    root.themeReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.themeReady) {
                handleMissingConfig("theme", themeLoader, ThemeDefaults.data, () => {
                    root.themeReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("theme", themeLoader)

        adapter: JsonAdapter {
            property bool oledMode: false
            property bool lightMode: false
            property int roundness: 16
            property string font: "Roboto Condensed"
            property int fontSize: 14
            property string monoFont: "Iosevka Nerd Font Mono"
            property int monoFontSize: 14
            property bool tintIcons: false
            property bool enableCorners: true
            property int animDuration: 150
            property real shadowOpacity: 1
            property string shadowColor: "shadow"
            property int shadowXOffset: 0
            property int shadowYOffset: 0
            property real shadowBlur: 1.52

            property JsonObject srBg: JsonObject {
                property string label: "Background"
                property list<var> gradient: [["background", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "surface"
                property string halftoneBackgroundColor: "background"
                property list<var> border: ["surfaceBright", 0]
                property string itemColor: "overBackground"
                property real opacity: 1.0
            }

            property JsonObject srPopup: JsonObject {
                property string label: "Popup"
                property list<var> gradient: [["background", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "surface"
                property string halftoneBackgroundColor: "background"
                property list<var> border: ["surfaceBright", 2]
                property string itemColor: "overBackground"
                property real opacity: 1.0
            }

            property JsonObject srInternalBg: JsonObject {
                property string label: "Internal BG"
                property list<var> gradient: [["background", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "surface"
                property string halftoneBackgroundColor: "background"
                property list<var> border: ["surfaceBright", 0]
                property string itemColor: "overBackground"
                property real opacity: 1.0
            }

            property JsonObject srBarBg: JsonObject {
                property string label: "Bar BG"
                property list<var> gradient: [["surfaceDim", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "surface"
                property string halftoneBackgroundColor: "surfaceDim"
                property list<var> border: ["surfaceBright", 0]
                property string itemColor: "overBackground"
                property real opacity: 0.0
            }

            property JsonObject srPane: JsonObject {
                property string label: "Pane"
                property list<var> gradient: [["surface", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "surfaceBright"
                property string halftoneBackgroundColor: "surface"
                property list<var> border: ["surfaceBright", 0]
                property string itemColor: "overBackground"
                property real opacity: 1.0
            }

            property JsonObject srCommon: JsonObject {
                property string label: "Common"
                property list<var> gradient: [["surface", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "background"
                property string halftoneBackgroundColor: "surface"
                property list<var> border: ["surfaceBright", 0]
                property string itemColor: "overBackground"
                property real opacity: 1.0
            }

            property JsonObject srFocus: JsonObject {
                property string label: "Focus"
                property list<var> gradient: [["surfaceBright", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "surfaceVariant"
                property string halftoneBackgroundColor: "surfaceBright"
                property list<var> border: ["surfaceBright", 0]
                property string itemColor: "overBackground"
                property real opacity: 1.0
            }

            property JsonObject srPrimary: JsonObject {
                property string label: "Primary"
                property list<var> gradient: [["primary", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "overPrimaryContainer"
                property string halftoneBackgroundColor: "primary"
                property list<var> border: ["primary", 0]
                property string itemColor: "overPrimary"
                property real opacity: 1.0
            }

            property JsonObject srPrimaryFocus: JsonObject {
                property string label: "Primary Focus"
                property list<var> gradient: [["overPrimaryContainer", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "primary"
                property string halftoneBackgroundColor: "overPrimaryContainer"
                property list<var> border: ["overBackground", 0]
                property string itemColor: "overPrimary"
                property real opacity: 1.0
            }

            property JsonObject srOverPrimary: JsonObject {
                property string label: "Over Primary"
                property list<var> gradient: [["overPrimary", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "primaryContainer"
                property string halftoneBackgroundColor: "overPrimary"
                property list<var> border: ["overPrimary", 0]
                property string itemColor: "primary"
                property real opacity: 1.0
            }

            property JsonObject srSecondary: JsonObject {
                property string label: "Secondary"
                property list<var> gradient: [["secondary", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "overSecondaryContainer"
                property string halftoneBackgroundColor: "secondary"
                property list<var> border: ["secondary", 0]
                property string itemColor: "overSecondary"
                property real opacity: 1.0
            }

            property JsonObject srSecondaryFocus: JsonObject {
                property string label: "Secondary Focus"
                property list<var> gradient: [["overSecondaryContainer", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "secondary"
                property string halftoneBackgroundColor: "overSecondaryContainer"
                property list<var> border: ["overBackground", 0]
                property string itemColor: "overSecondary"
                property real opacity: 1.0
            }

            property JsonObject srOverSecondary: JsonObject {
                property string label: "Over Secondary"
                property list<var> gradient: [["overSecondary", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "secondaryContainer"
                property string halftoneBackgroundColor: "overSecondary"
                property list<var> border: ["overSecondary", 0]
                property string itemColor: "secondary"
                property real opacity: 1.0
            }

            property JsonObject srTertiary: JsonObject {
                property string label: "Tertiary"
                property list<var> gradient: [["tertiary", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "overTertiaryContainer"
                property string halftoneBackgroundColor: "tertiary"
                property list<var> border: ["tertiary", 0]
                property string itemColor: "overTertiary"
                property real opacity: 1.0
            }

            property JsonObject srTertiaryFocus: JsonObject {
                property string label: "Tertiary Focus"
                property list<var> gradient: [["overTertiaryContainer", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "tertiary"
                property string halftoneBackgroundColor: "overTertiaryContainer"
                property list<var> border: ["overBackground", 0]
                property string itemColor: "overTertiary"
                property real opacity: 1.0
            }

            property JsonObject srOverTertiary: JsonObject {
                property string label: "Over Tertiary"
                property list<var> gradient: [["overTertiary", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "tertiaryContainer"
                property string halftoneBackgroundColor: "overTertiary"
                property list<var> border: ["overTertiary", 0]
                property string itemColor: "tertiary"
                property real opacity: 1.0
            }

            property JsonObject srError: JsonObject {
                property string label: "Error"
                property list<var> gradient: [["error", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "overErrorContainer"
                property string halftoneBackgroundColor: "error"
                property list<var> border: ["error", 0]
                property string itemColor: "overError"
                property real opacity: 1.0
            }

            property JsonObject srErrorFocus: JsonObject {
                property string label: "Error Focus"
                property list<var> gradient: [["overBackground", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "error"
                property string halftoneBackgroundColor: "overErrorContainer"
                property list<var> border: ["overBackground", 0]
                property string itemColor: "overError"
                property real opacity: 1.0
            }

            property JsonObject srOverError: JsonObject {
                property string label: "Over Error"
                property list<var> gradient: [["overError", 0.0]]
                property string gradientType: "linear"
                property int gradientAngle: 0
                property real gradientCenterX: 0.5
                property real gradientCenterY: 0.5
                property real halftoneDotMin: 0.0
                property real halftoneDotMax: 2.0
                property real halftoneStart: 0.0
                property real halftoneEnd: 1.0
                property string halftoneDotColor: "errorContainer"
                property string halftoneBackgroundColor: "overError"
                property list<var> border: ["overError", 0]
                property string itemColor: "error"
                property real opacity: 1.0
            }
        }
    }

    // ============================================
    // FRAME MODULE
    // ============================================
    FileView {
        id: frameLoader
        path: root.configDir + "/frame.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.frameReady) {
                validateModule("frame", frameLoader, FrameDefaults.data, () => {
                    root.frameReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.frameReady) {
                handleMissingConfig("frame", frameLoader, FrameDefaults.data, () => {
                    root.frameReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("frame", frameLoader)

        adapter: JsonAdapter {
            property bool enabled: false
            property int thickness: 6
        }
    }

    // ============================================
    // WORKSPACES MODULE
    // ============================================
    FileView {
        id: workspacesLoader
        path: root.configDir + "/workspaces.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.workspacesReady) {
                validateModule("workspaces", workspacesLoader, WorkspacesDefaults.data, () => {
                    root.workspacesReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.workspacesReady) {
                handleMissingConfig("workspaces", workspacesLoader, WorkspacesDefaults.data, () => {
                    root.workspacesReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("workspaces", workspacesLoader)

        adapter: JsonAdapter {
            property int shown: 9
            property bool showAppIcons: false
            property bool alwaysShowNumbers: false
            property bool showNumbers: false
            property bool dynamic: false
        }
    }

    // ============================================
    // OVERVIEW MODULE
    // ============================================
    FileView {
        id: overviewLoader
        path: root.configDir + "/overview.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.overviewReady) {
                validateModule("overview", overviewLoader, OverviewDefaults.data, () => {
                    root.overviewReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.overviewReady) {
                handleMissingConfig("overview", overviewLoader, OverviewDefaults.data, () => {
                    root.overviewReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("overview", overviewLoader)

        adapter: JsonAdapter {
            property bool enabled: true
            property int rows: 2
            property int columns: 5
            property real scale: 0.1
            property real workspaceSpacing: 4
            property list<string> screenList: []
        }
    }

    // ============================================
    // NOTCH MODULE
    // ============================================
    FileView {
        id: notchLoader
        path: root.configDir + "/notch.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.notchReady) {
                validateModule("notch", notchLoader, NotchDefaults.data, () => {
                    root.notchReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.notchReady) {
                handleMissingConfig("notch", notchLoader, NotchDefaults.data, () => {
                    root.notchReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("notch", notchLoader)

        adapter: JsonAdapter {
            property string theme: "default"
            property string position: "top"
            property int hoverRegionHeight: 16
            property bool keepHidden: true
            property bool disableHoverExpansion: false
            property bool alwaysVisible: false
            property bool availableOnFullscreen: true
            property bool reserveSpace: false
            property string logoIcon: ""
            property bool logoTint: false
            property bool logoFullTint: false
            property int logoSize: 23
            property bool use12hFormat: false
        }
    }

    // ============================================
    // COMPOSITOR MODULE
    // ============================================
    FileView {
        id: compositorLoader
        path: root.configDir + "/compositor.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.compositorReady) {
                validateModule("compositor", compositorLoader, CompositorDefaults.data, () => {
                    root.compositorReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.compositorReady) {
                handleMissingConfig("compositor", compositorLoader, CompositorDefaults.data, () => {
                    root.compositorReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("compositor", compositorLoader)

        adapter: JsonAdapter {
            property var activeBorderColor: ["primary"]
            property int borderAngle: 45
            property var inactiveBorderColor: ["surface"]
            property int inactiveBorderAngle: 45
            property int borderSize: 2
            property int rounding: 16
            property bool syncRoundness: true
            property bool syncBorderWidth: false
            property bool syncBorderColor: false
            property bool syncShadowOpacity: false
            property bool syncShadowColor: false
            property int gapsIn: 2
            property int gapsOut: 4
            property string layout: "dwindle"
            property bool shadowEnabled: true
            property int shadowRange: 8
            property int shadowRenderPower: 3
            property bool shadowSharp: false
            property bool shadowIgnoreWindow: true
            property string shadowColor: "shadow"
            property string shadowColorInactive: "shadow"
            property real shadowOpacity: 0.5
            property string shadowOffset: "0 0"
            property real shadowScale: 1.0
            property bool blurEnabled: true
            property int blurSize: 4
            property int blurPasses: 2
            property bool blurIgnoreOpacity: true
            property bool blurExplicitIgnoreAlpha: false
            property real blurIgnoreAlphaValue: 0.2
            property bool blurNewOptimizations: true
            property bool blurXray: false
            property real blurNoise: 0.0
            property real blurContrast: 1.0
            property real blurBrightness: 1.0
            property real blurVibrancy: 0.0
            property real blurVibrancyDarkness: 0.0
            property bool blurSpecial: true
            property bool blurPopups: false
            property real blurPopupsIgnorealpha: 0.2
            property bool blurInputMethods: false
            property real blurInputMethodsIgnorealpha: 0.2
        }
    }

    // ============================================
    // MEDIA MODULE
    // ============================================
    FileView {
        id: mediaLoader
        path: root.configDir + "/media.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.mediaReady) {
                validateModule("media", mediaLoader, MediaDefaults.data, () => {
                    root.mediaReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.mediaReady) {
                handleMissingConfig("media", mediaLoader, MediaDefaults.data, () => {
                    root.mediaReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("media", mediaLoader)

        adapter: JsonAdapter {
            // Publishes artwork, but registers a player per tab.
            property bool enableFirefoxPlayer: false
            property bool coverArtEmbedded: true
            property bool coverArtOnline: false
            property string coverArtFallback: "appIcon"  // appIcon | wallpaper | none
        }
    }

    // ============================================
    // PERFORMANCE MODULE
    // ============================================
    FileView {
        id: performanceLoader
        path: root.configDir + "/performance.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.performanceReady) {
                validateModule("performance", performanceLoader, PerformanceDefaults.data, () => {
                    root.performanceReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.performanceReady) {
                handleMissingConfig("performance", performanceLoader, PerformanceDefaults.data, () => {
                    root.performanceReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("performance", performanceLoader)

        adapter: JsonAdapter {
            property bool blurTransition: true
            property bool windowPreview: true
            property bool wavyLine: true
            property bool rotateCoverArt: true
            property bool dashboardPersistTabs: true
            property int dashboardMaxPersistentTabs: 2
        }
    }

    // ============================================
    // WEATHER MODULE
    // ============================================
    FileView {
        id: weatherLoader
        path: root.configDir + "/weather.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.weatherReady) {
                validateModule("weather", weatherLoader, WeatherDefaults.data, () => {
                    root.weatherReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.weatherReady) {
                handleMissingConfig("weather", weatherLoader, WeatherDefaults.data, () => {
                    root.weatherReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("weather", weatherLoader)

        adapter: JsonAdapter {
            property string location: ""
            property string unit: "C"
        }
    }

    // ============================================
    // DESKTOP MODULE
    // ============================================
    FileView {
        id: desktopLoader
        path: root.configDir + "/desktop.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.desktopReady) {
                validateModule("desktop", desktopLoader, DesktopDefaults.data, () => {
                    root.desktopReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.desktopReady) {
                handleMissingConfig("desktop", desktopLoader, DesktopDefaults.data, () => {
                    root.desktopReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("desktop", desktopLoader)

        adapter: JsonAdapter {
            property bool enabled: false
            property int iconSize: 40
            property int spacingVertical: 16
            property string textColor: "overBackground"
        }
    }

    // ============================================
    // LOCKSCREEN MODULE
    // ============================================
    FileView {
        id: lockscreenLoader
        path: root.configDir + "/lockscreen.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.lockscreenReady) {
                validateModule("lockscreen", lockscreenLoader, LockscreenDefaults.data, () => {
                    root.lockscreenReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.lockscreenReady) {
                handleMissingConfig("lockscreen", lockscreenLoader, LockscreenDefaults.data, () => {
                    root.lockscreenReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("lockscreen", lockscreenLoader)

        adapter: JsonAdapter {
            property string position: "bottom"
        }
    }

    // ============================================
    // PREFIX MODULE
    // ============================================
    FileView {
        id: prefixLoader
        path: root.configDir + "/prefix.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.prefixReady) {
                validateModule("prefix", prefixLoader, PrefixDefaults.data, () => {
                    root.prefixReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.prefixReady) {
                handleMissingConfig("prefix", prefixLoader, PrefixDefaults.data, () => {
                    root.prefixReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("prefix", prefixLoader)

        adapter: JsonAdapter {
            property string clipboard: "cc"
            property string emoji: "ee"
            property string tmux: "tt"
            property string wallpapers: "ww"
            property string notes: "nn"
        }
    }

    // ============================================
    // SYSTEM MODULE
    // ============================================
    FileView {
        id: systemLoader
        path: root.configDir + "/system.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.systemReady) {
                validateModule("system", systemLoader, SystemDefaults.data, () => {
                    root.systemReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.systemReady) {
                handleMissingConfig("system", systemLoader, SystemDefaults.data, () => {
                    root.systemReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("system", systemLoader)

        adapter: JsonAdapter {
            property list<string> disks: ["/"]
            property bool updateServiceEnabled: true
            property JsonObject idle: JsonObject {
                property JsonObject general: JsonObject {
                    property string lock_cmd: "flok lock"
                    property string before_sleep_cmd: "loginctl lock-session"
                    property string after_sleep_cmd: "flok screen on"
                }
                property list<var> listeners: [
                    {
                        "timeout": 150,
                        "onTimeout": "flok brightness 10 -s",
                        "onResume": "flok brightness -r"
                    },
                    {
                        "timeout": 300,
                        "onTimeout": "loginctl lock-session"
                    },
                    {
                        "timeout": 330,
                        "onTimeout": "flok screen off",
                        "onResume": "flok screen on"
                    },
                    {
                        "timeout": 1800,
                        "onTimeout": "flok suspend"
                    }
                ]
            }
            property JsonObject ocr: JsonObject {
                property bool eng: true
                property bool spa: false
                property bool lat: false
                property bool jpn: false
                property bool chi_sim: false
                property bool chi_tra: false
                property bool kor: false
            }
            property JsonObject pomodoro: JsonObject {
                property int workTime: 1800
                property int restTime: 300
                property bool autoStart: false
                property bool syncSpotify: false
            }
        }
    }

    // ============================================
    // DOCK MODULE
    // ============================================
    FileView {
        id: dockLoader
        path: root.configDir + "/dock.json"
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.dockReady) {
                validateModule("dock", dockLoader, DockDefaults.data, () => {
                    root.dockReady = true;
                });
            }
        }
        onLoadFailed: {
            if (error === FileViewError.FileNotFound && !root.dockReady) {
                handleMissingConfig("dock", dockLoader, DockDefaults.data, () => {
                    root.dockReady = true;
                });
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("dock", dockLoader)

        adapter: JsonAdapter {
            property bool enabled: false
            property string theme: "default"
            property string position: "bottom"
            property int height: 48
            property int iconSize: 24
            property int spacing: 4
            property int margin: 4
            property int hoverRegionHeight: 16
            property bool pinnedOnStartup: false
            property bool hoverToReveal: true
            property bool availableOnFullscreen: true
            property bool showRunningIndicators: true
            property bool showPinButton: true
            property bool showOverviewButton: true
            property list<string> ignoredAppRegexes: ["quickshell.*", "xdg-desktop-portal.*"]
            property list<string> screenList: []
            property bool keepHidden: false
        }
    }

    // Pinned apps (per-user)
    property bool pinnedAppsReady: false

    FileView {
        id: pinnedAppsLoader
        path: Quickshell.dataPath("pinnedapps.json")
        atomicWrites: true
        watchChanges: true
        onLoaded: {
            if (!root.pinnedAppsReady) {
                var raw = text();
                if (!raw || raw.trim().length === 0) {
                    console.log("pinnedapps.json not found, creating with default values...");
                    pinnedAppsLoader.writeAdapter();
                }
                root.pinnedAppsReady = true;
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            root.pauseAutoSave = false;
        }
        onPathChanged: reload()
        onAdapterUpdated: root.queueWrite("pinnedApps", pinnedAppsLoader)

        adapter: JsonAdapter {
            property list<string> apps: ["kitty"]
        }
    }



    // Keybinds (binds.json)
    // Timer to repair keybinds after initial load
    Timer {
        id: repairKeybindsTimer
        interval: 500
        repeat: false
        onTriggered: {
            repairKeybinds();
        }
    }


    // Timer to create binds.json if missing after initial load
    Timer {
        id: createKeybindsTimer
        interval: 1000
        repeat: false
        onTriggered: {
            const raw = keybindsLoader.text();
            if (!raw || raw.trim().length === 0) {
                console.log("binds.json still missing after delay, creating...");
                keybindsLoader.writeAdapter();
                repairKeybindsTimer.start();
            }
        }
    }
    // Repair missing binds
    function repairKeybinds() {
        const raw = keybindsLoader.text();
        if (!raw) return;

        try {
            const current = JSON.parse(raw);
            let needsUpdate = false;

            // Ensure flokshell structure exists
            if (!current.flokshell) {
                current.flokshell = {};
                needsUpdate = true;
            }

            // Migrate nested to flat structure
            if (current.flokshell.dashboard && typeof current.flokshell.dashboard === "object" && !current.flokshell.dashboard.modifiers) {
                console.log("Migrating nested flokshell binds to flat structure...");
                const nested = current.flokshell.dashboard;
                
                // Map old names to new names and update arguments
                if (nested.widgets) {
                    current.flokshell.launcher = nested.widgets;
                    current.flokshell.launcher.argument = "flok run launcher";
                    current.flokshell.launcher.action = createAction(current.flokshell.launcher);
                }
                if (nested.dashboard) {
                    current.flokshell.dashboard = nested.dashboard;
                    current.flokshell.dashboard.argument = "flok run dashboard";
                    current.flokshell.dashboard.action = createAction(current.flokshell.dashboard);
                }
                if (nested.assistant) {
                    current.flokshell.assistant = nested.assistant;
                    current.flokshell.assistant.argument = "flok run assistant";
                    current.flokshell.assistant.action = createAction(current.flokshell.assistant);
                }
                if (nested.clipboard) {
                    current.flokshell.clipboard = nested.clipboard;
                    current.flokshell.clipboard.argument = "flok run clipboard";
                    current.flokshell.clipboard.action = createAction(current.flokshell.clipboard);
                }
                if (nested.emoji) {
                    current.flokshell.emoji = nested.emoji;
                    current.flokshell.emoji.argument = "flok run emoji";
                    current.flokshell.emoji.action = createAction(current.flokshell.emoji);
                }
                if (nested.notes) {
                    current.flokshell.notes = nested.notes;
                    current.flokshell.notes.argument = "flok run notes";
                    current.flokshell.notes.action = createAction(current.flokshell.notes);
                }
                if (nested.tmux) {
                    current.flokshell.tmux = nested.tmux;
                    current.flokshell.tmux.argument = "flok run tmux";
                    current.flokshell.tmux.action = createAction(current.flokshell.tmux);
                }
                if (nested.wallpapers) {
                    current.flokshell.wallpapers = nested.wallpapers;
                    current.flokshell.wallpapers.argument = "flok run wallpapers";
                    current.flokshell.wallpapers.action = createAction(current.flokshell.wallpapers);
                }

                // Remove the old nested object
                delete current.flokshell.dashboard;
                needsUpdate = true;
            }

            if (!current.flokshell.system) {
                current.flokshell.system = {};
                needsUpdate = true;
            }

            // Get default binds from adapter
            const adapter = keybindsLoader.adapter;
            if (!adapter || !adapter.flokshell) return;

            // Helper function to create clean bind object
            function createAction(bindObj) {
                if (bindObj && bindObj.action) {
                    return KeybindActions.ensureAction(bindObj.action);
                }
                return KeybindActions.actionFromLegacy(bindObj.dispatcher || "", bindObj.argument || "", bindObj.flags || "");
            }

            function createCleanBind(bindObj) {
                return {
                    "modifiers": bindObj.modifiers || [],
                    "key": bindObj.key || "",
                    "action": createAction(bindObj)
                };
            }

            // Check flokshell core binds
            const flokshellKeys = ["launcher", "dashboard", "clipboard", "emoji", "notes", "tmux", "wallpapers"];
            for (const key of flokshellKeys) {
                if (!current.flokshell[key] && adapter.flokshell[key]) {
                    console.log("Adding missing flokshell bind:", key);
                    current.flokshell[key] = createCleanBind(adapter.flokshell[key]);
                    needsUpdate = true;
                } else if (current.flokshell[key] && !current.flokshell[key].action) {
                    current.flokshell[key].action = createAction(current.flokshell[key]);
                    delete current.flokshell[key].dispatcher;
                    delete current.flokshell[key].argument;
                    delete current.flokshell[key].flags;
                    needsUpdate = true;
                }
            }

            // Check system binds
            const systemKeys = ["overview", "powermenu", "config", "lockscreen", "tools", "screenshot", "screenrecord", "lens", "reload", "quit"];
            for (const key of systemKeys) {
                if (!current.flokshell.system[key] && adapter.flokshell.system && adapter.flokshell.system[key]) {
                    console.log("Adding missing system bind:", key);
                    current.flokshell.system[key] = createCleanBind(adapter.flokshell.system[key]);
                    needsUpdate = true;
                } else if (current.flokshell.system[key] && !current.flokshell.system[key].action) {
                    current.flokshell.system[key].action = createAction(current.flokshell.system[key]);
                    delete current.flokshell.system[key].dispatcher;
                    delete current.flokshell.system[key].argument;
                    delete current.flokshell.system[key].flags;
                    needsUpdate = true;
                }
            }

            if (current.custom && current.custom.length > 0) {
                const normalized = KeybindActions.normalizeCustomBinds(current.custom);
                if (normalized.changed) {
                    current.custom = normalized.binds;
                    needsUpdate = true;
                }
            }

            if (needsUpdate) {
                console.log("Auto-repairing binds.json: adding missing binds");
                keybindsLoader.setText(JSON.stringify(current, null, 2));
            }
        } catch (e) {
            console.warn("Failed to repair binds.json:", e);
        }
    }

    FileView {
        id: keybindsLoader
        path: keybindsPath
        atomicWrites: true
        watchChanges: true
        Component.onCompleted: {
            // Ensure binds.json is created even if onLoaded never fires
            createKeybindsTimer.start();
        }
        onLoaded: {
            if (!root.keybindsInitialLoadComplete) {
                var raw = text();
                if (!raw || raw.trim().length === 0) {
                    console.log("binds.json not found, creating with default values...");
                    keybindsLoader.writeAdapter();
                    repairKeybindsTimer.start();
                } else {
                    // File exists, check if it needs repair
                    repairKeybindsTimer.start();
                }
                root.keybindsInitialLoadComplete = true;
                createKeybindsTimer.start();
            }
        }
        onFileChanged: {
            root.pauseAutoSave = true;
            reload();
            normalizeCustomBinds();
            root.pauseAutoSave = false;
        }
        onPathChanged: {
            reload();
            normalizeCustomBinds();
        }
        onAdapterUpdated: {
            if (root.keybindsInitialLoadComplete) {
                root.queueWrite("keybinds", keybindsLoader, true);
            }
        }

        // Normalize custom binds
        function normalizeCustomBinds() {
            if (!adapter || !adapter.custom)
                return;

            const normalized = KeybindActions.normalizeCustomBinds(adapter.custom);
            if (normalized.changed) {
                console.log("Normalizing custom binds: migrating to action format");
                adapter.custom = normalized.binds;
            }
        }

        adapter: JsonAdapter {
            property JsonObject flokshell: JsonObject {
                property JsonObject launcher: JsonObject {
                    property list<string> modifiers: ["SUPER"]
                    property string key: "Super_L"
                property var action: ({ "id": "flokshell.launcher", "args": {} })
            }
            property JsonObject dashboard: JsonObject {
                property list<string> modifiers: ["SUPER"]
                property string key: "D"
                property var action: ({ "id": "flokshell.dashboard", "args": {} })
            }

            property JsonObject clipboard: JsonObject {
                property list<string> modifiers: ["SUPER"]
                property string key: "V"
                property var action: ({ "id": "flokshell.clipboard", "args": {} })
            }
            property JsonObject emoji: JsonObject {
                property list<string> modifiers: ["SUPER"]
                property string key: "PERIOD"
                property var action: ({ "id": "flokshell.emoji", "args": {} })
            }
            property JsonObject notes: JsonObject {
                property list<string> modifiers: ["SUPER"]
                property string key: "N"
                property var action: ({ "id": "flokshell.notes", "args": {} })
            }
            property JsonObject tmux: JsonObject {
                property list<string> modifiers: ["SUPER"]
                property string key: "T"
                property var action: ({ "id": "flokshell.tmux", "args": {} })
            }
            property JsonObject wallpapers: JsonObject {
                property list<string> modifiers: ["SUPER"]
                property string key: "COMMA"
                property var action: ({ "id": "flokshell.wallpapers", "args": {} })
            }
            property JsonObject system: JsonObject {
                property JsonObject config: JsonObject {
                    property list<string> modifiers: ["SUPER", "SHIFT"]
                    property string key: "C"
                    property var action: ({ "id": "flokshell.config", "args": {} })
                }
                property JsonObject lockscreen: JsonObject {
                    property list<string> modifiers: ["SUPER"]
                    property string key: "L"
                    property var action: ({ "id": "system.lock", "args": {} })
                }
                property JsonObject overview: JsonObject {
                    property list<string> modifiers: ["SUPER"]
                    property string key: "TAB"
                    property var action: ({ "id": "flokshell.overview", "args": {} })
                }
                property JsonObject powermenu: JsonObject {
                    property list<string> modifiers: ["SUPER"]
                    property string key: "ESCAPE"
                    property var action: ({ "id": "flokshell.powermenu", "args": {} })
                }
                property JsonObject tools: JsonObject {
                    property list<string> modifiers: ["SUPER"]
                    property string key: "S"
                    property var action: ({ "id": "flokshell.tools", "args": {} })
                }
                property JsonObject screenshot: JsonObject {
                    property list<string> modifiers: ["SUPER", "SHIFT"]
                    property string key: "S"
                    property var action: ({ "id": "flokshell.screenshot", "args": {} })
                }
                property JsonObject screenrecord: JsonObject {
                    property list<string> modifiers: ["SUPER", "SHIFT"]
                    property string key: "R"
                    property var action: ({ "id": "flokshell.screenrecord", "args": {} })
                }
                property JsonObject lens: JsonObject {
                    property list<string> modifiers: ["SUPER", "SHIFT"]
                    property string key: "A"
                    property var action: ({ "id": "flokshell.lens", "args": {} })
                }
                property JsonObject reload: JsonObject {
                    property list<string> modifiers: ["SUPER", "ALT"]
                    property string key: "B"
                    property var action: ({ "id": "flokshell.reload", "args": {} })
                }
                property JsonObject quit: JsonObject {
                    property list<string> modifiers: ["SUPER", "CTRL", "ALT"]
                    property string key: "B"
                    property var action: ({ "id": "flokshell.quit", "args": {} })
                }
            }
            }
            // Default getters
            readonly property var defaultFlokshellBinds: {
                "flokshell": {
                    "launcher": { "modifiers": ["SUPER"], "key": "Super_L", "action": { "id": "flokshell.launcher", "args": {} } },
                    "dashboard": { "modifiers": ["SUPER"], "key": "D", "action": { "id": "flokshell.dashboard", "args": {} } },
                    "clipboard": { "modifiers": ["SUPER"], "key": "V", "action": { "id": "flokshell.clipboard", "args": {} } },
                    "emoji": { "modifiers": ["SUPER"], "key": "PERIOD", "action": { "id": "flokshell.emoji", "args": {} } },
                    "notes": { "modifiers": ["SUPER"], "key": "N", "action": { "id": "flokshell.notes", "args": {} } },
                    "tmux": { "modifiers": ["SUPER"], "key": "T", "action": { "id": "flokshell.tmux", "args": {} } },
                    "wallpapers": { "modifiers": ["SUPER"], "key": "COMMA", "action": { "id": "flokshell.wallpapers", "args": {} } }
                },
                "system": {
                    "config": { "modifiers": ["SUPER", "SHIFT"], "key": "C", "action": { "id": "flokshell.config", "args": {} } },
                    "lockscreen": { "modifiers": ["SUPER"], "key": "L", "action": { "id": "system.lock", "args": {} } },
                    "overview": { "modifiers": ["SUPER"], "key": "TAB", "action": { "id": "flokshell.overview", "args": {} } },
                    "powermenu": { "modifiers": ["SUPER"], "key": "ESCAPE", "action": { "id": "flokshell.powermenu", "args": {} } },
                    "tools": { "modifiers": ["SUPER"], "key": "S", "action": { "id": "flokshell.tools", "args": {} } },
                    "screenshot": { "modifiers": ["SUPER", "SHIFT"], "key": "S", "action": { "id": "flokshell.screenshot", "args": {} } },
                    "screenrecord": { "modifiers": ["SUPER", "SHIFT"], "key": "R", "action": { "id": "flokshell.screenrecord", "args": {} } },
                    "lens": { "modifiers": ["SUPER", "SHIFT"], "key": "A", "action": { "id": "flokshell.lens", "args": {} } },
                    "reload": { "modifiers": ["SUPER", "ALT"], "key": "B", "action": { "id": "flokshell.reload", "args": {} } },
                    "quit": { "modifiers": ["SUPER", "CTRL", "ALT"], "key": "B", "action": { "id": "flokshell.quit", "args": {} } }
                }
            }

            function getFlokshellDefault(section, key) {
                if (defaultFlokshellBinds[section] && defaultFlokshellBinds[section][key]) {
                    const bind = defaultFlokshellBinds[section][key];
                    return {
                        "modifiers": bind.modifiers || [],
                        "key": bind.key || "",
                        "action": KeybindActions.ensureAction(bind.action)
                    };
                }
                return null;
            }

            property list<var> custom: [
                {
                    "name": "Close Window",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "C"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "killactive",
                            "argument": "",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Workspace 1",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "1"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Workspace 2",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "2"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "2",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Workspace 3",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "3"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "3",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Workspace 4",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "4"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "4",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Workspace 5",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "5"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "5",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Workspace 6",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "6"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "6",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Workspace 7",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "7"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "7",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Workspace 8",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "8"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "8",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Workspace 9",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "9"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "9",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Workspace 10",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "0"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "10",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Workspace 1",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "1"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Workspace 2",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "2"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "2",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Workspace 3",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "3"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "3",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Workspace 4",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "4"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "4",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Workspace 5",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "5"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "5",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Workspace 6",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "6"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "6",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Workspace 7",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "7"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "7",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Workspace 8",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "8"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "8",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Workspace 9",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "9"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "9",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Workspace 10",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "0"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "10",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Silently to Workspace 1",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "1"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspacesilent",
                            "argument": "1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Silently to Workspace 2",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "2"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspacesilent",
                            "argument": "2",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Silently to Workspace 3",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "3"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspacesilent",
                            "argument": "3",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Silently to Workspace 4",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "4"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspacesilent",
                            "argument": "4",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Silently to Workspace 5",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "5"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspacesilent",
                            "argument": "5",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Silently to Workspace 6",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "6"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspacesilent",
                            "argument": "6",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Silently to Workspace 7",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "7"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspacesilent",
                            "argument": "7",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Silently to Workspace 8",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "8"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspacesilent",
                            "argument": "8",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Silently to Workspace 9",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "9"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspacesilent",
                            "argument": "9",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Silently to Workspace 10",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "0"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspacesilent",
                            "argument": "10",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Switch Occupied Workspace -1",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "mouse_down"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "e-1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Switch Occupied Workspace +1",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "mouse_up"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "e+1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Switch Occupied Workspace -1",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "Z"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "e-1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Switch Occupied Workspace +1",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "X"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "e+1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Switch Relative Workspace -1",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "Z"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "-1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Switch Relative Workspace +1",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "X"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "workspace",
                            "argument": "+1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Drag Window",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "mouse:272"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movewindow",
                            "argument": "",
                            "flags": "m",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Resize Window with Mouse",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "mouse:273"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "resizewindow",
                            "argument": "",
                            "flags": "m",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Media Play Pause",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86AudioPlay"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "playerctl play-pause",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Media Previous",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86AudioPrev"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "playerctl previous",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Media Next",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86AudioNext"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "playerctl next",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Media Play Pause",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86AudioMedia"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "playerctl play-pause",
                            "flags": "l",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Media Stop",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86AudioStop"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "playerctl stop",
                            "flags": "l",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Volume Up",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86AudioRaiseVolume"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "wpctl set-volume -l 1 @DEFAULT_AUDIO_SINK@ 10%+",
                            "flags": "le",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Volume Down",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86AudioLowerVolume"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "wpctl set-volume -l 1 @DEFAULT_AUDIO_SINK@ 10%-",
                            "flags": "le",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Mute Audio",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86AudioMute"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle",
                            "flags": "le",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Brightness Up",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86MonBrightnessUp"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "flok brightness +5",
                            "flags": "le",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Brightness Down",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86MonBrightnessDown"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "flok brightness -5",
                            "flags": "le",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Calculator",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "XF86Calculator"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "notify-send \"Soon\"",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Toggle Special Workspace",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "V"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "togglespecialworkspace",
                            "argument": "",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window to Special Workspace",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "V"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movetoworkspace",
                            "argument": "special",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Lock Session on Lid Switch",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "switch:Lid Switch"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "loginctl lock-session",
                            "flags": "l",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Display Off on Lid Close",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "switch:on:Lid Switch"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "flok screen off",
                            "flags": "l",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Display On on Lid Open",
                    "keys": [
                        {
                            "modifiers": [],
                            "key": "switch:off:Lid Switch"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "exec",
                            "argument": "flok screen on",
                            "flags": "l",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Focus Up",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "Up"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movefocus",
                            "argument": "u",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Focus Up",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL"],
                            "key": "k"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movefocus",
                            "argument": "u",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Focus Down",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "Down"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movefocus",
                            "argument": "d",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Focus Down",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL"],
                            "key": "j"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movefocus",
                            "argument": "d",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Focus Left",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "Left"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movefocus",
                            "argument": "l",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Focus Left",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL"],
                            "key": "z"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movefocus",
                            "argument": "l",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Focus Left",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL"],
                            "key": "h"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movefocus",
                            "argument": "l",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Focus Right",
                    "keys": [
                        {
                            "modifiers": ["SUPER"],
                            "key": "Right"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movefocus",
                            "argument": "r",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Focus Right",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL"],
                            "key": "x"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movefocus",
                            "argument": "r",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Focus Right",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL"],
                            "key": "l"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movefocus",
                            "argument": "r",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Left",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "Left"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movewindow",
                            "argument": "l",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Left",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "h"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movewindow",
                            "argument": "l",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Right",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "Right"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movewindow",
                            "argument": "r",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Right",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "l"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movewindow",
                            "argument": "r",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Up",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "Up"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movewindow",
                            "argument": "u",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Up",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "k"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movewindow",
                            "argument": "u",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Down",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "Down"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movewindow",
                            "argument": "d",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Window Down",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "j"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "movewindow",
                            "argument": "d",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Resize Column +0.1",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "Right"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "colresize +0.1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Resize Column +0.1",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "l"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "colresize +0.1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Resize Column -0.1",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "Left"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "colresize -0.1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Resize Column -0.1",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "h"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "colresize -0.1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Resize Active 0 50",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "Down"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "resizeactive",
                            "argument": "0 50",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Resize Active 0 50",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "j"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "resizeactive",
                            "argument": "0 50",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Resize Active 0 -50",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "Up"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "resizeactive",
                            "argument": "0 -50",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Resize Active 0 -50",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "k"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "resizeactive",
                            "argument": "0 -50",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Promote Column",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT"],
                            "key": "SPACE"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "promote",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Toggle Fit",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL"],
                            "key": "SPACE"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "togglefit",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Resize Column +conf",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "SHIFT"],
                            "key": "SPACE"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "colresize +conf",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Swap Column Left",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT", "CTRL"],
                            "key": "Left"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "swapcol l",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Swap Column Left",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT", "CTRL"],
                            "key": "h"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "swapcol l",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Swap Column Right",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT", "CTRL"],
                            "key": "Right"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "swapcol r",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Swap Column Right",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "ALT", "CTRL"],
                            "key": "l"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "swapcol r",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Column to Workspace 1",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL", "ALT"],
                            "key": "1"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "movecoltoworkspace 1",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Column to Workspace 2",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL", "ALT"],
                            "key": "2"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "movecoltoworkspace 2",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Column to Workspace 3",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL", "ALT"],
                            "key": "3"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "movecoltoworkspace 3",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Column to Workspace 4",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL", "ALT"],
                            "key": "4"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "movecoltoworkspace 4",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Column to Workspace 5",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL", "ALT"],
                            "key": "5"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "movecoltoworkspace 5",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Column to Workspace 6",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL", "ALT"],
                            "key": "6"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "movecoltoworkspace 6",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Column to Workspace 7",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL", "ALT"],
                            "key": "7"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "movecoltoworkspace 7",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Column to Workspace 8",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL", "ALT"],
                            "key": "8"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "movecoltoworkspace 8",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Column to Workspace 9",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL", "ALT"],
                            "key": "9"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "movecoltoworkspace 9",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                },
                {
                    "name": "Move Column to Workspace 10",
                    "keys": [
                        {
                            "modifiers": ["SUPER", "CTRL", "ALT"],
                            "key": "0"
                        }
                    ],
                    "actions": [
                        {
                            "dispatcher": "layoutmsg",
                            "argument": "movecoltoworkspace 10",
                            "flags": "",
                            "layouts": []
                        }
                    ],
                    "enabled": true
                }
            ]
        }
    }

    // Adapters emit one update per property; coalesce into one write per module.
    property var pendingWrites: ({})

    function queueWrite(name, loader, force) {
        if (!force && (!root[name + "Ready"] || root.pauseAutoSave))
            return;
        root.pendingWrites[name] = loader;
        writeDebounce.restart();
    }

    function flushWrites() {
        for (const name in root.pendingWrites) {
            root.pendingWrites[name].writeAdapter();
        }
        root.pendingWrites = ({});
    }

    Timer {
        id: writeDebounce
        interval: 250
        repeat: false
        onTriggered: root.flushWrites()
    }

    // Validation helper
    function validateModule(name, loader, defaults, onComplete) {
        var raw = loader.text();
        if (!raw || raw.trim().length === 0) {
            // File is missing or empty — create with defaults
            console.log(name + ".json missing or empty, creating default...");
            loader.setText(JSON.stringify(defaults, null, 2));
            onComplete();
            return;
        }

        try {
            var current = JSON.parse(raw);
            var validated = ConfigValidator.validate(current, defaults);

            if (JSON.stringify(current) !== JSON.stringify(validated)) {
                console.log("Merging and updating " + name + ".json...");
                loader.setText(JSON.stringify(validated, null, 2));
            }
            onComplete();
        } catch (e) {
            console.log("Error validating " + name + " config (invalid JSON?): " + e);
            console.log("Overwriting with defaults due to error.");
            loader.setText(JSON.stringify(defaults, null, 2));
            onComplete();
        }
    }

    // Handle missing config files - copy from preset or create with defaults
    function handleMissingConfig(name, loader, defaults, onComplete) {
        var presetPath = root.presetDir + "/" + name + ".json";
        var targetPath = root.configDir + "/" + name + ".json";
        console.log(name + ".json not found, checking preset: " + presetPath);

        // Reload after the copy exits; alongside it races the write.
        var copyProcess = Qt.createQmlObject("import Quickshell.Io; Process {}", root, "copyProcess");
        copyProcess.command = ["cp", presetPath, targetPath];
        copyProcess.exited.connect(() => {
            copyProcess.destroy();
            loader.reload();

            // No preset, or the copy failed. The file must exist or queueWrite
            // refuses to save and nothing changed in settings survives.
            Qt.callLater(() => {
                if (!root[name + "Ready"]) {
                    console.log("Using defaults for " + name + ".json");
                    loader.setText(JSON.stringify(defaults, null, 2));
                }
                onComplete();
            });
        });
        copyProcess.running = true;
    }


    // Exposed properties
    // Theme configuration
    property QtObject theme: themeLoader.adapter
    property bool oledMode: lightMode ? false : theme.oledMode
    property bool lightMode: theme.lightMode

    property int roundness: theme.roundness
    property string defaultFont: theme.font
    property int animDuration: Services.GameModeService.toggled ? 0 : theme.animDuration
    property bool tintIcons: theme.tintIcons

    // Handle lightMode changes
    onLightModeChanged: {
        console.log("lightMode changed to:", lightMode);
        if (GlobalStates.wallpaperManager) {
            var wallpaperManager = GlobalStates.wallpaperManager;
            if (wallpaperManager.currentWallpaper) {
                console.log("Re-running Matugen due to lightMode change");
                wallpaperManager.runMatugenForCurrentWallpaper();
            }
        }
    }

    // Bar configuration
    property QtObject frame: frameLoader.adapter
    property bool showBackground: theme.srBarBg.opacity > 0

    // Workspace configuration
    property QtObject workspaces: workspacesLoader.adapter

    // Overview configuration
    property QtObject overview: overviewLoader.adapter

    // Notch configuration
    property QtObject notch: notchLoader.adapter
    property string notchTheme: notch.theme
    property string notchPosition: notch.position

    onNotchPositionChanged: {
        if (!initialLoadComplete || !dockReady) return;

        // If notch moves bottom
        if (notchPosition === "bottom") {
            // Conflict with Dock?
            if (dock.position === "bottom") {
                console.log("Notch moved to bottom, adjusting Dock position...");
                // Offset Dock to avoid notch
                dock.position = "left";
                // Trigger save
                GlobalStates.markShellChanged();
            }
        } 
        // If notch moves top
        else if (notchPosition === "top") {
            // Restore Dock if displaced
            if (dock.position === "left" || dock.position === "right") {
                console.log("Notch moved to top, restoring Dock to bottom...");
                dock.position = "bottom";
                GlobalStates.markShellChanged();
            }
        }
    }

    // Compositor configuration
    property QtObject compositor: compositorLoader.adapter
    property int compositorRounding: compositor.syncRoundness ? roundness : compositor.rounding
    property int compositorBorderSize: compositor.syncBorderWidth ? (theme.srBg.border[1] || 0) : compositor.borderSize
    property string compositorBorderColor: compositor.syncBorderColor ? (theme.srBg.border[0] || "primary") : (compositor.activeBorderColor.length > 0 ? compositor.activeBorderColor[0] : "primary")
    property real compositorShadowOpacity: compositor.syncShadowOpacity ? theme.shadowOpacity : compositor.shadowOpacity
    property string compositorShadowColor: compositor.syncShadowColor ? theme.shadowColor : compositor.shadowColor

    // Performance configuration
    property QtObject performance: performanceLoader.adapter
    property bool blurTransition: performance.blurTransition

    // Weather configuration
    property QtObject weather: weatherLoader.adapter

    // Desktop configuration
    property QtObject desktop: desktopLoader.adapter

    // Lockscreen configuration
    property QtObject lockscreen: lockscreenLoader.adapter

    // Prefix configuration
    property QtObject prefix: prefixLoader.adapter

    // System configuration
    property QtObject system: systemLoader.adapter

    // Dock configuration
    property QtObject dock: dockLoader.adapter

    // Media configuration
    property QtObject media: mediaLoader.adapter

    // Pinned apps configuration (stored in dataPath)
    property QtObject pinnedApps: pinnedAppsLoader.adapter



    // Module save functions
    function saveBar() {
        frameLoader.writeAdapter();
    }
    function saveWorkspaces() {
        workspacesLoader.writeAdapter();
    }
    function saveOverview() {
        overviewLoader.writeAdapter();
    }
    function saveNotch() {
        notchLoader.writeAdapter();
    }
    function saveCompositor() {
        compositorLoader.writeAdapter();
    }
    function savePerformance() {
        performanceLoader.writeAdapter();
    }
    function saveWeather() {
        weatherLoader.writeAdapter();
    }
    function saveDesktop() {
        desktopLoader.writeAdapter();
    }
    function saveLockscreen() {
        lockscreenLoader.writeAdapter();
    }
    function savePrefix() {
        prefixLoader.writeAdapter();
    }
    function saveSystem() {
        systemLoader.writeAdapter();
    }
    function saveDock() {
        dockLoader.writeAdapter();
    }
    function saveMedia() {
        mediaLoader.writeAdapter();
    }
    function savePinnedApps() {
        pinnedAppsLoader.writeAdapter();
    }


    // Color helpers
    function isHexColor(colorValue) {
        if (!colorValue || typeof colorValue !== 'string')
            return false;
        const normalized = colorValue.toLowerCase().trim();
        return normalized.startsWith('#') || normalized.startsWith('rgb');
    }

    function resolveColor(colorValue) {
        if (!colorValue) return "transparent"; // Fallback
        
        if (isHexColor(colorValue)) {
            return colorValue;
        }
        
        // Check Colors singleton
        if (typeof Colors === 'undefined' || !Colors) return "transparent";
        
        return Colors[colorValue] || "transparent"; 
    }

    function resolveColorWithOpacity(colorValue, opacity) {
        if (!colorValue) return Qt.rgba(0,0,0,0);
        
        const color = isHexColor(colorValue) ? Qt.color(colorValue) : (Colors[colorValue] || Qt.color("transparent"));
        return Qt.rgba(color.r, color.g, color.b, opacity);
    }
}
