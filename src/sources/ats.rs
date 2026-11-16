//! Employers' own career sites, read through the public job-board APIs of the hiring
//! systems they run on. Links from here are always the employer's direct listing.

use std::time::Duration;

use anyhow::Context;
use reqwest::Client;
use serde_json::{Value, json};

use super::{get_json, iso_from_epoch, iso_from_str, post_json, text};
use crate::employers::{Employer, Mode};
use crate::model::{Candidate, Inclusion, Remote};

/// Workday lists 20 postings per page. These cap how deep one scan pages: further
/// when the site's own remote filter was used, since every result is then relevant.
const WORKDAY_MAX_SEARCHED: usize = 240;
const WORKDAY_MAX_FILTERED: usize = 400;
const WORKDAY_PAGE: usize = 20;

pub fn source_id(employer: &Employer) -> String {
    let key = if employer.ats == "workday" {
        &employer.tenant
    } else {
        &employer.token
    };
    format!("{}:{}", employer.ats, key)
}

pub async fn fetch(http: &Client, employer: &Employer) -> anyhow::Result<Vec<Candidate>> {
    match employer.ats.as_str() {
        "greenhouse" => greenhouse(http, employer).await,
        "lever" => lever(http, employer).await,
        "ashby" => ashby(http, employer).await,
        "workday" => workday(http, employer).await,
        other => anyhow::bail!("unknown ats \"{other}\" in employers.toml"),
    }
}

fn inclusion_for(employer: &Employer) -> Inclusion {
    match employer.mode {
        Mode::Program => Inclusion::Known {
            kind: employer.inclusion_kind(),
            reason: employer.reason.clone(),
            evidence_url: employer.evidence.clone(),
        },
        Mode::Signal => Inclusion::NeedsSignal,
    }
}

fn candidate(employer: &Employer, id: String, title: String) -> Candidate {
    Candidate {
        source: source_id(employer),
        source_label: format!("{} careers", employer.name),
        external_id: id,
        title,
        company: employer.name.clone(),
        location: String::new(),
        url: String::new(),
        description_html: String::new(),
        salary: String::new(),
        posted_at: None,
        remote: Remote::Unverified,
        inclusion: inclusion_for(employer),
        direct: true,
        detail_url: None,
    }
}

fn keep(candidate: &Candidate) -> bool {
    !candidate.external_id.is_empty() && !candidate.title.is_empty() && candidate.url.starts_with("http")
}

// ---- Greenhouse -----------------------------------------------------------------

async fn greenhouse(http: &Client, employer: &Employer) -> anyhow::Result<Vec<Candidate>> {
    let url = format!(
        "https://boards-api.greenhouse.io/v1/boards/{}/jobs?content=true",
        employer.token
    );
    let body = get_json(http, &url).await?;
    let jobs = body["jobs"].as_array().context("Greenhouse sent no job list")?;
    Ok(jobs
        .iter()
        .map(|job| {
            let mut c = candidate(employer, text(job, "id"), text(job, "title"));
            c.location = text(&job["location"], "name");
            c.url = text(job, "absolute_url");
            // Greenhouse sends the description as entity-escaped HTML.
            c.description_html =
                html_escape::decode_html_entities(job["content"].as_str().unwrap_or_default()).into_owned();
            c.posted_at = iso_from_str(&text(job, "first_published")).or_else(|| iso_from_str(&text(job, "updated_at")));
            // Greenhouse has no remote flag; the location text has to say it.
            c.remote = Remote::Unverified;
            c
        })
        .filter(keep)
        .collect())
}

// ---- Lever ----------------------------------------------------------------------

