//! Runs a scan twice a day, and whenever the user asks for one.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::state::AppState;
use crate::{log_info, log_warn, scan};

/// Twice a day.
pub const SCAN_INTERVAL_HOURS: i64 = 12;

/// When a scan reached no source at all (offline, most likely), try again sooner.
const RETRY_MINUTES: i64 = 30;

/// When the next automatic scan should start.
pub fn next_due(state: &AppState) -> DateTime<Utc> {
    let Ok(Some(last)) = state.db.last_run() else {
        return Utc::now();
    };
    let finished = last
        .finished_at
        .as_deref()
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|parsed| parsed.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    if last.ok {
        finished + chrono::Duration::hours(SCAN_INTERVAL_HOURS)
    } else {
        finished + chrono::Duration::minutes(RETRY_MINUTES)
    }
}

pub async fn scan_once(state: &AppState) -> bool {
    state.scanning.store(true, Ordering::SeqCst);
    log_info!("scan started");
    let outcome = scan::run(state).await;
    state.scanning.store(false, Ordering::SeqCst);
    state.update_badge();
    match outcome {
        Ok(added) => {
            log_info!("scan finished: {added} new job(s)");
            true
        }
        Err(e) => {
            log_warn!("scan failed: {e:#}");
            false
        }
    }
}

pub async fn run(state: Arc<AppState>) {
    loop {
        // The due time is compared against the wall clock once a minute rather than
        // slept towards, so a laptop that was asleep catches up as soon as it wakes.
        // Until the user has said which roles and regions they want, an automatic scan
        // would fill the board with every remote job there is. The first scan starts
        // when they save their settings (or press "Check now").
        if Utc::now() >= next_due(&state) && state.db.settings().configured {
            if !scan_once(&state).await {
                // Something is wrong beyond a source being down; do not spin.
                tokio::time::sleep(Duration::from_secs(300)).await;
            }
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            _ = state.refresh.notified() => {
                scan_once(&state).await;
            }
        }
    }
}
