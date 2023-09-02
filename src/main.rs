mod github_status;
mod log;
mod manifold_markets;

use std::{sync::Arc, time::Duration};

use anyhow::Result;
use chrono::{DateTime, Datelike, Utc};
use futures::future::try_join_all;
use manifold_markets::{IncidentType, ManifoldClient, TargetMarkets};
use reqwest::{self, Client};
use tokio::{select, sync::Mutex, time::sleep};
use tracing::{debug, info, warn};

use crate::manifold_markets::Outcome;

const GITHUB_POLL_INTERVAL_MS: u64 = 500;
const DEFAULT_BET_SIZE: u32 = 200;
const DATE_EXCLUSION_LIST: [(u32, u32); 1] = [(8, 30)];
const EXCLUSION_DAY_SLEEP_MINUTES: u64 = 20;

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

    fn matches(&self, now: &DateTime<Utc>, incident_type: &IncidentType) -> bool {
        self.month == now.month() && self.day == now.day() && self.incident_type == *incident_type
    }
}

async fn scan_targets(
    github_client: &Client,
    manifold_client: &ManifoldClient,
    target_markets: Arc<Mutex<TargetMarkets>>,
) -> Result<()> {
    loop {
        let now = Utc::now();

        if DATE_EXCLUSION_LIST.contains(&(now.month(), now.day())) {
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

        info!(
            indicator = %current_incident_type,
            description = response.description(),
            "GitHub has an incident!"
        );

        if current_incident_type == IncidentType::Red {
            info!("It's a red incident 🤑!");
        }

        let live_target_count = target_markets.lock().await.targets().len();

        debug!(count = live_target_count, "have live targets");

        let matching_targets: Vec<TargetIndicident> = target_markets
            .lock()
            .await
            .targets()
            .cloned()
            .filter(|target| target.matches(&now, &current_incident_type))
            .collect();

        if matching_targets.is_empty() {
            let target_markets = target_markets.lock().await;
            warn!(
                incident_type = %current_incident_type,
                today_month = now.month(),
                today_day = now.day(),
                target_markets = ?target_markets,
                "GitHub has an incident, but we have no matching targets, did we fail to fetch the target market?",
            );
        } else {
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
                for _i in 0..2 {
                    tasks.push(manifold_client.bet(
                        &target.contract_id,
                        &Outcome::Yes,
                        DEFAULT_BET_SIZE,
                    ));
                }
            }

            try_join_all(tasks).await?;

            info!("bets placed, sleeping to avoid betting again");
            sleep(Duration::from_secs(u64::MAX)).await;
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
