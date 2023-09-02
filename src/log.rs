use tracing_subscriber::EnvFilter;

pub fn init() {
    let log_json = std::env::var("LOG_JSON")
        .map(|s| s == "true")
        .unwrap_or(false);
    if log_json {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .json()
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .init();
    }
}