async fn lever(http: &Client, employer: &Employer) -> anyhow::Result<Vec<Candidate>> {
    let url = format!("https://api.lever.co/v0/postings/{}?mode=json", employer.token);
    let body = get_json(http, &url).await?;
    let jobs = body.as_array().context("Lever sent no job list")?;
    Ok(jobs
        .iter()
        // Lever states the workplace type on every posting; anything but "remote" is skipped.
        .filter(|job| job["workplaceType"].as_str() == Some("remote"))
        .map(|job| {
            let mut c = candidate(employer, text(job, "id"), text(job, "text"));
            let categories = &job["categories"];
            let mut places: Vec<String> = categories["allLocations"]
                .as_array()
                .map(|all| all.iter().filter_map(|v| v.as_str()).map(str::to_string).collect())
                .unwrap_or_default();
            if places.is_empty() {
                places.push(text(categories, "location"));
            }
            c.location = places.join("; ");
            c.url = text(job, "hostedUrl");
            let mut html = text(job, "description");
            for list in job["lists"].as_array().into_iter().flatten() {
                html.push_str(&format!("<h3>{}</h3><ul>{}</ul>", text(list, "text"), text(list, "content")));
            }
            html.push_str(&text(job, "additional"));
            c.description_html = html;
            c.posted_at = job["createdAt"].as_i64().and_then(|ms| iso_from_epoch(ms / 1000));
            let range = &job["salaryRange"];
            c.salary = super::salary_range(
                range["min"].as_f64(),
                range["max"].as_f64(),
                &text(range, "currency"),
                &text(range, "interval"),
            );
            c.remote = Remote::Declared;
            c
        })
        .filter(keep)
        .collect())
}

// ---- Ashby ----------------------------------------------------------------------

async fn ashby(http: &Client, employer: &Employer) -> anyhow::Result<Vec<Candidate>> {
    let url = format!(
        "https://api.ashbyhq.com/posting-api/job-board/{}?includeCompensation=true",
        employer.token
    );
    let body = get_json(http, &url).await?;
    let jobs = body["jobs"].as_array().context("Ashby sent no job list")?;
    Ok(jobs
        .iter()
        .filter(|job| job["isListed"].as_bool() != Some(false))
        // Ashby states the workplace type; "Hybrid" and "OnSite" are skipped outright.
        .filter(|job| match job["workplaceType"].as_str() {
            Some(kind) => kind.eq_ignore_ascii_case("remote"),
            None => job["isRemote"].as_bool() == Some(true),
        })
        .map(|job| {
            let mut c = candidate(employer, text(job, "id"), text(job, "title"));
            let mut places = vec![text(job, "location")];
            for extra in job["secondaryLocations"].as_array().into_iter().flatten() {
                places.push(text(extra, "location"));
            }
            places.retain(|p| !p.is_empty());
            c.location = places.join("; ");
            c.url = text(job, "jobUrl");
            c.description_html = text(job, "descriptionHtml");
            c.posted_at = iso_from_str(&text(job, "publishedAt"));
            c.salary = text(&job["compensation"], "compensationTierSummary");
            c.remote = Remote::Declared;
            c
        })
        .filter(keep)
        .collect())
}

// ---- Workday --------------------------------------------------------------------

