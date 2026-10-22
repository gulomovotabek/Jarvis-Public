use anyhow::{Context, Result, bail};
use serde_json::Value;
use tracing::debug;

const BASE_URL: &str = "https://open.faceit.com/data/v4";

pub struct FaceitClient {
    client: reqwest::Client,
    api_key: String,
}

impl FaceitClient {
    pub fn new(api_key: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .context("failed to build FACEIT HTTP client")?;
        Ok(Self { client, api_key })
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let url = format!("{BASE_URL}{path}");
        debug!("FACEIT API GET {url}");
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .context("FACEIT API request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            if body.contains("was not found") || status.as_u16() == 404 {
                bail!("Player not found on FACEIT");
            }
            bail!("FACEIT API returned {status}: {body}");
        }
        resp.json::<Value>()
            .await
            .context("failed to parse FACEIT response")
    }

    async fn resolve_player_id(&self, nickname: &str) -> Result<(String, Value)> {
        let encoded = url::form_urlencoded::byte_serialize(nickname.as_bytes()).collect::<String>();
        let data = self.get(&format!("/players?nickname={encoded}")).await?;
        let player_id = data["player_id"]
            .as_str()
            .context("player_id not found — player may not exist")?
            .to_string();
        Ok((player_id, data))
    }

    pub async fn get_profile(&self, nickname: &str) -> Result<String> {
        let (player_id, profile) = self.resolve_player_id(nickname).await?;
        let stats = self
            .get(&format!("/players/{player_id}/stats/cs2"))
            .await?;
        let lifetime = &stats["lifetime"];

        let nick = profile["nickname"].as_str().unwrap_or(nickname);
        let mut out = format!("=== FACEIT Profile: {nick} ===\n");
        out.push_str(&format!("Player ID: {player_id}\n"));

        if let Some(country) = profile["country"].as_str() {
            out.push_str(&format!("Country: {}\n", country.to_uppercase()));
        }

        let cs2 = &profile["games"]["cs2"];
        if let Some(elo) = cs2["faceit_elo"].as_u64() {
            out.push_str(&format!("ELO: {elo}\n"));
        }
        if let Some(level) = cs2["skill_level"].as_u64() {
            out.push_str(&format!("Skill Level: {level}\n"));
        }

        out.push_str("\n--- Lifetime CS2 Stats ---\n");
        for (key, label) in [
            ("Matches", "Matches"),
            ("Wins", "Wins"),
            ("Win Rate %", "Win Rate"),
            ("Average K/D Ratio", "Avg K/D"),
            ("Average Headshots %", "Avg HS%"),
            ("ADR", "ADR"),
            ("Longest Win Streak", "Best Win Streak"),
            ("Current Win Streak", "Current Streak"),
        ] {
            if let Some(val) = lifetime[key].as_str() {
                let suffix = if key == "Win Rate %" || key == "Average Headshots %" {
                    "%"
                } else {
                    ""
                };
                out.push_str(&format!("{label}: {val}{suffix}\n"));
            }
        }

        if let Some(recent) = lifetime["Recent Results"].as_array() {
            let results: String = recent
                .iter()
                .map(|v| match v.as_str() {
                    Some("1") => "W",
                    Some("0") => "L",
                    _ => "?",
                })
                .collect::<Vec<_>>()
                .join("");
            out.push_str(&format!("Recent: {results}\n"));
        }

        out.push_str(&format!(
            "\nProfile: https://www.faceit.com/en/players/{nick}\n"
        ));
        Ok(out)
    }

    pub async fn get_matches(&self, nickname: &str, limit: usize) -> Result<String> {
        let limit = limit.min(20);
        let (player_id, _) = self.resolve_player_id(nickname).await?;

        let data = self
            .get(&format!(
                "/players/{player_id}/history?game=cs2&offset=0&limit={limit}"
            ))
            .await?;

        let items = data["items"]
            .as_array()
            .context("no match history in response")?;

        if items.is_empty() {
            return Ok(format!("No recent CS2 matches found for {nickname}"));
        }

        let mut out = format!(
            "=== Recent CS2 Matches for {nickname} ({} matches) ===\n\n",
            items.len()
        );

        for (i, item) in items.iter().enumerate() {
            let match_id = item["match_id"].as_str().unwrap_or("?");
            let started = item["started_at"].as_i64().unwrap_or(0);
            let finished = item["finished_at"].as_i64().unwrap_or(0);

            let winner = item["results"]["winner"].as_str().unwrap_or("");
            let player_team = find_player_team(item, &player_id);
            let result_str = if player_team.as_deref() == Some(winner) {
                "WIN"
            } else if winner.is_empty() {
                "?"
            } else {
                "LOSS"
            };

            let score = format!(
                "{} - {}",
                item["results"]["score"]["faction1"]
                    .as_u64()
                    .unwrap_or(0),
                item["results"]["score"]["faction2"]
                    .as_u64()
                    .unwrap_or(0),
            );

            let duration_mins = if finished > started && started > 0 {
                (finished - started) / 60
            } else {
                0
            };

            let comp = item["competition_name"].as_str().unwrap_or("");

            out.push_str(&format!(
                "{}. [{}] {} | Score: {} | {}m | {} | ID: {}\n",
                i + 1,
                result_str,
                format_unix_date(started),
                score,
                duration_mins,
                comp,
                match_id
            ));
        }

        Ok(out)
    }

    pub async fn get_match_details(&self, match_id: &str) -> Result<String> {
        let data = self
            .get(&format!("/matches/{match_id}/stats"))
            .await?;

        let rounds = data["rounds"]
            .as_array()
            .context("no rounds data in match stats")?;

        if rounds.is_empty() {
            return Ok(format!("No stats available for match {match_id}"));
        }

        let mut out = format!("=== Match Details: {match_id} ===\n\n");

        for (ri, round) in rounds.iter().enumerate() {
            let round_stats = &round["round_stats"];
            let map = round_stats["Map"].as_str().unwrap_or("?");
            let score = round_stats["Score"].as_str().unwrap_or("?");
            let winner_id = round_stats["Winner"].as_str().unwrap_or("");

            if rounds.len() > 1 {
                out.push_str(&format!("--- Round {} ---\n", ri + 1));
            }
            out.push_str(&format!("Map: {map} | Score: {score}\n\n"));

            if let Some(teams) = round["teams"].as_array() {
                for team in teams {
                    let team_id = team["team_id"].as_str().unwrap_or("");
                    let team_name = team["team_stats"]["Team"].as_str().unwrap_or("?");
                    let is_winner = !winner_id.is_empty() && team_id == winner_id;
                    let marker = if is_winner { " (WINNER)" } else { "" };

                    out.push_str(&format!("Team: {team_name}{marker}\n"));
                    out.push_str(&format!(
                        "{:<20} {:>5} {:>5} {:>5} {:>6} {:>5}\n",
                        "Player", "K", "D", "A", "K/D", "HS%"
                    ));
                    out.push_str(&format!("{}\n", "-".repeat(52)));

                    if let Some(players) = team["players"].as_array() {
                        for p in players {
                            let ps = &p["player_stats"];
                            let name = p["nickname"].as_str().unwrap_or("?");
                            let kills = ps["Kills"].as_str().unwrap_or("0");
                            let deaths = ps["Deaths"].as_str().unwrap_or("0");
                            let assists = ps["Assists"].as_str().unwrap_or("0");
                            let kd = ps["K/D Ratio"].as_str().unwrap_or("0.00");
                            let hs_pct = ps["Headshots %"].as_str().unwrap_or("0");
                            let display_name =
                                if name.len() > 20 { &name[..20] } else { name };
                            out.push_str(&format!(
                                "{:<20} {:>5} {:>5} {:>5} {:>6} {:>4}%\n",
                                display_name, kills, deaths, assists, kd, hs_pct
                            ));
                        }
                    }
                    out.push('\n');
                }
            }
        }

        out.push_str(&format!(
            "Match page: https://www.faceit.com/en/cs2/room/{match_id}\n"
        ));
        Ok(out)
    }

}



fn find_player_team(match_item: &Value, player_id: &str) -> Option<String> {
    for faction in ["faction1", "faction2"] {
        if let Some(players) = match_item["teams"][faction]["players"].as_array() {
            if players
                .iter()
                .any(|p| p["player_id"].as_str() == Some(player_id))
            {
                return Some(faction.to_string());
            }
        }
    }
    None
}

fn format_unix_date(ts: i64) -> String {
    if ts <= 0 {
        return "?".to_string();
    }
    let secs_per_day: i64 = 86400;
    let days = ts / secs_per_day;
    let time_secs = ts % secs_per_day;
    let h = time_secs / 3600;
    let m = (time_secs % 3600) / 60;
    let (year, month, day) = days_to_date(days);
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02} UTC")
}

/// Civil calendar conversion (Howard Hinnant's algorithm)
fn days_to_date(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, d)
}
