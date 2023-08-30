use anyhow::{Context, Result};
use backoff::{future::retry, ExponentialBackoff};
use reqwest::{self, Client, StatusCode};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Status {
    description: String,
    indicator: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct StatusEnvelope {
    status: Status,
}

impl StatusEnvelope {
    pub fn description(&self) -> &str {
        &self.status.description
    }

    pub fn indicator(&self) -> &str {
        &self.status.indicator
    }

    pub fn is_ok(&self) -> bool {
        self.status.indicator == "none"
    }
}

const GITHUB_STATUS_URL: &str = "https://www.githubstatus.com/api/v2/status.json";

/// Get the current GitHub incident status.
/// Uses an exponential backoff on 429s, errors out on anything else.
pub async fn get_incident_status(github_client: &Client) -> Result<StatusEnvelope> {
    use backoff::Error;

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
