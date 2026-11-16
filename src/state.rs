use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

use crate::db::Db;
use crate::paths::Paths;

/// Everything the server, the scheduler and the menu-bar icon share.
pub struct AppState {
    pub db: Db,
    pub paths: Paths,
    pub http: reqwest::Client,
    /// Set when the app is being driven by an automated test: files and links that
    /// would normally be handed to other apps are only logged.
    pub test_mode: bool,
    /// True while a scan is in progress.
    pub scanning: AtomicBool,
    /// Wakes the scheduler for a scan the user asked for.
    pub refresh: Notify,
    /// Set by the menu-bar icon so it can show how many cards are waiting in New.
    pub badge: OnceLock<Box<dyn Fn(i64) + Send + Sync>>,
}

impl AppState {
    pub fn is_scanning(&self) -> bool {
        self.scanning.load(Ordering::SeqCst)
    }

    /// Asks for a scan now. Returns false when one is already running.
    pub fn request_scan(&self) -> bool {
        if self.is_scanning() {
            return false;
        }
        self.refresh.notify_one();
        true
    }

    pub fn update_badge(&self) {
        if let Some(show) = self.badge.get() {
            show(self.db.count_new());
        }
    }
}
