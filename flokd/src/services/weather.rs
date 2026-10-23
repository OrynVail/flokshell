use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::bus::Bus;
use crate::config;

const SERVICE: &str = "weather";
const REFRESH: Duration = Duration::from_secs(600);
const ATTEMPTS: u32 = 3;
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)] // an older cache missing a field reads as that field's default
pub struct State {
    available: bool,
    loading: bool,
    failed: bool,
    stale: bool,
    code: i64,
    description: String,
    // Always Celsius and km/h. Unit is a display preference and converting here
    // would mean reading config the shell already has.
    temp_c: f64,
    max_c: f64,
    min_c: f64,
    wind_kmh: f64,
    sunrise: String,
    sunset: String,
    forecast: Vec<Day>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Day {
    date: String,
    code: i64,
    max_c: f64,
    min_c: f64,
}

#[derive(Deserialize)]
struct Forecast {
    current_weather: Current,
    daily: Daily,
}

#[derive(Deserialize)]
struct Current {
    temperature: f64,
    windspeed: f64,
    weathercode: i64,
}

#[derive(Deserialize)]
struct Daily {
    time: Vec<String>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
    sunrise: Vec<String>,
    sunset: Vec<String>,
    weathercode: Vec<i64>,
}

#[derive(Deserialize)]
struct Geocode {
    results: Option<Vec<Coords>>,
}

#[derive(Deserialize, Clone, Copy)]
struct Coords {
    latitude: f64,
    longitude: f64,
}

// All keyless, all rate-limited on their own schedule, all spelling the fields
// the same way — geojs quotes the numbers, hence the coercion in `as_f64`.
// ipapi.co is last because it is the one already answering 429 here.
const GEOIP: [&str; 3] = [
    "https://ipwho.is/",
    "https://get.geojs.io/v1/ip/geo.json",
    "https://ipapi.co/json/",
];

#[derive(Deserialize)]
struct GeoIp {
    latitude: Option<serde_json::Value>,
    longitude: Option<serde_json::Value>,
}

fn as_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.parse().ok())
}

impl GeoIp {
    fn coords(&self) -> Option<Coords> {
        Some(Coords {
            latitude: as_f64(self.latitude.as_ref()?)?,
            longitude: as_f64(self.longitude.as_ref()?)?,
        })
    }
}

pub fn description(code: i64) -> &'static str {
    match code {
        0 => "Clear sky",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 => "Foggy",
        48 => "Rime fog",
        51..=53 => "Light drizzle",
        55 => "Dense drizzle",
        56..=57 => "Freezing drizzle",
        61 => "Light rain",
        63 => "Moderate rain",
        65 => "Heavy rain",
        66..=67 => "Freezing rain",
        71 => "Light snow",
        73 => "Moderate snow",
        75 => "Heavy snow",
        77 => "Snow grains",
        80..=81 => "Rain showers",
        82 => "Heavy showers",
        85..=86 => "Snow showers",
        95 => "Thunderstorm",
        96..=99 => "Thunderstorm with hail",
        _ => "Unknown",
    }
}

// open-meteo gives "YYYY-MM-DDTHH:MM"; the shell wants wall-clock only.
fn clock(stamp: &str) -> String {
    stamp.split('T').nth(1).unwrap_or_default().to_string()
}

pub struct Service {
    http: reqwest::Client,
    bus: Bus,
    state: State,
}

