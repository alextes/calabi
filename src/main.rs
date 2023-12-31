//! # Calabi
//! Calabi is a bot that bets on the outcome of GitHub incidents on [Manifold](https://www.manifold.co/).
//!
//! ## Improvements
//! - [ ] Periodically check if we have bet yes on a target market. If so, add the market to your
//! exclusion list. https://docs.manifold.markets/api#get-v0marketmarketidpositions
//! - [ ] When betting on a target market, add the market to the exclusion list when finished.
mod github_status;
mod log;
mod manifold_markets;

use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::Result;
use chrono::{Datelike, NaiveDate, Utc};
use futures::future::try_join_all;
use lazy_static::lazy_static;
use manifold_markets::{IncidentType, ManifoldClient, TargetMarkets};
use reqwest::{self, Client};
use tokio::{select, sync::Mutex, time::sleep};
use tracing::{debug, info};

use crate::manifold_markets::Outcome;

const GITHUB_POLL_INTERVAL_MS: u64 = 500;
const DEFAULT_BET_SIZE: u32 = 500;
const NR_OF_BETS: u32 = 2;
const EXCLUSION_DAY_SLEEP_MINUTES: u64 = 20;

lazy_static! {
    static ref DATE_EXCLUSION_LIST: [NaiveDate; 1] = [NaiveDate::from_ymd_opt(2023, 9, 6).unwrap()];
}

#[derive(Debug, Clone)]
pub struct TargetIndicident {
    contract_id: String,
    day: u32,
    incident_type: IncidentType,
    month: u32,
}

impl TargetIndicident {
    fn is_past(&self) -> bool {
        let today = Utc::now();
        today.month() > self.month || (today.month() == self.month && today.day() > self.day)
    }

    fn matches(&self, now: &NaiveDate, incident_type: &IncidentType) -> bool {
        self.month == now.month() && self.day == now.day() && self.incident_type == *incident_type
    }
}

async fn scan_targets(
    github_client: &Client,
    manifold_client: &ManifoldClient,
    target_markets: Arc<Mutex<TargetMarkets>>,
) -> Result<()> {
    let mut contract_exclusion_list: HashSet<String> = HashSet::new();

    loop {
        let now = Utc::now().date_naive();

        if DATE_EXCLUSION_LIST.contains(&now) {
            info!(
                today_month = now.month(),
                today_day = now.day(),
                "today is on the exclusion list, sleeping for {} minutes",
                EXCLUSION_DAY_SLEEP_MINUTES
            );
            sleep(Duration::from_secs(60 * EXCLUSION_DAY_SLEEP_MINUTES)).await;
            continue;
        }

        let response = github_status::get_incident_status(github_client).await?;

        if response.is_ok() {
            debug!("GitHub is working fine, nothing to do, sleeping");
            sleep(Duration::from_millis(GITHUB_POLL_INTERVAL_MS)).await;
            continue;
        }

        let current_incident_type: IncidentType = response.indicator().parse()?;

        debug!(
            indicator = %current_incident_type,
            description = response.description(),
            "GitHub has an incident!"
        );

        if current_incident_type == IncidentType::Red {
            info!("It's a red incident 🤑!");
        }

        let live_target_count = target_markets.lock().await.len();

        debug!(count = live_target_count, "have live targets");

        let target_markets = target_markets.lock().await;

        let matching_targets = target_markets.matching_targets(&now, &current_incident_type);
        debug!(count = matching_targets.len(), "have matching targets");

        let matching_targets = matching_targets
            .into_iter()
            .filter(|target| !contract_exclusion_list.contains(&target.contract_id))
            .collect::<Vec<_>>();
        debug!(
            count = matching_targets.len(),
            "have matching targets not on exclusion list"
        );

        let mut tasks = Vec::new();

        for target in &matching_targets {
            debug!(
                incident_type = %current_incident_type,
                today_month = now.month(),
                today_day = now.day(),
                target_month = target.month,
                target_day = target.day,
                "target matches incident, queuing bet",
            );

            // Bet three times on each target.
            // We don't know how much mana we have to spend.
            for _ in 0..NR_OF_BETS {
                tasks.push(manifold_client.bet(
                    &target.contract_id,
                    &Outcome::Yes,
                    DEFAULT_BET_SIZE,
                ));
            }
        }

        try_join_all(tasks).await?;
        info!("bets placed");

        // Add matching targets to the exclusion list.
        for target in matching_targets {
            contract_exclusion_list.insert(target.contract_id.clone());
        }

        sleep(Duration::from_millis(GITHUB_POLL_INTERVAL_MS)).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    log::init();

    info!("starting calabi, where's yau?");

    let github_client = reqwest::Client::new();
    let manifold_client = ManifoldClient::new();

    let targets = Arc::new(Mutex::new(TargetMarkets::new()));

    let update_targets_thread = tokio::spawn({
        let manifold_client = manifold_client.clone();
        let targets = targets.clone();
        async move { manifold_markets::update_targets(&manifold_client, targets).await }
    });

    let scan_targets_thread = tokio::spawn({
        async move { scan_targets(&github_client, &manifold_client, targets).await }
    });

    select!(
        result = update_targets_thread => result.unwrap(),
        result = scan_targets_thread => result.unwrap()
    )?;

    Ok(())
}
