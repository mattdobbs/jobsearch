//! Job boards. Six list remote jobs only; Hire Autism is a board for autistic job
//! seekers and is asked for its remote listings, or for all of them when the user
//! also wants hybrid or on-site jobs.
//!
//! Board postings carry no claim about the employer, so (Hire Autism aside) each one
//! has to earn its place through what the posting itself says.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::Context;
use regex::Regex;
use reqwest::Client;
use serde_json::Value;

use super::{encode_component, get_json, get_text, iso_from_epoch, iso_from_str, salary_range, text};
use crate::filter::plain_text;
use crate::model::{Candidate, Inclusion, Remote, Workplaces};

#[derive(Debug, Clone, Copy)]
pub struct Board {
    pub id: &'static str,
    pub label: &'static str,
}

pub const ALL: [Board; 7] = [
    Board { id: "hireautism", label: "Hire Autism" },
    Board { id: "remotive", label: "Remotive" },
    Board { id: "remoteok", label: "Remote OK" },
    Board { id: "jobicy", label: "Jobicy" },
    Board { id: "himalayas", label: "Himalayas" },
    Board { id: "weworkremotely", label: "We Work Remotely" },
    Board { id: "workingnomads", label: "Working Nomads" },
];

pub async fn fetch(http: &Client, board: Board, wanted: Workplaces) -> anyhow::Result<Vec<Candidate>> {
    let found = match board.id {
        "hireautism" => hire_autism(http, board, wanted).await?,
        "remotive" => remotive(http, board).await?,
        "remoteok" => remote_ok(http, board).await?,
        "jobicy" => jobicy(http, board).await?,
        "himalayas" => himalayas(http, board).await?,
        "weworkremotely" => we_work_remotely(http, board).await?,
        "workingnomads" => working_nomads(http, board).await?,
        other => anyhow::bail!("unknown board {other}"),
    };
    Ok(found
        .into_iter()
        .filter(|c| !c.external_id.is_empty() && !c.title.is_empty() && c.url.starts_with("http"))
        .collect())
}

/// A posting from a remote-only board: remote by definition, inclusive only if it says so.
fn candidate(board: Board, id: String, title: String, company: String) -> Candidate {
    Candidate {
        source: board.id.to_string(),
        source_label: board.label.to_string(),
        external_id: id,
        title,
        company,
        location: String::new(),
        url: String::new(),
        description_html: String::new(),
        salary: String::new(),
        posted_at: None,
        remote: Remote::Declared,
        inclusion: Inclusion::NeedsSignal,
        direct: false,
        detail_url: None,
    }
}

// ---- Remotive -------------------------------------------------------------------

async fn remotive(http: &Client, board: Board) -> anyhow::Result<Vec<Candidate>> {
    let body = get_json(http, "https://remotive.com/api/remote-jobs").await?;
    let jobs = body["jobs"].as_array().context("Remotive sent no job list")?;
    Ok(jobs
        .iter()
        .map(|job| {
            let mut c = candidate(board, text(job, "id"), text(job, "title"), text(job, "company_name"));
            c.location = text(job, "candidate_required_location");
            c.url = text(job, "url");
            c.description_html = text(job, "description");
            c.salary = text(job, "salary");
            c.posted_at = iso_from_str(&text(job, "publication_date"));
            c
        })
        .collect())
}

// ---- Remote OK ------------------------------------------------------------------

async fn remote_ok(http: &Client, board: Board) -> anyhow::Result<Vec<Candidate>> {
    let body = get_json(http, "https://remoteok.com/api").await?;
    let jobs = body.as_array().context("Remote OK sent no job list")?;
    Ok(jobs
        .iter()
        // The first element is a legal notice, not a job.
        .filter(|job| job["position"].is_string())
        .map(|job| {
            let mut c = candidate(board, text(job, "id"), text(job, "position"), text(job, "company"));
            c.location = text(job, "location");
            c.url = text(job, "url");
            c.description_html = text(job, "description");
            c.salary = salary_range(
                job["salary_min"].as_f64().filter(|v| *v > 0.0),
                job["salary_max"].as_f64().filter(|v| *v > 0.0),
                "USD",
                "year",
            );
            c.posted_at = iso_from_str(&text(job, "date"));
            c
        })
        .collect())
}

