use crate::mcp::McpServer;
use crate::mcp_tools::Tool;
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub struct CurrencyRatesTool;

fn default_fiat() -> Vec<String> {
    vec!["USD".into(), "EUR".into(), "RUB".into()]
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CurrencyRatesParams {
    /// Fiat currency codes to quote against UZS (CBU.uz official rates). Default: USD, EUR, RUB.
    #[serde(default = "default_fiat")]
    pub fiat: Vec<String>,
    /// CoinGecko coin ids for crypto USD prices, e.g. ["bitcoin", "ethereum"]. Empty = skip crypto.
    #[serde(default)]
    pub crypto: Vec<String>,
}

impl Tool for CurrencyRatesTool {
    type Params = CurrencyRatesParams;

    fn name(&self) -> &'static str {
        "currency_rates"
    }

    fn description(&self) -> &'static str {
        "Fiat exchange rates vs UZS (official CBU.uz) and optional crypto USD prices (CoinGecko). No API key."
    }

    fn run<'a>(
        &'a self,
        p: CurrencyRatesParams,
        _server: &'a McpServer,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .user_agent("jarvis/1.0")
                .build()?;
            let mut sections = Vec::new();

            if !p.fiat.is_empty() {
                let rates: Value = client
                    .get("https://cbu.uz/uz/arkhiv-kursov-valyut/json/")
                    .send()
                    .await
                    .context("CBU.uz request failed")?
                    .json()
                    .await
                    .context("failed to parse CBU.uz response")?;
                let wanted: Vec<String> = p.fiat.iter().map(|c| c.to_uppercase()).collect();
                let mut lines = Vec::new();
                if let Some(items) = rates.as_array() {
                    for item in items {
                        let ccy = item["Ccy"].as_str().unwrap_or("");
                        if wanted.iter().any(|w| w == ccy) {
                            lines.push(format!(
                                "{} = {} UZS (per {} unit, {})",
                                ccy,
                                item["Rate"].as_str().unwrap_or("?"),
                                item["Nominal"].as_str().unwrap_or("1"),
                                item["Date"].as_str().unwrap_or("?"),
                            ));
                        }
                    }
                }
                if lines.is_empty() {
                    lines.push(format!("no CBU rates found for: {}", wanted.join(", ")));
                }
                sections.push(format!("CBU.uz official rates:\n{}", lines.join("\n")));
            }

            if !p.crypto.is_empty() {
                let ids = p.crypto.join(",").to_lowercase();
                let prices: Value = client
                    .get("https://api.coingecko.com/api/v3/simple/price")
                    .query(&[("ids", ids.as_str()), ("vs_currencies", "usd")])
                    .send()
                    .await
                    .context("CoinGecko request failed")?
                    .error_for_status()
                    .context("CoinGecko returned error status (rate limit?)")?
                    .json()
                    .await
                    .context("failed to parse CoinGecko response")?;
                let mut lines = Vec::new();
                if let Some(map) = prices.as_object() {
                    for (id, val) in map {
                        if let Some(usd) = val["usd"].as_f64() {
                            lines.push(format!("{id} = ${usd}"));
                        }
                    }
                }
                if lines.is_empty() {
                    lines.push(format!("no CoinGecko prices found for: {ids}"));
                }
                sections.push(format!("Crypto (USD):\n{}", lines.join("\n")));
            }

            if sections.is_empty() {
                return Ok("nothing requested — pass fiat codes and/or crypto ids".into());
            }
            Ok(sections.join("\n\n"))
        })
    }
}