/// Workday career sites can hold tens of thousands of postings. Where a site has a
/// workplace filter, the app asks it for exactly the postings the employer marks as
/// fully remote. Otherwise it asks the site's search for "remote" and holds each hit
/// to the same strict remote rules as everything else. Descriptions are not in the
/// results; `detail_url` lets the scan fetch them for postings that get that far.
async fn workday(http: &Client, employer: &Employer) -> anyhow::Result<Vec<Candidate>> {
    let api = format!("https://{}/wday/cxs/{}/{}", employer.host, employer.tenant, employer.site);
    let public = if employer.public_base.is_empty() {
        format!("https://{}/{}", employer.host, employer.site)
    } else {
        employer.public_base.trim_end_matches('/').to_string()
    };
    let jobs_url = format!("{api}/jobs");

    // The first, unfiltered page says which filters the site offers.
    let overview = post_json(http, &jobs_url, &json!({ "appliedFacets": {}, "limit": 1, "offset": 0, "searchText": "" })).await?;
    let (facets, search, by_filter, limit) = match remote_filter(&overview) {
        Some((parameter, values)) => (json!({ parameter: values }), "", true, WORKDAY_MAX_FILTERED),
        None => (json!({}), "remote", false, WORKDAY_MAX_SEARCHED),
    };

    let mut found = Vec::new();
    let mut total = usize::MAX;
    let mut offset = 0;
    while offset < total.min(limit) {
        let query = json!({ "appliedFacets": facets, "limit": WORKDAY_PAGE, "offset": offset, "searchText": search });
        let page = post_json(http, &jobs_url, &query).await?;
        // Only the first page reports the real total.
        if offset == 0 {
            total = page["total"].as_u64().unwrap_or(0) as usize;
        }
        let postings = page["jobPostings"].as_array().cloned().unwrap_or_default();
        if postings.is_empty() {
            break;
        }
        for posting in &postings {
            if let Some(c) = workday_posting(employer, &api, &public, posting, by_filter) {
                found.push(c);
            }
        }
        offset += WORKDAY_PAGE;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Ok(found)
}

/// Reads one of Workday's workplace labels. Every employer words these its own way
/// ("Remote", "100% Remote", "Fully Virtual Job", "Hybrid or 100% Remote",
/// "Office Worker (NOT Remote)", "Flexible Work Arrangement"), so a label only counts
/// as remote when it says so and names nothing else.
fn classify_workplace(label: &str) -> Remote {
    const NOT_REMOTE: [&str; 11] = [
        "hybrid", "not remote", "office", "onsite", "on-site", "on site", "flex", "campus", "field", "in person",
        "in-person",
    ];
    const REMOTE: [&str; 6] = ["remote", "virtual", "work from home", "telecommut", "home based", "home-based"];
    let label = label.trim().to_lowercase();
    if label.is_empty() || label.contains("not designated") || label.contains("unspecified") || label == "n/a" {
        return Remote::Unverified;
    }
    if NOT_REMOTE.iter().any(|word| label.contains(word)) {
        return Remote::No;
    }
    if REMOTE.iter().any(|word| label.contains(word)) {
        return Remote::Declared;
    }
    Remote::No
}

fn workday_label(value: &Value) -> Remote {
    classify_workplace(value.as_str().unwrap_or_default())
}

/// Finds the site's workplace filter, if it has one, and the values in it that mean
/// fully remote. Returns the filter's parameter name and those value ids.
fn remote_filter(overview: &Value) -> Option<(String, Vec<String>)> {
    for facet in overview["facets"].as_array()? {
        let parameter = text(facet, "facetParameter");
        let name = format!("{parameter} {}", text(facet, "descriptor")).to_lowercase().replace('_', " ");
        let about_workplace = ["remote", "workplace", "worksite", "work type", "work arrangement", "location type"]
            .iter()
            .any(|word| name.contains(word));
        if parameter.is_empty() || !about_workplace {
            continue;
        }
        let remote_values: Vec<String> = facet["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|value| classify_workplace(&text(value, "descriptor")) == Remote::Declared)
            .map(|value| text(value, "id"))
            .filter(|id| !id.is_empty())
            .collect();
        if !remote_values.is_empty() {
            return Some((parameter, remote_values));
        }
    }
    None
}

/// `by_filter` is true when the posting came back from the site's own fully-remote
/// filter, which then stands in for a label the result row may not repeat.
fn workday_posting(employer: &Employer, api: &str, public: &str, posting: &Value, by_filter: bool) -> Option<Candidate> {
    let path = text(posting, "externalPath");
    if path.is_empty() {
        return None;
    }
    let remote = match workday_label(&posting["remoteType"]) {
        Remote::No => return None,
        Remote::Unverified if by_filter => Remote::Declared,
        other => other,
    };

    let id = posting["bulletFields"]
        .as_array()
        .and_then(|fields| fields.first())
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| path.clone());
    let mut c = candidate(employer, id, text(posting, "title"));

    // The path holds the primary location even when the summary says "6 Locations".
    let place = path
        .strip_prefix("/job/")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let summary = text(posting, "locationsText");
    let extra_locations = summary
        .strip_suffix(" Locations")
        .and_then(|n| n.trim().parse::<usize>().ok());
    c.location = match extra_locations {
        Some(n) if !place.is_empty() => format!("{place} (+{} more)", n.saturating_sub(1)),
        _ if summary.is_empty() => place,
        _ if place.to_lowercase().contains("remote") && !summary.to_lowercase().contains("remote") => {
            format!("{summary} ({place})")
        }
        _ => summary,
    };
    c.url = format!("{public}{path}");
    c.detail_url = Some(format!("{api}{path}"));
    c.remote = remote;
    Some(c)
}

/// Fills in the description and the authoritative remote label for one posting.
pub async fn workday_detail(http: &Client, url: &str, candidate: &mut Candidate) -> anyhow::Result<()> {
    let body = get_json(http, url).await?;
    let info = &body["jobPostingInfo"];
    anyhow::ensure!(info.is_object(), "Workday sent no posting details");

    candidate.description_html = text(info, "jobDescription");
    if let Some(posted) = iso_from_str(&text(info, "startDate")) {
        candidate.posted_at = Some(posted);
    }
    let external = text(info, "externalUrl");
    if external.starts_with("http") {
        candidate.url = external;
    }

    let mut places = vec![text(info, "location")];
    for extra in info["additionalLocations"].as_array().into_iter().flatten() {
        if let Some(place) = extra.as_str() {
            places.push(place.to_string());
        }
    }
    places.retain(|p| !p.is_empty());
    let requisition_place = text(&info["jobRequisitionLocation"], "descriptor");
    if !requisition_place.is_empty() && !places.contains(&requisition_place) {
        places.insert(0, requisition_place);
    }
    if !places.is_empty() {
        let shown = places.iter().take(4).cloned().collect::<Vec<_>>().join("; ");
        let hidden = places.len().saturating_sub(4);
        let listed = if hidden > 0 { format!("{shown} (+{hidden} more)") } else { shown };
        // Keep a remote label that only the search result carried.
        let had_remote = candidate.location.to_lowercase().contains("remote");
        candidate.location = if had_remote && !listed.to_lowercase().contains("remote") {
            format!("{listed} (Remote)")
        } else {
            listed
        };
    }
    // Workday locations are often just "MA - Springfield"; name the country too, so
    // the region filter has something to match.
    let country = text(&info["country"], "descriptor");
    if !country.is_empty() && !candidate.location.contains(&country) {
        candidate.location = format!("{} · {country}", candidate.location);
    }

    // The detail page carries the employer's own workplace label; show it, and let it
    // decide.
    let label = text(info, "remoteType");
    if !label.is_empty() {
        candidate.remote = workday_label(&info["remoteType"]);
        if !candidate.location.to_lowercase().contains(&label.to_lowercase()) {
            candidate.location = format!("{} ({label})", candidate.location);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::check_remote_labels;

    fn employer() -> Employer {
        Employer {
            name: "Example".into(),
            aliases: vec![],
            mode: Mode::Program,
            kind: String::new(),
            reason: "Runs a program".into(),
            evidence: "https://example.com".into(),
            ats: "workday".into(),
            token: String::new(),
            host: "example.wd5.myworkdayjobs.com".into(),
            tenant: "example".into(),
            site: "External".into(),
            public_base: String::new(),
        }
    }

    fn posting(title: &str, path: &str, locations: &str, remote_type: Option<&str>) -> Value {
        json!({ "title": title, "externalPath": path, "locationsText": locations,
                "remoteType": remote_type, "bulletFields": ["R-1"] })
    }

    fn parse(p: &Value) -> Option<Candidate> {
        workday_posting(&employer(), "https://example.wd5.myworkdayjobs.com/wday/cxs/example/External", "https://example.wd5.myworkdayjobs.com/External", p, false)
    }

    #[test]
    fn workday_remote_label_is_authoritative() {
        let remote = parse(&posting("Counsel", "/job/MA---Springfield/Counsel_R-1", "6 Locations", Some("Remote"))).unwrap();
        assert_eq!(remote.remote, Remote::Declared);
        assert_eq!(remote.location, "MA Springfield (+5 more)");
        assert_eq!(remote.url, "https://example.wd5.myworkdayjobs.com/External/job/MA---Springfield/Counsel_R-1");
        assert_eq!(remote.key(), "workday:example:R-1");

        // Explicit hybrid / onsite labels are dropped before any other check.
        assert!(parse(&posting("Remote Operations Specialist", "/job/GA-Atlanta/x_R-1", "GA Atlanta", Some("Hybrid"))).is_none());
        assert!(parse(&posting("Engineer", "/job/GA-Atlanta/x_R-1", "GA Atlanta", Some("Onsite"))).is_none());
    }

    #[test]
    fn workday_labels_in_an_employers_own_words_are_understood() {
        assert_eq!(workday_label(&json!("Remote")), Remote::Declared);
        assert_eq!(workday_label(&json!("Remote Home-Based")), Remote::Declared);
        assert_eq!(workday_label(&json!("Remote Customer-Based")), Remote::Declared);
        assert_eq!(workday_label(&json!("Hybrid")), Remote::No);
        assert_eq!(workday_label(&json!("Hybrid Remote")), Remote::No);
        assert_eq!(workday_label(&json!("Onsite")), Remote::No);
        assert_eq!(workday_label(&json!("Office - Flexible")), Remote::No);
        assert_eq!(workday_label(&json!("Office Tech-Flex")), Remote::No);
        assert_eq!(workday_label(&json!(null)), Remote::Unverified);
        assert_eq!(workday_label(&json!("")), Remote::Unverified);
        // Wordings seen on real career sites.
        for label in ["100% Remote", "Fully Remote", "Virtual Worker", "Fully Virtual Job", "Remote Eligible, USA"] {
            assert_eq!(classify_workplace(label), Remote::Declared, "{label}");
        }
        for label in [
            "Office Worker (NOT Remote)",
            "Hybrid or 100% Remote",
            "Remote/Hybrid",
            "Remote or Hybrid",
            "Flexible Work Arrangement",
            "Onsite Position",
            "Office Focused",
            "Hybrid Flex",
            "Field Based",
            "On Campus",
            "Off Campus",
        ] {
            assert_eq!(classify_workplace(label), Remote::No, "{label}");
        }
        assert_eq!(classify_workplace("Not Designated - CW"), Remote::Unverified);
    }

    #[test]
    fn the_sites_own_remote_filter_is_found() {
        let overview = json!({ "facets": [
            { "facetParameter": "jobFamilyGroup", "descriptor": "Job Category", "values": [{ "descriptor": "Remote Sensing", "id": "x" }] },
            { "facetParameter": "remoteType", "descriptor": "Remote Type", "values": [
                { "descriptor": "Hybrid", "id": "h1" },
                { "descriptor": "Hybrid or 100% Remote", "id": "h2" },
                { "descriptor": "100% Remote", "id": "r1" },
            ] },
        ] });
        assert_eq!(remote_filter(&overview), Some(("remoteType".to_string(), vec!["r1".to_string()])));

        let named_differently = json!({ "facets": [
            { "facetParameter": "Workplace_Arrangement", "descriptor": "Workplace Arrangement", "values": [
                { "descriptor": "Remote", "id": "r" }, { "descriptor": "Hybrid", "id": "h" }, { "descriptor": "Office", "id": "o" },
            ] },
        ] });
        assert_eq!(remote_filter(&named_differently), Some(("Workplace_Arrangement".to_string(), vec!["r".to_string()])));

        // A filter with no fully-remote value is no use.
        let none_remote = json!({ "facets": [
            { "facetParameter": "remoteType", "descriptor": "Remote Type", "values": [
                { "descriptor": "Office Worker (NOT Remote)", "id": "o" }, { "descriptor": "Remote or Hybrid", "id": "rh" },
            ] },
        ] });
        assert_eq!(remote_filter(&none_remote), None);
        assert_eq!(remote_filter(&json!({})), None);
    }

    #[test]
    fn a_posting_from_the_remote_filter_counts_as_declared() {
        let unlabeled = posting("Systems Engineer", "/job/TX-Dallas/Systems-Engineer_R-1", "TX Dallas", None);
        let listed = workday_posting(&employer(), "https://x/wday/cxs/x/External", "https://x/External", &unlabeled, true).unwrap();
        assert_eq!(listed.remote, Remote::Declared);
        // A row that carries its own non-remote label is still dropped.
        let hybrid = posting("Systems Engineer", "/job/TX-Dallas/Systems-Engineer_R-1", "TX Dallas", Some("Hybrid"));
        assert!(workday_posting(&employer(), "https://x/wday/cxs/x/External", "https://x/External", &hybrid, true).is_none());
    }

    #[test]
    fn workday_without_a_label_falls_back_to_text_rules() {
        // Remote named in the path only.
        let c = parse(&posting("Security Director", "/job/California---Remote/Security-Director_JR1", "14 Locations", None)).unwrap();
        assert_eq!(c.remote, Remote::Unverified);
        assert!(check_remote_labels(&c.title, &c.location, c.remote).is_ok(), "{}", c.location);

        // A search hit that merely mentions "remote" somewhere is not enough.
        let c = parse(&posting("ASIC Engineer", "/job/USA-CA-San-Jose/ASIC-Engineer_R2", "USA-CA San Jose", None)).unwrap();
        assert!(check_remote_labels(&c.title, &c.location, c.remote).is_err());

        // Mixed labels in the title are rejected.
        let c = parse(&posting("Network Engineer, Hybrid Remote & On-Site OK", "/job/All-Oklahoma/x_3", "6 Locations", None)).unwrap();
        assert!(check_remote_labels(&c.title, &c.location, c.remote).is_err());
    }
}
