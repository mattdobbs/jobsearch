//! Where jobs come from.
//!
//! `ats` reads employers' own career sites (the direct listings); `boards` reads
//! remote-only job boards and the Hire Autism board. Every fetcher only gathers
//! postings and reports what each one's listing says about where the work is done —
//! deciding which ones become cards happens in `scan`.

mod ats;
mod boards;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use reqwest::Client;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::employers::Employer;
use crate::model::{Candidate, Workplaces};

/// Identifies the app honestly to the sites it reads. Contains nothing about the user.
const USER_AGENT: &str = "jobsearch-board/0.1 (personal job-tracking app)";

/// How many sources are fetched at once.
const CONCURRENCY: usize = 6;

pub struct SourceResult {
    pub source: String,
    pub label: String,
    /// Employer career sites are processed before job boards, so that when both carry
    /// the same job the card links to the employer's own listing.
    pub direct: bool,
    pub candidates: Vec<Candidate>,
    pub error: Option<String>,
}

pub fn http_client() -> anyhow::Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(15))
        .build()
        .context("could not create the HTTP client")
}

/// Fetches every source. A source that fails reports its error and yields nothing;
/// it never stops the others. `wanted` is the kinds of job the user asked for: the
/// sources that have to be asked for one kind at a time are asked only for those.
pub async fn fetch_all(http: &Client, employers: &[Employer], wanted: Workplaces) -> Vec<SourceResult> {
    let limit = Arc::new(Semaphore::new(CONCURRENCY));
    let mut tasks: JoinSet<SourceResult> = JoinSet::new();

    for employer in employers.iter().filter(|e| !e.ats.is_empty()) {
        let (http, employer, limit) = (http.clone(), employer.clone(), limit.clone());
        tasks.spawn(async move {
            let _permit = limit.acquire().await;
            let outcome = ats::fetch(&http, &employer, wanted).await;
            finish(ats::source_id(&employer), format!("{} careers", employer.name), true, outcome)
        });
    }

    for board in boards::ALL {
        let (http, limit) = (http.clone(), limit.clone());
        tasks.spawn(async move {
            let _permit = limit.acquire().await;
            let outcome = boards::fetch(&http, board, wanted).await;
            finish(board.id.to_string(), board.label.to_string(), false, outcome)
        });
    }

    let mut results = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(result) => results.push(result),
            Err(e) => crate::log_warn!("a source task failed: {e}"),
        }
    }
    results.sort_by(|a, b| b.direct.cmp(&a.direct).then_with(|| a.label.cmp(&b.label)));
    results
}

fn finish(source: String, label: String, direct: bool, outcome: anyhow::Result<Vec<Candidate>>) -> SourceResult {
    match outcome {
        Ok(candidates) => SourceResult {
            source,
            label,
            direct,
            candidates,
            error: None,
        },
        Err(e) => {
            crate::log_warn!("{label}: {e:#}");
            SourceResult {
                source,
                label,
                direct,
                candidates: Vec::new(),
                error: Some(format!("{e:#}")),
            }
        }
    }
}

// ---- shared helpers -----------------------------------------------------------

fn host_of(url: &str) -> &str {
    url.split("://").nth(1).unwrap_or(url).split('/').next().unwrap_or(url)
}

/// A site answered that it is getting too many requests. The caller should ease off
/// and come back, not treat the site as broken.
#[derive(Debug)]
pub struct RateLimited {
    host: String,
}

impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} asked for fewer requests (HTTP 429)", self.host)
    }
}

impl std::error::Error for RateLimited {}

pub fn is_rate_limited(error: &anyhow::Error) -> bool {
    error.downcast_ref::<RateLimited>().is_some()
}

async fn check(response: reqwest::Response, url: &str) -> anyhow::Result<reqwest::Response> {
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(RateLimited {
            host: host_of(url).to_string(),
        }
        .into());
    }
    anyhow::ensure!(status.is_success(), "{} answered HTTP {}", host_of(url), status.as_u16());
    Ok(response)
}

pub async fn get_json(http: &Client, url: &str) -> anyhow::Result<Value> {
    let response = http
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .with_context(|| format!("could not reach {}", host_of(url)))?;
    check(response, url)
        .await?
        .json()
        .await
        .with_context(|| format!("{} sent something that is not JSON", host_of(url)))
}

pub async fn post_json(http: &Client, url: &str, body: &Value) -> anyhow::Result<Value> {
    let response = http
        .post(url)
        .header("Accept", "application/json")
        .json(body)
        .send()
        .await
        .with_context(|| format!("could not reach {}", host_of(url)))?;
    check(response, url)
        .await?
        .json()
        .await
        .with_context(|| format!("{} sent something that is not JSON", host_of(url)))
}

pub async fn get_text(http: &Client, url: &str) -> anyhow::Result<String> {
    let response = http
        .get(url)
        .send()
        .await
        .with_context(|| format!("could not reach {}", host_of(url)))?;
    Ok(check(response, url).await?.text().await?)
}

