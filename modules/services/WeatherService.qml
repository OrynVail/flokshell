pragma Singleton

import QtQuick
import Quickshell
import Quickshell.Io
import qs.config
import qs.modules.globals

Singleton {
    id: root

    // Current weather data
    property string weatherSymbol: ""
    property real currentTemp: 0
    property real maxTemp: 0
    property real minTemp: 0
    property int weatherCode: 0
    property real windSpeed: 0
    property bool dataAvailable: false
    property bool isLoading: false
    property bool hasFailed: false
    property bool isStale: false

    // 7-day forecast data
    property var forecast: []

    // Sun position data
    property string sunrise: ""  // HH:MM
    property string sunset: ""   // HH:MM
    property real sunProgress: 0.0  // Arc position (0.0-1.0)
    property bool isDay: true
    property string timeOfDay: "Day"  // "Day", "Evening", "Night"
    property string weatherDescription: ""

    // Debug mode
    property bool debugMode: false
    property real debugHour: 12.0  // 0-24 hour (e.g. 14.5 = 2:30 PM)
    property int debugWeatherCode: 0

    // flokd owns fetching, geocoding, retry and the cache; it emits Celsius and
    // a full state replace per message, so handling one is a plain assignment.
    FlokdSocket {
        id: flokd
        service: "weather"
        onMessage: d => root.apply(d)
    }

    function apply(d) {
        root.dataAvailable = d.available;
        root.isLoading = d.loading;
        root.hasFailed = d.failed;
        root.isStale = d.stale;

        root.weatherCode = d.code;
        root.weatherDescription = d.description;
        root.weatherSymbol = getWeatherCodeEmoji(d.code);
        root.currentTemp = convertTemp(d.temp_c);
        root.maxTemp = convertTemp(d.max_c);
        root.minTemp = convertTemp(d.min_c);
        root.windSpeed = d.wind_kmh;
        root.sunrise = d.sunrise;
        root.sunset = d.sunset;

        root.forecast = (d.forecast || []).map((day, i) => {
            // Manual date parse to avoid UTC midnight issues ("YYYY-MM-DD")
            let p = day.date.split("-");
            let name = i === 0 ? "Today" : new Date(parseInt(p[0]), parseInt(p[1]) - 1, parseInt(p[2])).toLocaleDateString(Qt.locale(), "ddd");
            return {
                date: day.date,
                dayName: name.charAt(0).toUpperCase() + name.slice(1),
                weatherCode: day.code,
                emoji: getWeatherCodeEmoji(day.code),
                maxTemp: convertTemp(day.max_c),
                minTemp: convertTemp(day.min_c)
            };
        });

        root.calculateSunPosition();
    }

    function updateWeather() {
        flokd.send("refresh");
    }

    // Unit is a display preference; flokd stays in Celsius.
    function convertTemp(temp) {
        if (Config.weather.unit === "F") {
            return (temp * 9 / 5) + 32;
        }
        return temp;
    }

    // Convert "HH:MM" to decimal hours
    function parseTime(timeStr) {
        if (!timeStr)
            return 0;
        var parts = timeStr.split(":");
        return parseInt(parts[0]) + parseInt(parts[1]) / 60;
    }

    // Fixed sunrise/sunset for visual consistency (sun at zenith at 12:00)
    readonly property real visualSunriseHour: 6.0
    readonly property real visualSunsetHour: 18.0

    // Calculate sun/moon progress based on hour (0-24 format)
    function calculateSunProgress(hour, sunriseH, sunsetH) {
        if (hour >= sunriseH && hour <= sunsetH) {
            return (hour - sunriseH) / (sunsetH - sunriseH);
        } else {
            var nightDuration = 24 - (sunsetH - sunriseH);
            if (hour > sunsetH) {
                return (hour - sunsetH) / nightDuration;
            } else {
                return (hour + (24 - sunsetH)) / nightDuration;
            }
        }
    }

    // Calculate debug values based on debugHour
    readonly property real debugSunProgress: calculateSunProgress(debugHour, visualSunriseHour, visualSunsetHour)
    readonly property bool debugIsDay: debugHour >= visualSunriseHour && debugHour <= visualSunsetHour

    // Transition scheme for time blending
    function calculateTimeBlend(hour) {
        var day = 0, evening = 0, night = 0;

        if (hour >= 9 && hour <= 17) {
            day = 1.0;
        } else if (hour > 8 && hour < 9) {
            var t = hour - 8;
            evening = 1.0 - t;
            day = t;
        } else if (hour > 17 && hour < 18) {
            var t = hour - 17;
            day = 1.0 - t;
            evening = t;
        } else if (hour >= 6 && hour <= 8) {
            evening = 1.0;
        } else if (hour >= 18 && hour <= 20) {
            evening = 1.0;
        } else if (hour > 5 && hour < 6) {
            var t = hour - 5;
            night = 1.0 - t;
            evening = t;
        } else if (hour > 20 && hour < 21) {
            var t = hour - 20;
            evening = 1.0 - t;
            night = t;
        } else {
            night = 1.0;
        }

        return {
            day: day,
            evening: evening,
            night: night
        };
    }

    readonly property var debugTimeBlend: calculateTimeBlend(debugHour)
    property real currentHour: 12.0
    readonly property var realTimeBlend: calculateTimeBlend(currentHour)
    readonly property real realSunProgress: calculateSunProgress(currentHour, visualSunriseHour, visualSunsetHour)
    readonly property real realSunriseHour: sunrise.length > 0 ? parseTime(sunrise) : 6.0
    readonly property real realSunsetHour: sunset.length > 0 ? parseTime(sunset) : 18.0
    readonly property bool realIsDay: currentHour >= realSunriseHour && currentHour <= realSunsetHour

    readonly property var effectiveTimeBlend: debugMode ? debugTimeBlend : realTimeBlend

    readonly property string debugTimeOfDay: {
        var blend = debugTimeBlend;
        if (blend.day >= blend.evening && blend.day >= blend.night)
            return "Day";
        if (blend.evening >= blend.night)
            return "Evening";
        return "Night";
    }

    // Effective values (use debug values when debugMode is on)
    readonly property real effectiveSunProgress: debugMode ? debugSunProgress : realSunProgress
    readonly property string effectiveTimeOfDay: debugMode ? debugTimeOfDay : timeOfDay
    readonly property bool effectiveIsDay: debugMode ? debugIsDay : realIsDay
    readonly property int effectiveWeatherCode: debugMode ? debugWeatherCode : weatherCode
    readonly property string effectiveWeatherSymbol: debugMode ? getWeatherCodeEmoji(debugWeatherCode) : weatherSymbol
    readonly property string effectiveWeatherDescription: debugMode ? getWeatherDescription(debugWeatherCode) : weatherDescription

    // Weather effect types based on code
    readonly property string effectiveWeatherEffect: getWeatherEffect(effectiveWeatherCode)
    readonly property real effectiveWeatherIntensity: getWeatherIntensity(effectiveWeatherCode)

    function getWeatherEffect(code) {
        if (code === 0 || code === 1)
            return "clear";
        if (code === 2 || code === 3)
            return "clouds";
        if (code === 45 || code === 48)
            return "fog";
        if (code >= 51 && code <= 57)
            return "drizzle";
        if (code >= 61 && code <= 67)
            return "rain";
        if (code >= 71 && code <= 77)
            return "snow";
        if (code >= 80 && code <= 82)
            return "rain";
        if (code >= 85 && code <= 86)
            return "snow";
        if (code === 95)
            return "thunderstorm";
        if (code >= 96 && code <= 99)
            return "thunderstorm";
        return "clear";
    }

    function getWeatherIntensity(code) {
        if (code === 0 || code === 1)
            return 0.0;
        if (code === 2)
            return 0.5;
        if (code === 3)
            return 1.0;
        if (code === 45)
            return 0.5;
        if (code === 48)
            return 0.7;
        if (code === 51 || code === 56)
            return 0.3;
        if (code === 53)
            return 0.5;
        if (code === 55 || code === 57)
            return 0.7;
        if (code === 61)
            return 0.4;
        if (code === 63 || code === 66)
            return 0.6;
        if (code === 65 || code === 67)
            return 0.9;
        if (code === 71)
            return 0.3;
        if (code === 73)
            return 0.5;
        if (code === 75 || code === 77)
            return 0.8;
        if (code === 80)
            return 0.5;
        if (code === 81)
            return 0.7;
        if (code === 82)
            return 1.0;
        if (code === 85)
            return 0.6;
        if (code === 86)
            return 0.9;
        if (code === 95)
            return 0.8;
        if (code >= 96)
            return 1.0;
        return 0.0;
    }

    // Kept in QML, unlike the description: a glyph is a design choice, and §3 of
    // the shell's notes has the language mid-replacement.
    function getWeatherCodeEmoji(code) {
        if (code === 0)
            return "☀️";
        if (code === 1)
            return "🌤️";
        if (code === 2)
            return "⛅";
        if (code === 3)
            return "☁️";
        if (code === 45)
            return "🌫️";
        if (code === 48)
            return "🌨️";
        if (code >= 51 && code <= 53)
            return "🌦️";
        if (code === 55)
            return "🌧️";
        if (code >= 56 && code <= 57)
            return "🧊";
        if (code >= 61 && code <= 65)
            return "🌧️";
        if (code >= 66 && code <= 67)
            return "🧊";
        if (code >= 71 && code <= 77)
            return "❄️";
        if (code >= 80 && code <= 81)
            return "🌦️";
        if (code === 82)
            return "🌧️";
        if (code >= 85 && code <= 86)
            return "🌨️";
        if (code === 95)
            return "⛈️";
        if (code >= 96 && code <= 99)
            return "🌩️";
        return "❓";
    }

    function getWeatherDescription(code) {
        if (code === 0)
            return "Clear sky";
        if (code === 1)
            return "Mainly clear";
        if (code === 2)
            return "Partly cloudy";
        if (code === 3)
            return "Overcast";
        if (code === 45)
            return "Foggy";
        if (code === 48)
            return "Rime fog";
        if (code >= 51 && code <= 53)
            return "Light drizzle";
        if (code === 55)
            return "Dense drizzle";
        if (code >= 56 && code <= 57)
            return "Freezing drizzle";
        if (code === 61)
            return "Light rain";
        if (code === 63)
            return "Moderate rain";
        if (code === 65)
            return "Heavy rain";
        if (code >= 66 && code <= 67)
            return "Freezing rain";
        if (code === 71)
            return "Light snow";
        if (code === 73)
            return "Moderate snow";
        if (code === 75)
            return "Heavy snow";
        if (code === 77)
            return "Snow grains";
        if (code >= 80 && code <= 81)
            return "Rain showers";
        if (code === 82)
            return "Heavy showers";
        if (code >= 85 && code <= 86)
            return "Snow showers";
        if (code === 95)
            return "Thunderstorm";
        if (code >= 96 && code <= 99)
            return "Thunderstorm with hail";
        return "Unknown";
    }

    function calculateSunPosition() {
        var now = new Date();
        var hour = now.getHours() + now.getMinutes() / 60;

        // Update currentHour so readonly properties can react to time changes
        root.currentHour = hour;

        var sunriseH = sunrise.length > 0 ? parseTime(sunrise) : 6.0;
        var sunsetH = sunset.length > 0 ? parseTime(sunset) : 18.0;

        root.isDay = (hour >= sunriseH && hour <= sunsetH);
        root.sunProgress = calculateSunProgress(hour, sunriseH, sunsetH);

        var blend = calculateTimeBlend(hour);
        if (blend.day >= blend.evening && blend.day >= blend.night) {
            root.timeOfDay = "Day";
        } else if (blend.evening >= blend.night) {
            root.timeOfDay = "Evening";
        } else {
            root.timeOfDay = "Night";
        }
    }

    // Interpolated locally from the sunrise/sunset flokd already sent — a tick
    // per minute over the socket would be a round trip for arithmetic.
    property Timer sunPositionTimer: Timer {
        interval: 60000
        running: GlobalStates.dashboardOpen || GlobalStates.launcherOpen || GlobalStates.overviewOpen
        repeat: true
        onTriggered: root.calculateSunPosition()
    }

    Component.onCompleted: {
        var now = new Date();
        currentHour = now.getHours() + now.getMinutes() / 60;
    }
}
