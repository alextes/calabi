use std::{
    collections::{hash_map::Values, HashMap},
    fmt::Display,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Result};
use chrono::{Datelike, Utc};
use lazy_static::lazy_static;
use reqwest::{
    self,
    header::{AUTHORIZATION, CONTENT_TYPE},
    Client,
};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;
use tokio::{sync::Mutex, time::sleep};
use tracing::{debug, trace};

use crate::TargetIndicident;

const IBLUE_CREATOR_ID: &str = "HBlWMFF8XkcatdnIfNt0RPoCrXy1";
const ALEXTES_CREATOR_ID: &str = "fwGK5b9peFQbclczNeQdgCtjlYT2";
const MANIFOLD_MARKETS_API: &str = "https://manifold.markets/api";
const CONTENT_TYPE_APPLICATION_JSON: &str = "application/json";
const BET_PATH: &str = "/v0/bet";

lazy_static! {
    static ref MANIFOLD_API_KEY: String =
        std::env::var("MANIFOLD_API_KEY").expect("expected MANIFOLD_API_KEY set in environment");
    static ref AUTHORIZATION_KEY: String = format!("Key {}", *MANIFOLD_API_KEY);
    static ref MANIFOLD_BET_URL: String = format!("{}{}", MANIFOLD_MARKETS_API, BET_PATH);
}

enum Month {
    August,
    September,
    October,
    November,
    December,
}

impl From<Month> for u32 {
    fn from(month: Month) -> Self {
        match month {
            Month::August => 8,
            Month::September => 9,
            Month::October => 10,
            Month::November => 11,
            Month::December => 12,
        }
    }
}

fn month_from_question(question: &str) -> Option<Month> {
    if question.to_lowercase().contains("august") {
        Some(Month::August)
    } else if question.to_lowercase().contains("september") {
        Some(Month::September)
    } else if question.to_lowercase().contains("october") {
        Some(Month::October)
    } else if question.to_lowercase().contains("november") {
        Some(Month::November)
    } else if question.to_lowercase().contains("december") {
        Some(Month::December)
    } else {
        None
    }
}

fn day_from_question(question: &str) -> Option<u32> {
    let re = regex::Regex::new(r"on\s+\w+\s+(\d{1,2})").unwrap();
    re.captures(question)
        .and_then(|cap| cap.get(1).map(|m| m.as_str().parse().ok()))
        .flatten()
}

#[derive(Debug, Clone, PartialEq)]
pub enum IncidentType {
    Any,
    Red,
}

impl FromStr for IncidentType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "minor" => Ok(IncidentType::Any),
            "major" => Ok(IncidentType::Any),
            "critical" => Ok(IncidentType::Red),
            s => Err(anyhow!("unknown incident type: {}", s)),
        }
    }
}

impl Display for IncidentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IncidentType::Any => write!(f, "any"),
            IncidentType::Red => write!(f, "red"),
        }
    }
}

pub enum Outcome {
    Yes,
    #[allow(dead_code)]
    No,
}

