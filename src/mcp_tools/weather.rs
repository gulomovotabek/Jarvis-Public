use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub struct WeatherTool;

fn default_days() -> u32 {
    3
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WeatherParams {
    /// City or place name, e.g. "Tashkent" or "Amsterdam".
    pub location: String,
    /// Forecast days to include (default 3, max 7).
    #[serde(default = "default_days")]
    pub days: u32,
}

fn weather_code_text(code: u64) -> &'static str {
    match code {
        0 => "clear sky",
        1..=3 => "partly cloudy",
        45 | 48 => "fog",
        51..=57 => "drizzle",
        61..=67 => "rain",
        71..=77 => "snow",
        80..=82 => "rain showers",
        85 | 86 => "snow showers",
        95..=99 => "thunderstorm",
        _ => "unknown",
    }
}

impl Tool for WeatherTool {
    type Params = WeatherParams;

    fn name(&self) -> &'static str {
        "weather"
    }

    fn description(&self) -> &'static str {
        "Current weather + daily forecast for a location via open-meteo (no API key)."
    }

    fn run<'a>(
        &'a self,
        p: WeatherParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let days = p.days.clamp(1, 7);
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()?;

            let geo: Value = client
                .get("https://geocoding-api.open-meteo.com/v1/search")
                .query(&[("name", p.location.as_str()), ("count", "1")])
                .send()
                .await
                .context("geocoding request failed")?
                .json()
                .await
                .context("failed to parse geocoding response")?;
            let place = geo["results"]
                .as_array()
                .and_then(|a| a.first())
                .cloned();
            let Some(place) = place else {
                bail!("location not found: {}", p.location);
            };
            let lat = place["latitude"].as_f64().context("no latitude")?;
            let lon = place["longitude"].as_f64().context("no longitude")?;
            let name = place["name"].as_str().unwrap_or(&p.location);
            let country = place["country"].as_str().unwrap_or("");

            let forecast: Value = client
                .get("https://api.open-meteo.com/v1/forecast")
                .query(&[
                    ("latitude", lat.to_string()),
                    ("longitude", lon.to_string()),
                    (
                        "current",
                        "temperature_2m,apparent_temperature,relative_humidity_2m,precipitation,weather_code,wind_speed_10m"
                            .to_string(),
                    ),
                    (
                        "daily",
                        "temperature_2m_max,temperature_2m_min,precipitation_probability_max,weather_code".to_string(),
                    ),
                    ("timezone", "auto".to_string()),
                    ("forecast_days", days.to_string()),
                ])
                .send()
                .await
                .context("forecast request failed")?
                .json()
                .await
                .context("failed to parse forecast response")?;

            let cur = &forecast["current"];
            let mut out = format!(
                "{name}{}{} — now: {}°C (feels {}°C), {}, humidity {}%, wind {} km/h",
                if country.is_empty() { "" } else { ", " },
                country,
                cur["temperature_2m"].as_f64().unwrap_or(0.0),
                cur["apparent_temperature"].as_f64().unwrap_or(0.0),
                weather_code_text(cur["weather_code"].as_u64().unwrap_or(u64::MAX)),
                cur["relative_humidity_2m"].as_u64().unwrap_or(0),
                cur["wind_speed_10m"].as_f64().unwrap_or(0.0),
            );

            let daily = &forecast["daily"];
            if let Some(dates) = daily["time"].as_array() {
                for (i, date) in dates.iter().enumerate() {
                    let max = daily["temperature_2m_max"][i].as_f64().unwrap_or(0.0);
                    let min = daily["temperature_2m_min"][i].as_f64().unwrap_or(0.0);
                    let precip = daily["precipitation_probability_max"][i].as_u64().unwrap_or(0);
                    let code = daily["weather_code"][i].as_u64().unwrap_or(u64::MAX);
                    out.push_str(&format!(
                        "\n{}: {min}..{max}°C, {}, precip {precip}%",
                        date.as_str().unwrap_or("?"),
                        weather_code_text(code),
                    ));
                }
            }
            Ok(out)
        })
    }
}
