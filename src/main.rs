use std::time::Duration;

use anyhow::{Context, Result};
use backoff::{future::retry, ExponentialBackoff};
use chrono::{Datelike, Utc};
use futures::future::try_join_all;
use lazy_static::lazy_static;
use reqwest::{
    self,
    header::{AUTHORIZATION, CONTENT_TYPE},
    Client, StatusCode,
};
use serde::Deserialize;
use serde_json::json;
use tokio::time::sleep;
use tracing::{debug, info};

const MANIFOLD_MARKETS_API: &str = "https://manifold.markets/api";
const BET_PATH: &str = "/v0/bet";

const GITHUB_DOWN_AUG_24_CONTRACT_ID: &str = "RdZp1bQpW8ZqKQOzuqXp";
const GITHUB_DOWN_AUG_24_RED_CONTRACT_ID: &str = "G8UaeuL7taEHvNQS8A6Y";
const TARGET_MONTH: u64 = 8;
const TARGET_DAY: u64 = 23;
const CONTENT_TYPE_APPLICATION_JSON: &str = "application/json";

lazy_static! {
    static ref GITHUB_DOWN_BET_YES_PAYLOAD: serde_json::Value = json!({
        "amount": 80,
        "outcome": "YES",
        "contractId": GITHUB_DOWN_AUG_24_CONTRACT_ID,
    });
    static ref GITHUB_DOWN_RED_BET_YES_PAYLOAD: serde_json::Value = json!({
        "amount": 80,
        "outcome": "YES",
        "contractId": GITHUB_DOWN_AUG_24_RED_CONTRACT_ID,
    });
    static ref MANIFOLD_API_KEY: String =
        std::env::var("MANIFOLD_API_KEY").expect("MANIFOLD_API_KEY not set in environment");
    static ref AUTHORIZATION_KEY: String = format!("Key {}", *MANIFOLD_API_KEY);
    static ref MANIFOLD_BET_URL: String = format!("{}{}", MANIFOLD_MARKETS_API, BET_PATH);
}

#[derive(Debug, Deserialize)]
struct Status {
    description: String,
    indicator: String,
}

#[derive(Debug, serde::Deserialize)]
struct StatusEnvelope {
    status: Status,
}

async fn bet_20_github_down(client: &Client, red: bool) -> Result<()> {
    let payload = if red {
        &*GITHUB_DOWN_RED_BET_YES_PAYLOAD
    } else {
        &*GITHUB_DOWN_BET_YES_PAYLOAD
    };

    let response = client
        .post(&*MANIFOLD_BET_URL)
        .header(CONTENT_TYPE, CONTENT_TYPE_APPLICATION_JSON)
        .header(AUTHORIZATION, &*AUTHORIZATION_KEY)
        .json(payload)
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

const GITHUB_STATUS_URL: &str = "https://www.githubstatus.com/api/v2/status.json";
const GITHUB_POLL_INTERVAL_MS: u64 = 500;

/// Get the current GitHub incident status.
/// Uses an exponential backoff on 429s, errors out on anything else.
async fn get_incident_status() -> Result<StatusEnvelope> {
    use backoff::Error;

    let github_client = reqwest::Client::new();

    let get_status_with_backoff = || async {
        github_client
            .get(GITHUB_STATUS_URL)
            .send()
            .await
            .map_err(Error::Permanent)?
            .error_for_status()
            .map_err(|err| {
                if err.status() == Some(StatusCode::TOO_MANY_REQUESTS) {
                    Error::Transient {
                        err,
                        retry_after: None,
                    }
                } else {
                    Error::Permanent(err)
                }
            })?
            .json::<StatusEnvelope>()
            .await
            .map_err(Error::Permanent)
    };

    retry(ExponentialBackoff::default(), get_status_with_backoff)
        .await
        .context("failed to get GitHub status")
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    info!("starting calabi, where's yau?");

    let manifold_client = reqwest::Client::new();

    loop {
        let response = get_incident_status().await?;
        if response.status.indicator == "none" {
            debug!("GitHub is working fine, nothing to do, sleeping");
            sleep(Duration::from_millis(GITHUB_POLL_INTERVAL_MS)).await;
            continue;
        }

        info!(
            indicator = response.status.indicator,
            description = response.status.description,
            "GitHub has an incident!"
        );

        if response.status.indicator == "critical" {
            info!("It's a red incident 🤑!");
        }

        let today = Utc::now();
        if today.month() == 8 && today.day() == 24 {
            debug!(
                GITHUB_DOWN_AUG_24_CONTRACT_ID,
                TARGET_MONTH, TARGET_DAY, "today matches the target date of the contract",
            );

            let mut handles = Vec::new();

            for _ in 0..5 {
                if response.status.indicator == "critical" {
                    handles.push(bet_20_github_down(&manifold_client, true));
                }
                handles.push(bet_20_github_down(&manifold_client, false));
            }

            try_join_all(handles).await?;

            info!("bets placed, sleeping to avoid betting again");
            sleep(Duration::from_secs(u64::MAX)).await;
        } else {
            debug!(
                TARGET_MONTH,
                TARGET_DAY, "GitHub has an incident, but today does not match target",
            );
            sleep(Duration::from_millis(GITHUB_POLL_INTERVAL_MS)).await;
        }
    }
}
