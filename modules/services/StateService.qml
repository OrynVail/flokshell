pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Io

// Small persistent state shared by services, one JSON file.
Singleton {
    id: root

    property string stateFile: Quickshell.statePath("states.json")
    property var state: ({})
    property bool initialized: false

    signal stateLoaded

    FileView {
        id: file
        path: root.stateFile
        onLoaded: {
            try {
                const content = text().trim();
                root.state = content ? JSON.parse(content) : {};
            } catch (e) {
                console.warn("StateService: Failed to parse state file:", e);
                root.state = {};
            }
            root.ready();
        }
        onLoadFailed: error => {
            root.state = {};
            root.ready();
        }
    }

    function ready() {
        if (root.initialized)
            return;
        root.initialized = true;
        root.stateLoaded();
    }

    function get(key, defaultValue) {
        return root.state[key] !== undefined ? root.state[key] : defaultValue;
    }

    function set(key, value) {
        if (!root.initialized) {
            console.warn("StateService: Attempted to set state before initialization");
            return;
        }
        root.state[key] = value;
        save();
    }

    function save() {
        if (root.initialized)
            file.setText(JSON.stringify(root.state));
    }

    function load() {
        file.reload();
    }

    function clear() {
        root.state = {};
        save();
    }
}