// ---- Jobicy ---------------------------------------------------------------------

async fn jobicy(http: &Client, board: Board) -> anyhow::Result<Vec<Candidate>> {
    // The latest jobs, plus Jobicy's own search for postings that use these words.
    const QUERIES: [&str; 5] = [
        "count=100",
        "count=100&tag=neurodiversity",
        "count=100&tag=neurodivergent",
        "count=100&tag=neurodiverse",
        "count=100&tag=disabilities",
    ];
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for (index, query) in QUERIES.iter().enumerate() {
        let url = format!("https://jobicy.com/api/v2/remote-jobs?{query}");
        let body = match get_json(http, &url).await {
            Ok(body) => body,
            // The main feed failing is an error; a search coming back empty-handed is not.
            Err(e) if index == 0 => return Err(e),
            Err(_) => continue,
        };
        for job in body["jobs"].as_array().into_iter().flatten() {
            let id = text(job, "id");
            if !seen.insert(id.clone()) {
                continue;
            }
            let mut c = candidate(board, id, text(job, "jobTitle"), text(job, "companyName"));
            c.title = html_escape::decode_html_entities(&c.title).into_owned();
            c.company = html_escape::decode_html_entities(&c.company).into_owned();
            c.location = text(job, "jobGeo");
            c.url = text(job, "url");
            c.description_html = text(job, "jobDescription");
            c.salary = salary_range(
                job["salaryMin"].as_f64().filter(|v| *v > 0.0),
                job["salaryMax"].as_f64().filter(|v| *v > 0.0),
                &text(job, "salaryCurrency"),
                &text(job, "salaryPeriod"),
            );
            c.posted_at = iso_from_str(&text(job, "pubDate"));
            found.push(c);
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    Ok(found)
}

// ---- Himalayas ------------------------------------------------------------------

async fn himalayas(http: &Client, board: Board) -> anyhow::Result<Vec<Candidate>> {
    // The feed is newest-first, 20 per page. Twelve pages comfortably covers the
    // twelve hours between scans.
    const PAGES: usize = 12;
    let mut found = Vec::new();
    let mut cursor: Option<String> = None;
    for page in 0..PAGES {
        let url = match &cursor {
            Some(c) => format!("https://himalayas.app/jobs/api?limit=20&cursor={}", encode_component(c)),
            None => "https://himalayas.app/jobs/api?limit=20".to_string(),
        };
        let body = match get_json(http, &url).await {
            Ok(body) => body,
            Err(e) if page == 0 => return Err(e),
            Err(_) => break,
        };
        let jobs = body["jobs"].as_array().cloned().unwrap_or_default();
        if jobs.is_empty() {
            break;
        }
        for job in &jobs {
            let link = text(job, "guid");
            let mut c = candidate(board, link.clone(), text(job, "title"), text(job, "companyName"));
            let places: Vec<&str> = job["locationRestrictions"]
                .as_array()
                .map(|all| all.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            c.location = if places.is_empty() {
                "Worldwide".to_string()
            } else {
                places.join(", ")
            };
            c.url = link;
            c.description_html = text(job, "description");
            c.salary = salary_range(
                job["minSalary"].as_f64().filter(|v| *v > 0.0),
                job["maxSalary"].as_f64().filter(|v| *v > 0.0),
                &text(job, "currency"),
                &text(job, "salaryPeriod"),
            );
            c.posted_at = job["pubDate"].as_i64().and_then(iso_from_epoch);
            found.push(c);
        }
        cursor = body["nextCursor"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Ok(found)
}

// ---- We Work Remotely -----------------------------------------------------------

async fn we_work_remotely(http: &Client, board: Board) -> anyhow::Result<Vec<Candidate>> {
    let xml = get_text(http, "https://weworkremotely.com/remote-jobs.rss").await?;
    parse_wwr(&xml, board)
}

fn parse_wwr(xml: &str, board: Board) -> anyhow::Result<Vec<Candidate>> {
    let doc = roxmltree::Document::parse(xml).context("We Work Remotely sent a feed that is not valid XML")?;
    let field = |item: roxmltree::Node, name: &str| -> String {
        item.children()
            .find(|child| child.tag_name().name() == name && child.tag_name().namespace().is_none())
            .and_then(|child| child.text())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    Ok(doc
        .descendants()
        .filter(|node| node.has_tag_name("item"))
        .map(|item| {
            let link = field(item, "link");
            // Titles are written "Company: Job title".
            let heading = field(item, "title");
            let (company, title) = match heading.split_once(": ") {
                Some((company, title)) => (company.trim().to_string(), title.trim().to_string()),
                None => (String::new(), heading.clone()),
            };
            let id = link.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string();
            let mut c = candidate(board, id, title, company);
            c.location = field(item, "region");
            c.url = link;
            c.description_html = field(item, "description");
            c.posted_at = iso_from_str(&field(item, "pubDate"));
            c
        })
        .collect())
}

// ---- Working Nomads -------------------------------------------------------------

async fn working_nomads(http: &Client, board: Board) -> anyhow::Result<Vec<Candidate>> {
    let body = get_json(http, "https://www.workingnomads.com/api/exposed_jobs/").await?;
    let jobs = body.as_array().context("Working Nomads sent no job list")?;
    Ok(jobs
        .iter()
        .map(|job| {
            let link = text(job, "url");
            let id: String = link
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            let mut c = candidate(board, id, text(job, "title"), text(job, "company_name"));
            c.location = text(job, "location");
            c.url = link;
            c.description_html = text(job, "description");
            c.posted_at = iso_from_str(&text(job, "pub_date"));
            c
        })
        .collect())
}

// ---- Hire Autism ----------------------------------------------------------------

static HA_ITEM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<li class="post-(\d+)[^"]*"[^>]*>(.*?)</li>"#).expect("valid regex"));
static HA_COMPANY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<strong>(.*?)</strong>").expect("valid regex"));
static HA_TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<h3>(.*?)</h3>").expect("valid regex"));
static HA_LOCATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)class="location">(.*?)</div>"#).expect("valid regex"));
static HA_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"href="(https://www\.hireautism\.org/job/[^"]+)""#).expect("valid regex"));

struct HireAutismListing {
    id: String,
    company: String,
    title: String,
    location: String,
    link: String,
}

fn parse_hire_autism_listings(html: &str) -> Vec<HireAutismListing> {
    let first = |pattern: &Regex, body: &str| -> String {
        pattern
            .captures(body)
            .map(|c| plain_text(&c[1]))
            .unwrap_or_default()
    };
    HA_ITEM
        .captures_iter(html)
        .map(|item| {
            let body = &item[2];
            HireAutismListing {
                id: item[1].to_string(),
                company: first(&HA_COMPANY, body),
                title: first(&HA_TITLE, body),
                location: first(&HA_LOCATION, body),
                link: HA_LINK
                    .captures(body)
                    .map(|c| html_escape::decode_html_entities(&c[1]).into_owned())
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// Hire Autism is run by the Organization for Autism Research; employers post there
/// specifically to reach autistic candidates. Its search is asked for remote listings
/// unless the user wants other kinds too, and each listing's own remote flag is then
/// checked as well.
async fn hire_autism(http: &Client, board: Board, wanted: Workplaces) -> anyhow::Result<Vec<Candidate>> {
    let remote_only = if wanted.hybrid || wanted.onsite { "" } else { "&remote_position=true" };
    let mut listings = Vec::new();
    for page in 1..=5 {
        let url = format!("https://www.hireautism.org/jm-ajax/get_listings/?per_page=100&page={page}{remote_only}");
        let body = get_json(http, &url).await?;
        listings.extend(parse_hire_autism_listings(body["html"].as_str().unwrap_or_default()));
        if page >= body["max_num_pages"].as_i64().unwrap_or(1) {
            break;
        }
    }
    if listings.is_empty() {
        return Ok(Vec::new());
    }

    // Descriptions and the per-listing remote flag come from the site's content API,
    // a hundred listings at a time. If a call fails the listings are still usable;
    // they just have to show where the work is done through their location text.
    let ids: Vec<&str> = listings.iter().map(|l| l.id.as_str()).collect();
    let mut details: HashMap<String, Value> = HashMap::new();
    for batch in ids.chunks(100) {
        let details_url = format!(
            "https://www.hireautism.org/wp-json/wp/v2/job-listings?per_page=100&include={}",
            batch.join(",")
        );
        if let Ok(Value::Array(items)) = get_json(http, &details_url).await {
            details.extend(items.into_iter().map(|item| (text(&item, "id"), item)));
        }
    }

    Ok(listings
        .into_iter()
        .map(|listing| {
            let mut c = candidate(board, listing.id.clone(), listing.title, listing.company);
            c.location = listing.location;
            c.url = listing.link;
            c.remote = Remote::Unverified;
            c.inclusion = Inclusion::Known {
                kind: "board",
                reason: "Posted on Hire Autism, the Organization for Autism Research's job board for autistic job seekers".to_string(),
                evidence_url: "https://www.hireautism.org/".to_string(),
            };
            if let Some(detail) = details.get(&listing.id) {
                c.description_html = text(&detail["content"], "rendered");
                c.posted_at = iso_from_str(&format!("{}Z", text(detail, "date_gmt")));
                let flag = &detail["meta"]["_remote_position"];
                if flag.as_i64() == Some(1) || flag.as_str() == Some("1") || flag.as_bool() == Some(true) {
                    c.remote = Remote::Declared;
                }
            }
            c
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: Board = Board { id: "weworkremotely", label: "We Work Remotely" };

    #[test]
    fn wwr_feed_is_parsed() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:media="http://search.yahoo.com/mrss">
  <channel>
    <title>We Work Remotely</title>
    <item>
      <media:content url="https://example.com/logo.gif" type="image/png"/>
      <title>Acme Inc: Senior Engineer: Platform</title>
      <region>Anywhere in the World</region>
      <description>&lt;p&gt;We welcome neurodivergent applicants.&lt;/p&gt;</description>
      <pubDate>Mon, 16 Nov 2026 10:51:07 +0000</pubDate>
      <guid>https://weworkremotely.com/remote-jobs/acme-senior-engineer</guid>
      <link>https://weworkremotely.com/remote-jobs/acme-senior-engineer</link>
    </item>
  </channel>
</rss>"#;
        let jobs = parse_wwr(xml, BOARD).unwrap();
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.company, "Acme Inc");
        assert_eq!(job.title, "Senior Engineer: Platform");
        assert_eq!(job.external_id, "acme-senior-engineer");
        assert_eq!(job.location, "Anywhere in the World");
        assert_eq!(job.description_html, "<p>We welcome neurodivergent applicants.</p>");
        assert_eq!(job.posted_at.as_deref(), Some("2026-11-16T10:51:07Z"));
        assert_eq!(job.remote, Remote::Declared);
    }

    #[test]
    fn hire_autism_listing_markup_is_parsed() {
        let html = r#"<li class="post-38882 job_listing type-job_listing job-type-full-time" data-longitude="-73.9">
            <div class="top"><div class="left"><div class="logo"><img src="x.jpg" alt="Company logo"></div></div>
            <div class="right"><strong>Rise &amp; Shine</strong><div class="title"><h3>Market Research Associate</h3></div></div></div>
            <div class="bottom"><div class="left"><div class="job-type full-time">Full Time</div>
            <div class="location"> Brooklyn, NY <small>(Remote)</small> </div></div>
            <div class="right"><div class="link"><a class="hire-button orange" href="https://www.hireautism.org/job/market-research-associate/">VIEW DETAILS / APPLY</a></div></div></div></li>"#;
        let listings = parse_hire_autism_listings(html);
        assert_eq!(listings.len(), 1);
        let l = &listings[0];
        assert_eq!(l.id, "38882");
        assert_eq!(l.company, "Rise & Shine");
        assert_eq!(l.title, "Market Research Associate");
        assert_eq!(l.location, "Brooklyn, NY (Remote)");
        assert_eq!(l.link, "https://www.hireautism.org/job/market-research-associate/");
    }
}
