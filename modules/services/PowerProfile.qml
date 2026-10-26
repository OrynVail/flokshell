pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Services.UPower as UP
import qs.modules.theme

// Quickshell speaks power-profiles-daemon natively, so this stays out of flokd.
Singleton {
    id: root

    readonly property var profiles: ({
            "power-saver": UP.PowerProfile.PowerSaver,
            "balanced": UP.PowerProfile.Balanced,
            "performance": UP.PowerProfile.Performance
        })

    readonly property var availableProfiles: UP.PowerProfiles.hasPerformanceProfile ? ["power-saver", "balanced", "performance"] : ["power-saver", "balanced"]
    readonly property string currentProfile: Object.keys(profiles).find(k => profiles[k] === UP.PowerProfiles.profile) ?? ""

    function setProfile(profileName) {
        if (!availableProfiles.includes(profileName)) {
            console.warn("PowerProfile: Profile not available:", profileName);
            return;
        }
        UP.PowerProfiles.profile = profiles[profileName];
    }

    function getProfileIcon(profileName) {
        if (profileName === "power-saver")
            return Icons.powerSave;
        if (profileName === "performance")
            return Icons.performance;
        return Icons.balanced;
    }

    function getProfileDisplayName(profileName) {
        if (profileName === "power-saver")
            return "Power Save";
        if (profileName === "balanced")
            return "Balanced";
        if (profileName === "performance")
            return "Performance";
        return profileName;
    }
}
