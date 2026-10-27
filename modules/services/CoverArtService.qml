pragma Singleton

import QtQuick
import Quickshell
import qs.config
import qs.modules.globals

Singleton {
    id: root

    readonly property var player: MprisController.activePlayer

    // "" when nothing resolved — callers fall back rather than show stale art.
    property string artPath: ""
    property string artOrigin: "none"  // artUrl | embedded | sidecar | online | none
    property bool resolving: false

    readonly property bool hasArt: root.artPath !== ""

    // Browsers publish no desktopEntry, so fall through to identity and the
    // dbus name before giving up.
    readonly property string playerIcon: {
        const p = root.player;
        if (!p)
            return "";
        const name = (p.desktopEntry ?? "") !== "" ? p.desktopEntry : ((p.identity ?? "") !== "" ? p.identity : (p.dbusName ?? "").replace("org.mpris.MediaPlayer2.", "").split(".")[0]);
        if (name === "")
            return "";
        return Quickshell.iconPath(AppSearch.getCachedIcon(name), true);
    }

    readonly property string fallbackMode: Config.media?.coverArtFallback ?? "appIcon"

    // "" means draw a placeholder, not an image.
    readonly property string source: {
        if (root.hasArt)
            return "file://" + root.artPath;
        if (root.fallbackMode === "appIcon" && root.playerIcon !== "")
            return root.playerIcon;
        if (root.fallbackMode === "wallpaper")
            return root.wallpaperPath;
        return "";
    }

    readonly property bool usingFallback: !root.hasArt && root.source !== ""
    readonly property bool showPlaceholder: root.source === ""

    readonly property string wallpaperPath: {
        if (!GlobalStates.wallpaperManager)
            return "";
        const frame = GlobalStates.wallpaperManager.getLockscreenFramePath(GlobalStates.wallpaperManager.currentWallpaper);
        return frame ? "file://" + frame : "";
    }

    // Anything that can change the artwork; nothing that changes per second.
    readonly property string trackKey: {
        const p = root.player;
        if (!p)
            return "";
        return [p.dbusName ?? "", p.trackArtUrl ?? "", root.trackUrl, p.trackTitle ?? "", p.trackArtist ?? "", p.trackAlbum ?? ""].join("\u001f");
    }

    readonly property string trackUrl: {
        const meta = root.player?.metadata ?? null;
        return meta ? (meta["xesam:url"] ?? "") : "";
    }

    onTrackKeyChanged: {
        root.artPath = "";
        root.artOrigin = "none";
        if (root.trackKey === "")
            return;
        debounce.restart();
    }

    // MPRIS delivers one track change as several property updates.
    Timer {
        id: debounce
        interval: 200
        repeat: false
        onTriggered: root.resolve()
    }

    function resolve() {
        const p = root.player;
        if (!p)
            return;

        root.resolving = true;
        flokd.send("resolve", {
            key: root.trackKey,
            art_url: p.trackArtUrl ?? "",
            track_url: root.trackUrl,
            title: p.trackTitle ?? "",
            artist: p.trackArtist ?? "",
            album: p.trackAlbum ?? "",
            embedded: Config.media?.coverArtEmbedded ?? true,
            online: Config.media?.coverArtOnline ?? false
        });
    }

    FlokdSocket {
        id: flokd
        service: "coverart"
        onMessage: d => {
            if (d.key !== root.trackKey)
                return;
            root.resolving = false;
            root.artPath = d.ok ? d.path : "";
            root.artOrigin = d.source;
        }
    }
}
