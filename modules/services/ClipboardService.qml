pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io

// History, search and the system clipboard live in flokd; this is the panel's view of them.
QtObject {
    id: root

    property var items: []
    property var results: []
    property string query: ""
    property var imageDataById: ({})
    property var linkPreviewCache: ({})
    property int revision: 0

    signal listCompleted()
    signal resultsReady()
    signal fullContentRetrieved(string itemId, string content)
    signal linkPreviewFetched(string url, var metadata, string itemId)

    property FlokdSocket flokd: FlokdSocket {
        service: "clipboard"
        onFrame: msg => {
            if (msg.event === "state") {
                root.items = msg.data.items;
                root.listCompleted();
            } else if (msg.event === "content") {
                root.fullContentRetrieved(msg.data.id, msg.data.text);
            } else if (msg.event === "results" && msg.data.query === root.query) {
                root.results = msg.data.items;
                root.resultsReady();
            }
        }
    }

    property FlokdSocket linkPreviews: FlokdSocket {
        service: "linkpreview"
        onMessage: d => {
            if (!d.data.error)
                root.linkPreviewCache[d.url] = d.data;
            root.linkPreviewFetched(d.url, d.data, d.id);
        }
    }

    function copy(id) {
        flokd.send("copy", { id: id });
    }

    function copyFile(path, mime) {
        flokd.send("file", { path: path, mime: mime });
    }

    function search(text) {
        query = text;
        if (text.length > 0)
            flokd.send("search", { query: text });
    }

    function getFullContent(id) {
        flokd.send("content", { id: id });
    }

    function deleteItem(id) {
        flokd.send("delete", { id: id });
    }

    function clear() {
        flokd.send("clear");
    }

    function togglePin(id) {
        flokd.send("pin", { id: id });
    }

    function setAlias(id, alias) {
        flokd.send("alias", { id: id, alias: alias });
    }

    function find(id) {
        return items.find(i => i.id === id) || results.find(i => i.id === id);
    }

    function decodeToDataUrl(id, mime) {
        const item = find(id);
        if (!item || !item.binaryPath || imageDataById[id])
            return;
        imageDataById[id] = "file://" + item.binaryPath;
        revision++;
    }

    function getImageData(id) {
        return imageDataById[id] || "";
    }

    function fetchLinkPreview(url, itemId) {
        if (linkPreviewCache[url]) {
            Qt.callLater(() => root.linkPreviewFetched(url, linkPreviewCache[url], itemId));
            return;
        }
        linkPreviews.send("fetch", { url: url, id: itemId });
    }

    function move(itemId, step) {
        const index = items.findIndex(i => i.id === itemId);
        const other = index + step;
        if (index < 0 || other < 0 || other >= items.length || items[other].pinned !== items[index].pinned)
            return;

        // Swapped here too, so the row moves before flokd answers.
        const item = items[index];
        const neighbour = items[other];
        items[index] = neighbour;
        items[other] = item;
        listCompleted();
        flokd.send("swap", { a: item.id, b: neighbour.id });
    }

    function moveItemUp(itemId) {
        move(itemId, -1);
    }

    function moveItemDown(itemId) {
        move(itemId, 1);
    }

    property Process emojiTypeProcess: Process {
        running: false
        onExited: function(code) {
            if (code !== 0)
                console.warn("ClipboardService: emojiTypeProcess failed with code:", code);
        }
    }

    // Long enough for the panel to hand keyboard focus back.
    property Timer emojiTypeTimer: Timer {
        interval: 250
        repeat: false
        onTriggered: {
            emojiTypeProcess.command = ["wtype", "-M", "ctrl", "-P", "v", "-p", "v", "-m", "ctrl"];
            emojiTypeProcess.running = true;
        }
    }

    function copyAndTypeEmoji(emojiText) {
        flokd.send("text", { text: emojiText });
        emojiTypeTimer.start();
    }
}
