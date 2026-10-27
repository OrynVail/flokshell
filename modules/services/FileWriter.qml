pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Io

// Whole-file writes without a shell: no quoting to get wrong and no ARG_MAX,
// and FileView creates the directories on the way.
Singleton {
    id: root

    function write(path, text, done) {
        const f = Qt.createQmlObject('import Quickshell.Io; FileView { preload: false; printErrors: false }', root);
        f.path = path;
        f.saved.connect(() => {
            if (done)
                done(true);
            f.destroy();
        });
        f.saveFailed.connect(error => {
            console.warn("FileWriter: failed to write", path, error);
            if (done)
                done(false);
            f.destroy();
        });
        f.setText(text);
    }
}