impl Service {
    pub fn new(bus: Bus) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(TIMEOUT).build()?,
            bus,
            state: State::default(),
        })
    }

    pub async fn run(mut self, mut refresh: mpsc::Receiver<()>) {
        let mut config_changed = match config::watch() {
            Ok(rx) => rx,
            Err(e) => {
                tracing::warn!("config watch unavailable, location changes need a restart: {e}");
                mpsc::channel(1).1
            }
        };

        // Serve the cache before the network is touched, so a cold start shows
        // last night's weather instead of an empty widget for two seconds.
        if let Some(cached) = self.load_cache().await {
            self.state = State {
                stale: true,
                ..cached
            };
            self.publish();
        }

        let mut location = self.location().await;
        self.refresh(&location).await;

        let mut tick = tokio::time::interval(REFRESH);
        tick.reset();

        loop {
            tokio::select! {
                _ = tick.tick() => self.refresh(&location).await,
                Some(()) = refresh.recv() => self.refresh(&location).await,
                Some(()) = config_changed.recv() => {
                    let next = self.location().await;
                    if next != location {
                        tracing::info!(location = %next, "location changed");
                        location = next;
                        self.refresh(&location).await;
                    }
                }
            }
        }
    }

    async fn location(&self) -> String {
        config::read("weather.json")
            .await
            .get("location")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    async fn refresh(&mut self, location: &str) {
        self.state.loading = true;
        self.publish();

        let mut last = None;
        for attempt in 1..=ATTEMPTS {
            match self.fetch(location).await {
                Ok(state) => {
                    self.state = state;
                    self.publish();
                    self.save_cache().await;
                    return;
                }
                Err(e) => {
                    tracing::warn!(attempt, "fetch failed: {e:#}");
                    last = Some(e);
                    if attempt < ATTEMPTS {
                        tokio::time::sleep(Duration::from_secs(3 * attempt as u64)).await;
                    }
                }
            }
        }

        tracing::error!("giving up until next refresh: {:#}", last.unwrap());
        self.state.loading = false;
        self.state.failed = true;
        // Keep whatever was already shown. A twenty-minute-old temperature is
        // worth more than a blank widget; `stale` is how the shell can tell.
        self.state.stale = self.state.available;
        self.publish();
    }

    async fn fetch(&self, location: &str) -> Result<State> {
        let at = self.resolve(location).await?;
        let url = format!(
            "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}\
             &current_weather=true&daily=temperature_2m_max,temperature_2m_min,sunrise,sunset,weathercode\
             &timezone=auto&forecast_days=7",
            at.latitude, at.longitude
        );

        let f: Forecast = self
            .http
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("forecast response did not match the expected shape")?;

        let days = f.daily.time.len();
        if days == 0 {
            return Err(anyhow!("forecast carried no days"));
        }

        let forecast = (0..days)
            .map(|i| Day {
                date: f.daily.time[i].clone(),
                code: f.daily.weathercode.get(i).copied().unwrap_or(0),
                max_c: f.daily.temperature_2m_max.get(i).copied().unwrap_or(0.0),
                min_c: f.daily.temperature_2m_min.get(i).copied().unwrap_or(0.0),
            })
            .collect();

        Ok(State {
            available: true,
            loading: false,
            failed: false,
            stale: false,
            code: f.current_weather.weathercode,
            description: description(f.current_weather.weathercode).to_string(),
            temp_c: f.current_weather.temperature,
            max_c: f.daily.temperature_2m_max.first().copied().unwrap_or(0.0),
            min_c: f.daily.temperature_2m_min.first().copied().unwrap_or(0.0),
            wind_kmh: f.current_weather.windspeed,
            sunrise: f
                .daily
                .sunrise
                .first()
                .map(|s| clock(s))
                .unwrap_or_default(),
            sunset: f.daily.sunset.first().map(|s| clock(s)).unwrap_or_default(),
            forecast,
        })
    }

    async fn resolve(&self, location: &str) -> Result<Coords> {
        if location.is_empty() {
            return self.geoip().await;
        }
        if let Some(at) = parse_coords(location) {
            return Ok(at);
        }
        self.geocode(location).await
    }

    async fn geocode(&self, city: &str) -> Result<Coords> {
        let g: Geocode = self
            .http
            .get("https://geocoding-api.open-meteo.com/v1/search")
            .query(&[("name", city), ("count", "1")])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        g.results
            .unwrap_or_default()
            .first()
            .copied()
            .ok_or_else(|| anyhow!("no such place: {city}"))
    }

    // One provider rate-limiting is routine, not a reason to have no location.
    // An explicit `weather.location` skips all of this and is the better setup.
    async fn geoip(&self) -> Result<Coords> {
        for url in GEOIP {
            match self.geoip_from(url).await {
                Ok(at) => return Ok(at),
                Err(e) => tracing::debug!(url, "geoip provider unusable: {e:#}"),
            }
        }
        Err(anyhow!("no geoip provider answered with coordinates"))
    }

    async fn geoip_from(&self, url: &str) -> Result<Coords> {
        self.http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<GeoIp>()
            .await?
            .coords()
            .ok_or_else(|| anyhow!("no coordinates in response"))
    }

    fn publish(&self) {
        match serde_json::to_value(&self.state) {
            Ok(v) => self.bus.publish(SERVICE, &v),
            Err(e) => tracing::error!("serialise failed: {e}"),
        }
    }

    fn cache_path(&self) -> std::path::PathBuf {
        config::cache_dir().join("weather.json")
    }

    // The whole state round-trips. Rebuilding "current" from day 0 of the
    // forecast looks right and is not: the daily code is the day's summary,
    // not what it is doing outside now.
    async fn load_cache(&self) -> Option<State> {
        let bytes = tokio::fs::read(self.cache_path()).await.ok()?;
        let state: State = serde_json::from_slice(&bytes).ok()?;
        state.available.then_some(state)
    }

    async fn save_cache(&self) {
        let path = self.cache_path();
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Ok(bytes) = serde_json::to_vec(&self.state) {
            let _ = tokio::fs::write(path, bytes).await;
        }
    }
}

