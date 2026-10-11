//! Calls over the internet: tiles that call a web address (Home Assistant, OBS and the
//! like) and the weather in the top bar, from Open-Meteo (free, no key needed). Each call
//! blocks, so make them from a background thread.

use std::time::Duration;

use serde_json::Value;

use crate::commands::first_line;
use crate::config::{WeatherConfig, WebRequest};

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into()
}

/// Makes a tile's request. Returns the first line of the answer, or the status when the
/// answer is empty.
pub fn send(request: &WebRequest) -> Result<String, String> {
    let agent = agent();
    let url = request.url.as_str();
    let response = match request.method.as_str() {
        "GET" | "DELETE" | "HEAD" => {
            let mut builder = match request.method.as_str() {
                "GET" => agent.get(url),
                "DELETE" => agent.delete(url),
                _ => agent.head(url),
            };
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            builder.call()
        }
        "POST" | "PUT" | "PATCH" => {
            let mut builder = match request.method.as_str() {
                "POST" => agent.post(url),
                "PUT" => agent.put(url),
                _ => agent.patch(url),
            };
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            // JSON is what smart-home and streaming APIs expect, unless told otherwise.
            if !request.headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                && request.body.as_deref().is_some_and(looks_like_json)
            {
                builder = builder.header("Content-Type", "application/json");
            }
            builder.send(request.body.clone().unwrap_or_default())
        }
        other => return Err(format!("unknown method \"{other}\" (use GET, POST, PUT, PATCH or DELETE)")),
    };
    let mut response = response.map_err(describe)?;
    let status = response.status();
    let text = response.body_mut().read_to_string().unwrap_or_default();
    Ok(first_line(&text).unwrap_or_else(|| format!("Done ({status})")))
}

/// Downloads a text file, such as a calendar. Large calendars hold years of events, so
/// this allows up to 50 MB.
pub fn fetch_text(url: &str) -> Result<String, String> {
    let mut response = agent().get(url).call().map_err(describe)?;
    response
        .body_mut()
        .with_config()
        .limit(50 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())
}

fn looks_like_json(body: &str) -> bool {
    matches!(body.trim_start().chars().next(), Some('{' | '['))
}

fn describe(err: ureq::Error) -> String {
    match err {
        ureq::Error::StatusCode(code) => format!("the server answered {code}"),
        ureq::Error::Timeout(_) => "no answer within 10 seconds".into(),
        other => other.to_string(),
    }
}

/// The weather now and for the next days, in °C and km/h, and the place found for a place
/// name ("Amsterdam, Netherlands"; empty for coordinates).
#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    pub celsius: f32,
    pub summary: &'static str,
    pub place: String,
    pub feels_like: f32,
    /// Percent
    pub humidity: u8,
    pub wind_kmh: f32,
    /// Local times, such as "07:58".
    pub sunrise: String,
    pub sunset: String,
    /// Today first.
    pub days: Vec<DayForecast>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DayForecast {
    /// "2026-10-11"
    pub date: String,
    pub summary: &'static str,
    pub high: f32,
    pub low: f32,
    /// The highest chance of rain in the day, in percent.
    pub rain_chance: u8,
}

