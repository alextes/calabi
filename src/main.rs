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

struct TargetIndicident {
    month: u32,
    day: u32,
    contract_id: &'static str,
    red_contract_id: &'static str,
}
const TARGET_A: TargetIndicident = TargetIndicident {
    month: 8,
    day: 29,
    contract_id: "5kFCX8YfjxNCYTLXMzT9",
    red_contract_id: "o2AilVT2jmmef8YIGkzC",
};
const TARGET_B: TargetIndicident = TargetIndicident {
    month: 8,
    day: 30,
    contract_id: "hsXruT9P074SyAgWUX1L",
    red_contract_id: "vYSLqU2aGD6ZTdtUQUKY",
};
const TARGETS: &[TargetIndicident] = &[TARGET_A, TARGET_B];
const CONTENT_TYPE_APPLICATION_JSON: &str = "application/json";

lazy_static! {
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

async fn bet_20_github_down(client: &Client, contract_id: &str) -> Result<()> {
    let payload = json!({
        "amount": 80,
        "outcome": "YES",
        "contractId": contract_id,
    });

    let response = client
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

    for target in TARGETS {
        info!(
            contract_id = target.contract_id,
            red_contract_id = target.red_contract_id,
            month = target.month,
            day = target.day,
            "target date for contract",
        );
    }

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
        for target in TARGETS {
            let TargetIndicident {
                month,
                day,
                contract_id,
                red_contract_id,
            } = target;

            if today.month() == *month || today.day() == *day {
                debug!(
                    contract_id,
                    month, day, "today matches the target date of the contract",
                );

                let mut handles = Vec::new();

                for _ in 0..5 {
                    if response.status.indicator == "critical" {
                        handles.push(bet_20_github_down(&manifold_client, red_contract_id));
                    }
                    handles.push(bet_20_github_down(&manifold_client, contract_id));
                }

                try_join_all(handles).await?;

                info!("bets placed, sleeping to avoid betting again");
                sleep(Duration::from_secs(u64::MAX)).await;
            } else {
                debug!(
                    month,
                    day, "GitHub has an incident, but today does not match target",
                );
                sleep(Duration::from_millis(GITHUB_POLL_INTERVAL_MS)).await;
            }
        }
    }
}