fn parse_coords(s: &str) -> Option<Coords> {
    let (lat, lon) = s.split_once(',')?;
    Some(Coords {
        latitude: lat.trim().parse().ok()?,
        longitude: lon.trim().parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_cover_the_wmo_ranges() {
        assert_eq!(description(0), "Clear sky");
        assert_eq!(description(52), "Light drizzle");
        assert_eq!(description(67), "Freezing rain");
        assert_eq!(description(99), "Thunderstorm with hail");
        assert_eq!(description(4), "Unknown");
        assert_eq!(description(-1), "Unknown");
    }

    #[test]
    fn coords_parse_only_when_both_halves_are_numbers() {
        assert!(parse_coords("51.5,-0.12").is_some());
        assert!(parse_coords(" 51.5 , -0.12 ").is_some());
        assert!(parse_coords("London").is_none());
        assert!(parse_coords("51.5,").is_none());
        assert!(parse_coords("Pune,India").is_none());
    }

    #[test]
    fn clock_keeps_wall_time_and_tolerates_junk() {
        assert_eq!(clock("2026-10-23T07:42"), "07:42");
        assert_eq!(clock("2026-10-23"), "");
    }

    #[test]
    fn cache_round_trips_current_conditions() {
        let state = State {
            available: true,
            code: 3,
            temp_c: 31.8,
            sunrise: "06:30".into(),
            forecast: vec![Day {
                date: "2026-10-23".into(),
                code: 51,
                max_c: 33.3,
                min_c: 23.2,
            }],
            ..Default::default()
        };
        let back: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        // 3, not the forecast day's 51.
        assert_eq!(back.code, 3);
        assert_eq!(back.temp_c, 31.8);
        assert_eq!(back.sunrise, "06:30");
        assert_eq!(back.forecast.len(), 1);
    }

    #[test]
    fn geoip_accepts_numbers_and_quoted_numbers() {
        let numeric: GeoIp =
            serde_json::from_str(r#"{"latitude":31.71,"longitude":76.93}"#).unwrap();
        let quoted: GeoIp =
            serde_json::from_str(r#"{"latitude":"31.71","longitude":"76.93"}"#).unwrap();
        assert_eq!(numeric.coords().unwrap().latitude, 31.71);
        assert_eq!(quoted.coords().unwrap().latitude, 31.71);

        // A rate-limit body parses fine and must still count as a miss.
        let limited: GeoIp =
            serde_json::from_str(r#"{"error":true,"reason":"RateLimited"}"#).unwrap();
        assert!(limited.coords().is_none());
    }

    #[test]
    fn a_cache_without_data_is_ignored() {
        let empty = serde_json::to_vec(&State::default()).unwrap();
        let back: State = serde_json::from_slice(&empty).unwrap();
        assert!(!back.available);
    }
}