impl Serialize for Outcome {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Outcome::Yes => serializer.serialize_str("YES"),
            Outcome::No => serializer.serialize_str("NO"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Market {
    id: String,
    creator_id: String,
    question: String,
}

impl Market {
    fn is_any_incident_market(&self) -> bool {
        (self.creator_id == IBLUE_CREATOR_ID || self.creator_id == ALEXTES_CREATOR_ID)
            && self.question.contains("Will GitHub have any incident")
    }

    fn is_red_incident_market(&self) -> bool {
        (self.creator_id == IBLUE_CREATOR_ID || self.creator_id == ALEXTES_CREATOR_ID)
            && self.question.contains("Will GitHub have a red incident")
    }
}

type Markets = Vec<Market>;

#[derive(Debug, Clone)]
pub struct ManifoldClient {
    base_url: String,
    client: Client,
}

impl ManifoldClient {
    pub fn new() -> Self {
        Self {
            base_url: MANIFOLD_MARKETS_API.to_string(),
            client: reqwest::Client::new(),
        }
    }

    async fn fetch_markets(&self) -> Result<Markets> {
        let response = self
            .client
            .get(&format!("{}/v0/markets", self.base_url))
            .header(AUTHORIZATION, &*AUTHORIZATION_KEY)
            .send()
            .await?;

        match response.error_for_status() {
            Ok(response) => {
                let markets = response.json::<Markets>().await?;
                Ok(markets)
            }
            Err(err) => Err(err.into()),
        }
    }

    pub async fn bet(&self, contract_id: &str, outcome: &Outcome, amount: u32) -> Result<()> {
        let payload = json!({
            "amount": amount,
            "outcome": outcome,
            "contractId": contract_id,
        });

        let response = self
            .client
            .post(&*MANIFOLD_BET_URL)
            .header(CONTENT_TYPE, CONTENT_TYPE_APPLICATION_JSON)
            .header(AUTHORIZATION, &*AUTHORIZATION_KEY)
            .json(&payload)
            .send()
            .await?;

        match response.error_for_status() {
            Ok(response) => {
                debug!(status = %response.status(), "bet placed");
                Ok(())
            }
            Err(err) => Err(err.into()),
        }
    }
}

const CHECK_MARKETS_INTERVAL_SECONDS: u64 = 6;

#[derive(Debug)]
pub struct TargetMarkets(HashMap<String, TargetIndicident>);

impl TargetMarkets {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    fn add_new_target(&mut self, target: TargetIndicident) {
        self.0.insert(target.contract_id.clone(), target);
    }

    fn target_exists(&self, contract_id: &str) -> bool {
        self.0.contains_key(contract_id)
    }

    fn clear_old_targets(&mut self) {
        let today = Utc::now();
        self.0.retain(|_key, target| {
            let TargetIndicident { month, day, .. } = target;
            // Keep targets that are for future months, or future days of the current month.
            *month > today.month() || (*month == today.month() && *day >= today.day())
        });
    }

    pub fn targets(&self) -> Values<String, TargetIndicident> {
        self.0.values()
    }
}

pub async fn update_targets(
    manifold_client: &ManifoldClient,
    target_markets: Arc<Mutex<TargetMarkets>>,
) -> Result<()> {
    loop {
        debug!("checking for new targets");

        for target in target_markets.lock().await.targets() {
            debug!(?target, "current target");
        }

        let markets = manifold_client.fetch_markets().await?;

        target_markets.lock().await.clear_old_targets();

        for market in markets {
            if market.is_any_incident_market() {
                let target = TargetIndicident {
                    contract_id: market.id,
                    day: day_from_question(&market.question)
                        .expect("failed to parse day from question"),
                    incident_type: IncidentType::Any,
                    month: month_from_question(&market.question)
                        .expect("failed to parse month from question")
                        .into(),
                };

                if target.is_past() {
                    trace!(?target, "found past target, skipping");
                    continue;
                }

                if target_markets
                    .lock()
                    .await
                    .target_exists(&target.contract_id)
                {
                    continue;
                }

                debug!(?target, "found new any incident target");
                target_markets.lock().await.add_new_target(target);

                // TODO: get the current bets for the market, if you haven't already taken a NO
                // position, take a NO position.
            } else if market.is_red_incident_market() {
                let target = TargetIndicident {
                    contract_id: market.id,
                    day: day_from_question(&market.question)
                        .expect("failed to parse day from question"),
                    incident_type: IncidentType::Red,
                    month: month_from_question(&market.question)
                        .expect("failed to parse month from question")
                        .into(),
                };

                if target.is_past() {
                    trace!(?target, "found past target, skipping");
                    continue;
                }

                if target_markets
                    .lock()
                    .await
                    .target_exists(&target.contract_id)
                {
                    continue;
                }

                debug!(?target, "found new red incident target");
                target_markets.lock().await.add_new_target(target);

                // TODO: get the current bets for the market, if you haven't already taken a NO
                // position, take a NO position.
            }
        }

        sleep(Duration::from_secs(CHECK_MARKETS_INTERVAL_SECONDS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::day_from_question;

    #[test]
    fn test_day_from_question() {
        // Test original examples
        assert_eq!(
            day_from_question("Will GitHub have any incident on August 30th 2023?"),
            Some(30)
        );
        assert_eq!(
            day_from_question("Will GitHub have a red incident on August 30th 2023?"),
            Some(30)
        );

        // Test ordinal suffixes
        assert_eq!(
            day_from_question("Will GitHub have any incident on August 1st 2023?"),
            Some(1)
        );
        assert_eq!(
            day_from_question("Will GitHub have any incident on August 01st 2023?"),
            Some(1)
        );

        // Test empty case
        assert_eq!(day_from_question("Will GitHub have any incident?"), None);

        // Test without the word "on"
        assert_eq!(
            day_from_question("Will GitHub have any incident August 30th 2023?"),
            None
        );

        // Test multiple occurrences of "on"
        assert_eq!(
            day_from_question("Will GitHub have any incident on on August 30th 2023?"),
            Some(30)
        );
    }
}
