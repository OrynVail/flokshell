import QtQuick
import Quickshell
import qs.modules.services
import qs.config
import qs.modules.theme
import qs.modules.globals
import "../../config/KeybindActions.js" as KeybindActions

// Turns the shell's compositor settings and binds into what flokd renders as
// ~/.config/hypr/flokshell.{lua,conf}, and holds the live layout choice.
QtObject {
    id: root

    function vertical() {
        const position = Config.notchPosition || "top";
        return position === "left" || position === "right";
    }

    function colorOf(name) {
        const resolved = Config.resolveColor(name);
        return (typeof resolved === 'string') ? Qt.color(resolved) : resolved;
    }

    function hyprColor(c) {
        const hex = v => Math.round(v * 255).toString(16).padStart(2, '0');
        return c.a === 1.0 ? `rgb(${hex(c.r)}${hex(c.g)}${hex(c.b)})` : `rgba(${hex(c.r)}${hex(c.g)}${hex(c.b)}${hex(c.a)})`;
    }

    function opaque(c) {
        return Qt.rgba(c.r, c.g, c.b, 1.0);
    }

    function ignoreAlpha() {
        if (Config.compositor.blurExplicitIgnoreAlpha)
            return Config.compositor.blurIgnoreAlphaValue;
        const bar = (Config.theme.srBarBg && Config.theme.srBarBg.opacity !== undefined) ? Config.theme.srBarBg.opacity : 0;
        const bg = (Config.theme.srBg && Config.theme.srBg.opacity !== undefined) ? Config.theme.srBg.opacity : 1.0;
        return bar > 0 ? Math.min(bar, bg) : bg;
    }

    function binds() {
        let out = [];
        let seen = {};
        if (!Config.keybindsLoader.loaded || !Config.keybindsLoader.adapter)
            return out;
        const adapter = Config.keybindsLoader.adapter;

        function push(modifiers, key, action) {
            const resolved = KeybindActions.resolveAction(action);
            if (!resolved || !key || String(key).trim().length === 0)
                return;
            let dispatcher = resolved.dispatcher || "";
            let arg = resolved.argument || "";
            if (dispatcher === "layoutmsg" && arg.indexOf("focus ") === 0) {
                dispatcher = "movefocus";
                arg = arg.split(" ")[1] || "";
            } else if (dispatcher === "layoutmsg" && arg.indexOf("movewindowto ") === 0) {
                dispatcher = "movewindow";
                arg = arg.split(" ")[1] || "";
            }
            const bind = { mods: modifiers || [], key: String(key), dispatcher: dispatcher, arg: arg, flags: resolved.flags || "" };
            // The same action twice on a key would fire twice per press.
            const id = JSON.stringify(bind);
            if (seen[id])
                return;
            seen[id] = true;
            out.push(bind);
        }

        function core(keybind) {
            if (keybind)
                push(keybind.modifiers, keybind.key, keybind.action || keybind);
        }

        const flokshell = adapter.flokshell;
        if (flokshell) {
            for (const name of ["launcher", "dashboard", "clipboard", "emoji", "notes", "tmux", "wallpapers"])
                core(flokshell[name]);
            if (flokshell.system) {
                for (const name of ["overview", "powermenu", "config", "lockscreen", "tools", "screenshot", "screenrecord", "lens", "reload", "quit"])
                    core(flokshell.system[name]);
            }
        }

        for (const bind of (adapter.custom || [])) {
            if (!bind || bind.enabled === false)
                continue;
            if (bind.keys && bind.actions) {
                for (const k of bind.keys) {
                    if (!k || !k.key)
                        continue;
                    for (const action of bind.actions) {
                        if (action && action.layouts && action.layouts.length > 0 && action.layouts.indexOf(GlobalStates.compositorLayout) === -1)
                            continue;
                        push(k.modifiers, k.key, action);
                    }
                }
            } else {
                push(bind.modifiers, bind.key, bind.action || bind);
            }
        }
        return out;
    }

    function payload() {
        const active = Config.compositor.syncBorderColor ? [Config.compositorBorderColor] : (Config.compositor.activeBorderColor || ["primary"]);
        const inactive = Config.compositor.inactiveBorderColor || ["surface"];
        const shadow = colorOf(Config.compositorShadowColor);
        const alpha = ignoreAlpha();
        const shell = { blur: true, blurPopups: true, noAnim: true, ignoreAlpha: alpha };
        return {
            gapsIn: Config.compositor.gapsIn,
            gapsOut: Config.compositor.gapsOut,
            borderSize: Config.compositorBorderSize,
            activeBorder: active.map(n => hyprColor(colorOf(n))),
            activeAngle: Config.compositor.borderAngle,
            inactiveBorder: inactive.map(n => hyprColor(opaque(colorOf(n)))),
            inactiveAngle: Config.compositor.inactiveBorderAngle,
            rounding: Config.compositorRounding,
            blur: { enabled: Config.compositor.blurEnabled, size: Config.compositor.blurSize, passes: Config.compositor.blurPasses },
            shadow: {
                enabled: Config.compositor.shadowEnabled,
                range: Config.compositor.shadowRange,
                color: hyprColor(Qt.rgba(shadow.r, shadow.g, shadow.b, shadow.a * Config.compositorShadowOpacity))
            },
            layout: GlobalStates.compositorLayout || "",
            workspaceStyle: vertical() ? "slidefadevert 20%" : "slidefade 20%",
            binds: binds(),
            layerRules: [
                Object.assign({ namespace: "flokshell" }, shell),
                Object.assign({ namespace: "flokshell:overview" }, shell),
                { namespace: "quickshell:notificationPopup", blur: true, noAnim: true, ignoreAlpha: alpha }
            ],
            execOnce: ["flok"]
        };
    }

    function send() {
        if (Config.loader.loaded)
            flokd.send("config", payload());
        if (GlobalStates.compositorLayoutReady && GlobalStates.compositorLayout)
            flokd.send("layout", { name: GlobalStates.compositorLayout });
    }

    function applyCompositorConfig() {
        sendTimer.restart();
    }

    property Timer sendTimer: Timer {
        interval: 100
        onTriggered: root.send()
    }

    property FlokdSocket flokd: FlokdSocket {
        service: "compositor"
        onConnectionStateChanged: {
            if (connected)
                root.applyCompositorConfig();
        }
    }

    property Connections configConnections: Connections {
        target: Config.loader
        function onLoaded() { root.applyCompositorConfig(); }
        function onFileChanged() { root.applyCompositorConfig(); }
    }

    property Connections keybindsConnections: Connections {
        target: Config.keybindsLoader
        function onLoaded() { root.applyCompositorConfig(); }
        function onFileChanged() { root.applyCompositorConfig(); }
        function onAdapterUpdated() { root.applyCompositorConfig(); }
    }

    property Connections compositorConnections: Connections {
        target: Config.compositor
        function onBorderSizeChanged() { root.applyCompositorConfig(); }
        function onRoundingChanged() { root.applyCompositorConfig(); }
        function onGapsInChanged() { root.applyCompositorConfig(); }
        function onGapsOutChanged() { root.applyCompositorConfig(); }
        function onActiveBorderColorChanged() { root.applyCompositorConfig(); }
        function onInactiveBorderColorChanged() { root.applyCompositorConfig(); }
        function onBorderAngleChanged() { root.applyCompositorConfig(); }
        function onInactiveBorderAngleChanged() { root.applyCompositorConfig(); }
        function onSyncRoundnessChanged() { root.applyCompositorConfig(); }
        function onSyncBorderWidthChanged() { root.applyCompositorConfig(); }
        function onSyncBorderColorChanged() { root.applyCompositorConfig(); }
        function onSyncShadowOpacityChanged() { root.applyCompositorConfig(); }
        function onSyncShadowColorChanged() { root.applyCompositorConfig(); }
        function onShadowEnabledChanged() { root.applyCompositorConfig(); }
        function onShadowRangeChanged() { root.applyCompositorConfig(); }
        function onShadowColorChanged() { root.applyCompositorConfig(); }
        function onShadowOpacityChanged() { root.applyCompositorConfig(); }
        function onBlurEnabledChanged() { root.applyCompositorConfig(); }
        function onBlurSizeChanged() { root.applyCompositorConfig(); }
        function onBlurPassesChanged() { root.applyCompositorConfig(); }
        function onBlurExplicitIgnoreAlphaChanged() { root.applyCompositorConfig(); }
        function onBlurIgnoreAlphaValueChanged() { root.applyCompositorConfig(); }
    }

    property Connections colorsConnections: Connections {
        target: Colors
        function onFileChanged() { root.applyCompositorConfig(); }
        function onLoaded() { root.applyCompositorConfig(); }
    }

    property Connections themeConnections: Connections {
        target: Config.theme
        function onSrBarBgChanged() { root.applyCompositorConfig(); }
        function onSrBgChanged() { root.applyCompositorConfig(); }
        function onShadowColorChanged() { root.applyCompositorConfig(); }
        function onShadowOpacityChanged() { root.applyCompositorConfig(); }
    }

    property Connections barConnections: Connections {
        target: Config
        function onNotchPositionChanged() { root.applyCompositorConfig(); }
    }

    property Connections globalStatesConnections: Connections {
        target: GlobalStates
        function onCompositorLayoutChanged() { root.applyCompositorConfig(); }
        function onCompositorLayoutReadyChanged() { root.applyCompositorConfig(); }
    }

    Component.onCompleted: applyCompositorConfig()
}