/// Looks up the weather for the configured place.
pub fn weather(config: &WeatherConfig) -> Result<Weather, String> {
    let agent = agent();
    let (latitude, longitude, found) = match (config.latitude, config.longitude) {
        (Some(latitude), Some(longitude)) => (latitude, longitude, String::new()),
        _ => {
            let place = config.place.as_deref().unwrap_or_default().trim();
            let found: Value = agent
                .get("https://geocoding-api.open-meteo.com/v1/search")
                .query("name", place)
                .query("count", "1")
                .call()
                .map_err(describe)?
                .body_mut()
                .read_json()
                .map_err(|e| e.to_string())?;
            let first = &found["results"][0];
            let name = [first["name"].as_str(), first["country"].as_str()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ");
            match (first["latitude"].as_f64(), first["longitude"].as_f64()) {
                (Some(latitude), Some(longitude)) => (latitude, longitude, name),
                _ => return Err(format!("couldn't find a place called \"{place}\"")),
            }
        }
    };
    let forecast: Value = agent
        .get("https://api.open-meteo.com/v1/forecast")
        .query("latitude", latitude.to_string())
        .query("longitude", longitude.to_string())
        .query("current", "temperature_2m,apparent_temperature,relative_humidity_2m,wind_speed_10m,weather_code")
        .query("daily", "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max,sunrise,sunset")
        // Days and sunrise in the place's own time zone.
        .query("timezone", "auto")
        .query("forecast_days", "6")
        .call()
        .map_err(describe)?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    read_forecast(&forecast, found)
}

/// Reads Open-Meteo's answer.
fn read_forecast(forecast: &Value, place: String) -> Result<Weather, String> {
    let current = &forecast["current"];
    let number = |value: &Value| value.as_f64().map(|v| v as f32);
    let celsius = number(&current["temperature_2m"]).ok_or("the forecast had no temperature")?;
    let daily = &forecast["daily"];
    let list = |key: &str| daily[key].as_array().cloned().unwrap_or_default();
    let (dates, codes, highs, lows, rain) = (
        list("time"),
        list("weather_code"),
        list("temperature_2m_max"),
        list("temperature_2m_min"),
        list("precipitation_probability_max"),
    );
    let days = dates
        .iter()
        .enumerate()
        .filter_map(|(i, date)| {
            Some(DayForecast {
                date: date.as_str()?.to_string(),
                summary: summary(codes.get(i).and_then(Value::as_u64).unwrap_or(0)),
                high: number(highs.get(i)?)?,
                low: number(lows.get(i)?)?,
                rain_chance: rain.get(i).and_then(Value::as_u64).unwrap_or(0).min(100) as u8,
            })
        })
        .collect();
    // "2026-10-11T07:58" -> "07:58"
    let time_of = |key: &str| {
        list(key)
            .first()
            .and_then(Value::as_str)
            .and_then(|t| t.split_once('T'))
            .map(|(_, time)| time.to_string())
            .unwrap_or_default()
    };
    Ok(Weather {
        celsius,
        summary: summary(current["weather_code"].as_u64().unwrap_or(0)),
        place,
        feels_like: number(&current["apparent_temperature"]).unwrap_or(celsius),
        humidity: current["relative_humidity_2m"].as_u64().unwrap_or(0).min(100) as u8,
        wind_kmh: number(&current["wind_speed_10m"]).unwrap_or(0.0),
        sunrise: time_of("sunrise"),
        sunset: time_of("sunset"),
        days,
    })
}

/// Where releases of the hub are published.
pub const RELEASES_API: &str = "https://api.github.com/repos/sudpmishra/PC-Hub/releases/latest";

/// The newest published release: its version ("0.2.0") and the page to download it from.
/// None when nothing has been published yet.
pub fn latest_release() -> Result<Option<(String, String)>, String> {
    let response = agent()
        .get(RELEASES_API)
        // GitHub turns away requests without a user agent.
        .header("User-Agent", concat!("pc-hub/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call();
    let mut response = match response {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(err) => return Err(describe(err)),
    };
    let release: Value = response.body_mut().read_json().map_err(|e| e.to_string())?;
    let tag = release["tag_name"].as_str().ok_or("the release had no version")?;
    let page = release["html_url"].as_str().unwrap_or("https://github.com/sudpmishra/PC-Hub/releases");
    Ok(Some((tag.trim_start_matches(['v', 'V']).to_string(), page.to_string())))
}

/// Whether version `a` ("0.10.1") is newer than `b` ("0.9.4"), comparing each number.
pub fn is_newer(a: &str, b: &str) -> bool {
    // Trailing zeros don't count: 0.1.0 is the same as 0.1.
    let numbers = |v: &str| -> Vec<u64> {
        let mut numbers: Vec<u64> = v.split(['.', '-', '+']).map_while(|part| part.parse().ok()).collect();
        while numbers.last() == Some(&0) {
            numbers.pop();
        }
        numbers
    };
    numbers(a) > numbers(b)
}

/// A word or two for a WMO weather code, as Open-Meteo reports them.
pub fn summary(code: u64) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Cloudy",
        45 | 48 => "Fog",
        51..=57 => "Drizzle",
        61..=67 => "Rain",
        71..=77 => "Snow",
        80..=82 => "Showers",
        85 | 86 => "Snow showers",
        95..=99 => "Thunderstorm",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_words() {
        assert_eq!(summary(0), "Clear");
        assert_eq!(summary(63), "Rain");
        assert_eq!(summary(96), "Thunderstorm");
        assert_eq!(summary(42), "");
    }

    #[test]
    fn reads_a_forecast() {
        let answer: Value = serde_json::from_str(
            r#"{
                "current": { "temperature_2m": 14.2, "apparent_temperature": 12.1,
                             "relative_humidity_2m": 81, "wind_speed_10m": 18.5, "weather_code": 3 },
                "daily": { "time": ["2026-10-11", "2026-10-12"], "weather_code": [61, 0],
                           "temperature_2m_max": [16.0, 18.4], "temperature_2m_min": [9.5, 8.1],
                           "precipitation_probability_max": [80, 5],
                           "sunrise": ["2026-10-11T07:58", "2026-10-12T08:00"],
                           "sunset": ["2026-10-11T18:51", "2026-10-12T18:49"] }
            }"#,
        )
        .unwrap();
        let weather = read_forecast(&answer, "Amsterdam, Netherlands".into()).unwrap();
        assert_eq!((weather.celsius, weather.summary, weather.humidity), (14.2, "Cloudy", 81));
        assert_eq!((weather.sunrise.as_str(), weather.sunset.as_str()), ("07:58", "18:51"));
        assert_eq!(weather.days.len(), 2);
        assert_eq!(weather.days[0].summary, "Rain");
        assert_eq!((weather.days[1].high, weather.days[1].rain_chance), (18.4, 5));
        assert!(read_forecast(&serde_json::json!({}), String::new()).is_err());
    }

    #[test]
    fn versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.99.1"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.1"));
    }

    #[test]
    #[ignore]
    fn live_release_check() {
        // Fine either way: no releases yet (None), or one with a version.
        if let Some((version, page)) = latest_release().unwrap() {
            assert!(!version.is_empty());
            assert!(page.starts_with("https://"));
        }
    }

    #[test]
    fn json_bodies() {
        assert!(looks_like_json(" {\"a\": 1}"));
        assert!(looks_like_json("[1]"));
        assert!(!looks_like_json("on"));
    }

    // These reach the internet, so they only run when asked: cargo test -- --ignored
    #[test]
    #[ignore]
    fn live_weather() {
        let place = WeatherConfig {
            place: Some("Amsterdam".into()),
            ..Default::default()
        };
        let weather = weather(&place).unwrap();
        assert!((-60.0..60.0).contains(&weather.celsius));
        assert!(weather.place.contains("Amsterdam"), "{}", weather.place);
        assert!(weather.days.len() >= 5);
        assert!(!weather.sunrise.is_empty());
    }

    #[test]
    #[ignore]
    fn live_request() {
        let request = WebRequest {
            method: "GET".into(),
            url: "https://example.com".into(),
            headers: Vec::new(),
            body: None,
        };
        assert!(send(&request).unwrap().contains("doctype"));
        let missing = WebRequest {
            url: "https://example.com/no-such-page-here".into(),
            ..request
        };
        assert!(send(&missing).unwrap_err().contains("404"));
    }

    #[test]
    fn bad_methods_are_reported() {
        let request = WebRequest {
            method: "FETCH".into(),
            url: "http://localhost".into(),
            headers: Vec::new(),
            body: None,
        };
        assert!(send(&request).unwrap_err().contains("FETCH"));
    }
}