/// Reads a JSON field as text, whether it was sent as a string or a number.
fn text(value: &Value, key: &str) -> String {
    match &value[key] {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn iso(moment: DateTime<Utc>) -> String {
    moment.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn iso_from_epoch(seconds: i64) -> Option<String> {
    Utc.timestamp_opt(seconds, 0).single().map(iso)
}

/// Normalises the date formats the sources use into RFC 3339.
fn iso_from_str(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        return Some(iso(parsed.with_timezone(&Utc)));
    }
    if let Ok(parsed) = DateTime::parse_from_rfc2822(raw) {
        return Some(iso(parsed.with_timezone(&Utc)));
    }
    if let Ok(parsed) = NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S") {
        return Some(iso(parsed.and_utc()));
    }
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|d| iso(d.and_utc()))
}

/// Formats a pay range for display, e.g. "USD 120,000 – 150,000 / year".
fn salary_range(min: Option<f64>, max: Option<f64>, currency: &str, period: &str) -> String {
    fn grouped(amount: f64) -> String {
        let digits = format!("{}", amount.round() as i64);
        let mut out = String::new();
        for (i, ch) in digits.chars().enumerate() {
            if i > 0 && (digits.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(ch);
        }
        out
    }
    let range = match (min, max) {
        (Some(lo), Some(hi)) if hi > lo => format!("{} – {}", grouped(lo), grouped(hi)),
        (Some(only), _) | (None, Some(only)) => grouped(only),
        (None, None) => return String::new(),
    };
    let mut out = String::new();
    if !currency.is_empty() {
        out.push_str(currency);
        out.push(' ');
    }
    out.push_str(&range);
    let period = period.trim_end_matches("ly").trim_start_matches("per-").to_lowercase();
    if !period.is_empty() {
        out.push_str(" / ");
        out.push_str(match period.as_str() {
            "annual" | "year" | "yearly" => "year",
            "month" => "month",
            "week" => "week",
            "hour" => "hour",
            other => other,
        });
    }
    out
}

/// Percent-encodes a value for use in a URL query or an HTTP header parameter.
pub fn encode_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Reverses `encode_component`. Anything that is not a valid escape is kept as it is.
pub fn decode_component(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| raw.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Fetches the full description for a posting that was listed without one.
pub async fn fetch_detail(http: &Client, candidate: &mut Candidate) -> anyhow::Result<()> {
    let Some(url) = candidate.detail_url.clone() else {
        return Ok(());
    };
    ats::workday_detail(http, &url, candidate).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_to_slow_down_is_told_apart_from_a_failure() {
        let slow_down: anyhow::Error = RateLimited { host: "example.com".into() }.into();
        assert!(is_rate_limited(&slow_down));
        // Still recognised once a caller has added its own explanation.
        assert!(is_rate_limited(&slow_down.context("could not read the posting")));
        assert!(!is_rate_limited(&anyhow::anyhow!("example.com answered HTTP 500")));
    }

    #[test]
    fn dates_are_normalised() {
        assert_eq!(iso_from_str("2026-11-14T20:33:53").as_deref(), Some("2026-11-14T20:33:53Z"));
        assert_eq!(iso_from_str("2026-11-13T09:12:26-05:00").as_deref(), Some("2026-11-13T14:12:26Z"));
        assert_eq!(iso_from_str("Mon, 16 Nov 2026 10:51:07 +0000").as_deref(), Some("2026-11-16T10:51:07Z"));
        assert_eq!(iso_from_str("2026-10-28").as_deref(), Some("2026-10-28T00:00:00Z"));
        assert_eq!(iso_from_str("Posted 3 Days Ago"), None);
        assert_eq!(iso_from_epoch(1794834471).as_deref(), Some("2026-11-16T13:07:51Z"));
    }

    #[test]
    fn salary_ranges_are_readable() {
        assert_eq!(salary_range(Some(222500.0), Some(257500.0), "USD", "yearly"), "USD 222,500 – 257,500 / year");
        assert_eq!(salary_range(Some(90000.0), None, "", "annual"), "90,000 / year");
        assert_eq!(salary_range(Some(45.0), Some(60.0), "USD", "hourly"), "USD 45 – 60 / hour");
        assert_eq!(salary_range(None, None, "USD", "year"), "");
    }

    #[test]
    fn url_components_are_encoded() {
        assert_eq!(encode_component("MjAy+Ni/x=="), "MjAy%2BNi%2Fx%3D%3D");
        assert_eq!(encode_component("résumé v2.pdf"), "r%C3%A9sum%C3%A9%20v2.pdf");
    }

    #[test]
    fn url_components_are_decoded() {
        assert_eq!(decode_component("r%C3%A9sum%C3%A9%20v2.pdf"), "résumé v2.pdf");
        assert_eq!(decode_component(&encode_component("a/b c+d=é%.txt")), "a/b c+d=é%.txt");
        // Broken escapes are left alone rather than rejected.
        assert_eq!(decode_component("100%"), "100%");
        assert_eq!(decode_component("50%zz"), "50%zz");
        assert_eq!(decode_component("%4"), "%4");
    }

    #[test]
    fn host_is_extracted_for_error_messages() {
        assert_eq!(host_of("https://api.lever.co/v0/postings/x?mode=json"), "api.lever.co");
    }
}
