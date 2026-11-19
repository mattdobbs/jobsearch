//! Employers' own career sites, read through the public job-board APIs of the hiring
//! systems they run on. Links from here are always the employer's direct listing.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::Context;
use reqwest::Client;
use serde_json::{Value, json};

use super::{get_json, iso_from_epoch, iso_from_str, post_json, text};
use crate::employers::{Employer, Mode};
use crate::model::{Candidate, Inclusion, Remote, Workplaces};

/// Workday lists 20 postings per page. These cap how deep one scan pages: further
/// when the site's own workplace filter was used, since every result is then
/// relevant, and when reading the newest postings of every kind.
const WORKDAY_MAX_SEARCHED: usize = 240;
const WORKDAY_MAX_FILTERED: usize = 400;
const WORKDAY_MAX_NEWEST: usize = 400;
const WORKDAY_PAGE: usize = 20;

pub fn source_id(employer: &Employer) -> String {
    let key = if employer.ats == "workday" {
        &employer.tenant
    } else {
        &employer.token
    };
    format!("{}:{}", employer.ats, key)
}

/// Reads one employer's postings. Greenhouse, Lever and Ashby send their whole list
/// in one answer; a Workday site is asked only for the kinds of job in `wanted`.
pub async fn fetch(http: &Client, employer: &Employer, wanted: Workplaces) -> anyhow::Result<Vec<Candidate>> {
    match employer.ats.as_str() {
        "greenhouse" => greenhouse(http, employer).await,
        "lever" => lever(http, employer).await,
        "ashby" => ashby(http, employer).await,
        "workday" => workday(http, employer, wanted).await,
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
            // Lever states the workplace type on a posting, unless it is "unspecified".
            c.remote = match job["workplaceType"].as_str().unwrap_or_default().to_lowercase().as_str() {
                "remote" => Remote::Declared,
                "hybrid" => Remote::Hybrid,
                "onsite" | "on-site" => Remote::OnSite,
                _ => Remote::Unverified,
            };
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
            c.remote = ashby_workplace(job["workplaceType"].as_str(), job["isRemote"].as_bool());
            c
        })
        .filter(keep)
        .collect())
}

/// Ashby states the workplace type ("Remote", "Hybrid", "OnSite"). Older postings
/// carry only a remote flag.
fn ashby_workplace(kind: Option<&str>, is_remote: Option<bool>) -> Remote {
    match (kind.unwrap_or_default().to_lowercase().as_str(), is_remote) {
        ("remote", _) => Remote::Declared,
        ("hybrid", _) => Remote::Hybrid,
        ("onsite" | "on-site", _) => Remote::OnSite,
        (_, Some(true)) => Remote::Declared,
        (_, Some(false)) => Remote::OnSite,
        (_, None) => Remote::Unverified,
    }
}

// ---- Workday --------------------------------------------------------------------

/// Workday career sites can hold tens of thousands of postings, so a site is asked
/// only for the kinds of job the user wants, one kind at a time:
///
/// - where the site has a workplace filter with values for that kind, exactly the
///   postings the employer marks that way;
/// - otherwise, for remote and hybrid, the site's search for that word, with each
///   hit held to the same strict rules as everything else;
/// - otherwise, for on-site, the newest postings of every kind, since a posting
///   that is on-site rarely says so.
///
/// Descriptions are not in the results; `detail_url` lets the scan fetch them for
/// postings that get that far.
async fn workday(http: &Client, employer: &Employer, wanted: Workplaces) -> anyhow::Result<Vec<Candidate>> {
    let api = format!("https://{}/wday/cxs/{}/{}", employer.host, employer.tenant, employer.site);
    let public = if employer.public_base.is_empty() {
        format!("https://{}/{}", employer.host, employer.site)
    } else {
        employer.public_base.trim_end_matches('/').to_string()
    };
    let jobs_url = format!("{api}/jobs");

    // The first, unfiltered page says which filters the site offers.
    let overview = post_json(http, &jobs_url, &json!({ "appliedFacets": {}, "limit": 1, "offset": 0, "searchText": "" })).await?;
    let filter = workplace_filter(&overview);

    let mut found: Vec<Candidate> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (kind, word, is_wanted) in [
        (Remote::Declared, "remote", wanted.remote),
        (Remote::Hybrid, "hybrid", wanted.hybrid),
        (Remote::OnSite, "", wanted.onsite),
    ] {
        if !is_wanted {
            continue;
        }
        let values: Vec<&str> = filter
            .iter()
            .flat_map(|(_, values)| values)
            .filter(|(_, value_kind)| *value_kind == kind)
            .map(|(id, _)| id.as_str())
            .collect();
        let (facets, search, assumed, limit) = match &filter {
            Some((parameter, _)) if !values.is_empty() => {
                (json!({ (parameter.as_str()): values }), "", Some(kind), WORKDAY_MAX_FILTERED)
            }
            _ if word.is_empty() => (json!({}), "", None, WORKDAY_MAX_NEWEST),
            _ => (json!({}), word, None, WORKDAY_MAX_SEARCHED),
        };

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
                if let Some(c) = workday_posting(employer, &api, &public, posting, assumed) {
                    // The passes overlap; the first one to find a posting keeps it.
                    if seen.insert(c.external_id.clone()) {
                        found.push(c);
                    }
                }
            }
            offset += WORKDAY_PAGE;
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
    Ok(found)
}

/// Reads one of Workday's workplace labels. Every employer words these its own way
/// ("Remote", "100% Remote", "Fully Virtual Job", "Hybrid or 100% Remote",
/// "Office Worker (NOT Remote)", "Flexible Work Arrangement"), so a label only counts
/// as remote when it says so and names nothing else. One that offers hybrid beside
/// remote, or says flexible, is read as hybrid.
fn classify_workplace(label: &str) -> Remote {
    const HYBRID: [&str; 2] = ["hybrid", "flex"];
    const ON_SITE: [&str; 8] = ["office", "onsite", "on-site", "on site", "campus", "field", "in person", "in-person"];
    const REMOTE: [&str; 6] = ["remote", "virtual", "work from home", "telecommut", "home based", "home-based"];
    let label = label.trim().to_lowercase();
    if label.is_empty() || label.contains("not designated") || label.contains("unspecified") || label == "n/a" {
        return Remote::Unverified;
    }
    if HYBRID.iter().any(|word| label.contains(word)) {
        return Remote::Hybrid;
    }
    // "Non-Remote Posting", "Remote: No", "Not Eligible for Remote": the word remote
    // is there, and the label means the opposite.
    let negated = label
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| matches!(word, "no" | "non" | "not" | "ineligible" | "nonremote"));
    if negated || ON_SITE.iter().any(|word| label.contains(word)) {
        return Remote::OnSite;
    }
    if REMOTE.iter().any(|word| label.contains(word)) {
        return Remote::Declared;
    }
    // A label in words of the employer's own that does not say remote.
    Remote::OnSite
}

fn workday_label(value: &Value) -> Remote {
    classify_workplace(value.as_str().unwrap_or_default())
}

/// Finds the site's workplace filter, if it has one. Returns the filter's parameter
/// name and, for each of its values that says remote, hybrid or on-site, the value's
/// id and which of those it says.
fn workplace_filter(overview: &Value) -> Option<(String, Vec<(String, Remote)>)> {
    for facet in overview["facets"].as_array()? {
        let parameter = text(facet, "facetParameter");
        let name = format!("{parameter} {}", text(facet, "descriptor")).to_lowercase().replace('_', " ");
        let about_workplace = ["remote", "workplace", "worksite", "work type", "work arrangement", "location type"]
            .iter()
            .any(|word| name.contains(word));
        if parameter.is_empty() || !about_workplace {
            continue;
        }
        let values: Vec<(String, Remote)> = facet["values"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|value| (text(value, "id"), classify_workplace(&text(value, "descriptor"))))
            .filter(|(id, kind)| !id.is_empty() && *kind != Remote::Unverified)
            .collect();
        if !values.is_empty() {
            return Some((parameter, values));
        }
    }
    None
}

/// `assumed` is set when the posting came back from the site's own workplace filter:
/// what that filter was asked for then stands in for a label the result row may not
/// repeat.
fn workday_posting(employer: &Employer, api: &str, public: &str, posting: &Value, assumed: Option<Remote>) -> Option<Candidate> {
    let path = text(posting, "externalPath");
    if path.is_empty() {
        return None;
    }
    let remote = match workday_label(&posting["remoteType"]) {
        Remote::Unverified => assumed.unwrap_or(Remote::Unverified),
        labelled => labelled,
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
    use crate::filter::workplace_by_listing;
    use crate::model::Workplace;

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
        workday_posting(&employer(), "https://example.wd5.myworkdayjobs.com/wday/cxs/example/External", "https://example.wd5.myworkdayjobs.com/External", p, None)
    }

    fn listed(c: &Candidate) -> Workplace {
        workplace_by_listing(&c.title, &c.location, c.remote).0
    }

    #[test]
    fn workday_remote_label_is_authoritative() {
        let remote = parse(&posting("Counsel", "/job/MA---Springfield/Counsel_R-1", "6 Locations", Some("Remote"))).unwrap();
        assert_eq!(remote.remote, Remote::Declared);
        assert_eq!(remote.location, "MA Springfield (+5 more)");
        assert_eq!(remote.url, "https://example.wd5.myworkdayjobs.com/External/job/MA---Springfield/Counsel_R-1");
        assert_eq!(remote.key(), "workday:example:R-1");

        // An explicit hybrid or on-site label settles it, whatever the title says.
        let hybrid = parse(&posting("Engineer - Remote", "/job/GA-Atlanta/x_R-1", "GA Atlanta", Some("Hybrid"))).unwrap();
        assert_eq!((hybrid.remote, listed(&hybrid)), (Remote::Hybrid, Workplace::Hybrid));
        let onsite = parse(&posting("Engineer - Remote", "/job/GA-Atlanta/x_R-1", "GA Atlanta", Some("Onsite"))).unwrap();
        assert_eq!((onsite.remote, listed(&onsite)), (Remote::OnSite, Workplace::OnSite));
    }

    #[test]
    fn workday_labels_in_an_employers_own_words_are_understood() {
        assert_eq!(workday_label(&json!("Remote")), Remote::Declared);
        assert_eq!(workday_label(&json!("Remote Home-Based")), Remote::Declared);
        assert_eq!(workday_label(&json!("Remote Customer-Based")), Remote::Declared);
        assert_eq!(workday_label(&json!("Hybrid")), Remote::Hybrid);
        assert_eq!(workday_label(&json!("Hybrid Remote")), Remote::Hybrid);
        assert_eq!(workday_label(&json!("Onsite")), Remote::OnSite);
        assert_eq!(workday_label(&json!("Office - Flexible")), Remote::Hybrid);
        assert_eq!(workday_label(&json!("Office Tech-Flex")), Remote::Hybrid);
        assert_eq!(workday_label(&json!(null)), Remote::Unverified);
        assert_eq!(workday_label(&json!("")), Remote::Unverified);
        // Wordings seen on real career sites.
        for label in ["100% Remote", "Fully Remote", "Virtual Worker", "Fully Virtual Job", "Remote Eligible, USA"] {
            assert_eq!(classify_workplace(label), Remote::Declared, "{label}");
        }
        // Offered beside remote, hybrid still means the job is not fully remote.
        for label in ["Hybrid or 100% Remote", "Remote/Hybrid", "Remote or Hybrid", "Flexible Work Arrangement", "Hybrid Flex", "Hybrid Job"] {
            assert_eq!(classify_workplace(label), Remote::Hybrid, "{label}");
        }
        for label in [
            "Office Worker (NOT Remote)",
            // The word "remote", meaning the opposite.
            "Non-Remote Posting",
            "Non Remote",
            "Not Remote",
            "Remote: No",
            "Not Eligible for Remote Work",
            "Onsite Position",
            "Onsite Job",
            "Office Focused",
            "Field Based",
            "On Campus",
            "Off Campus",
            // Not a word for remote, so not remote.
            "Client Location",
        ] {
            assert_eq!(classify_workplace(label), Remote::OnSite, "{label}");
        }
        assert_eq!(classify_workplace("Not Designated - CW"), Remote::Unverified);
    }

    #[test]
    fn the_sites_own_workplace_filter_is_found() {
        let value = |id: &str, kind: Remote| (id.to_string(), kind);
        let overview = json!({ "facets": [
            { "facetParameter": "jobFamilyGroup", "descriptor": "Job Category", "values": [{ "descriptor": "Remote Sensing", "id": "x" }] },
            { "facetParameter": "remoteType", "descriptor": "Remote Type", "values": [
                { "descriptor": "Hybrid", "id": "h1" },
                { "descriptor": "Hybrid or 100% Remote", "id": "h2" },
                { "descriptor": "100% Remote", "id": "r1" },
                { "descriptor": "Not Designated", "id": "n1" },
            ] },
        ] });
        assert_eq!(
            workplace_filter(&overview),
            Some(("remoteType".to_string(), vec![value("h1", Remote::Hybrid), value("h2", Remote::Hybrid), value("r1", Remote::Declared)]))
        );

        let named_differently = json!({ "facets": [
            { "facetParameter": "Workplace_Arrangement", "descriptor": "Workplace Arrangement", "values": [
                { "descriptor": "Remote", "id": "r" }, { "descriptor": "Hybrid", "id": "h" }, { "descriptor": "Office", "id": "o" },
            ] },
        ] });
        assert_eq!(
            workplace_filter(&named_differently),
            Some(("Workplace_Arrangement".to_string(), vec![value("r", Remote::Declared), value("h", Remote::Hybrid), value("o", Remote::OnSite)]))
        );

        // A filter can lack a fully-remote value; the site's search is used for that kind.
        let none_remote = json!({ "facets": [
            { "facetParameter": "remoteType", "descriptor": "Remote Type", "values": [
                { "descriptor": "Office Worker (NOT Remote)", "id": "o" }, { "descriptor": "Remote or Hybrid", "id": "rh" },
            ] },
        ] });
        let (_, values) = workplace_filter(&none_remote).unwrap();
        assert!(values.iter().all(|(_, kind)| *kind != Remote::Declared));
        assert_eq!(workplace_filter(&json!({})), None);
    }

    #[test]
    fn a_posting_from_the_workplace_filter_takes_the_kind_that_was_asked_for() {
        let unlabeled = posting("Systems Engineer", "/job/TX-Dallas/Systems-Engineer_R-1", "TX Dallas", None);
        let from = |assumed: Remote| workday_posting(&employer(), "https://x/wday/cxs/x/External", "https://x/External", &unlabeled, Some(assumed)).unwrap();
        assert_eq!(from(Remote::Declared).remote, Remote::Declared);
        assert_eq!(from(Remote::Hybrid).remote, Remote::Hybrid);
        assert_eq!(listed(&from(Remote::OnSite)), Workplace::OnSite);
        // A row that carries its own label keeps it.
        let hybrid = posting("Systems Engineer", "/job/TX-Dallas/Systems-Engineer_R-1", "TX Dallas", Some("Hybrid"));
        let row = workday_posting(&employer(), "https://x/wday/cxs/x/External", "https://x/External", &hybrid, Some(Remote::Declared)).unwrap();
        assert_eq!(row.remote, Remote::Hybrid);
    }

    #[test]
    fn ashby_and_lever_workplace_types_are_read() {
        assert_eq!(ashby_workplace(Some("Remote"), Some(true)), Remote::Declared);
        assert_eq!(ashby_workplace(Some("Hybrid"), Some(false)), Remote::Hybrid);
        assert_eq!(ashby_workplace(Some("OnSite"), None), Remote::OnSite);
        // Older postings carry only the flag.
        assert_eq!(ashby_workplace(None, Some(true)), Remote::Declared);
        assert_eq!(ashby_workplace(None, Some(false)), Remote::OnSite);
        assert_eq!(ashby_workplace(None, None), Remote::Unverified);
    }

    #[test]
    fn workday_without_a_label_falls_back_to_text_rules() {
        // Remote named in the path only.
        let c = parse(&posting("Security Director", "/job/California---Remote/Security-Director_JR1", "14 Locations", None)).unwrap();
        assert_eq!(c.remote, Remote::Unverified);
        assert_eq!(listed(&c), Workplace::Remote, "{}", c.location);

        // A search hit that merely mentions "remote" somewhere is not enough.
        let c = parse(&posting("ASIC Engineer", "/job/USA-CA-San-Jose/ASIC-Engineer_R2", "USA-CA San Jose", None)).unwrap();
        assert_eq!(listed(&c), Workplace::Unstated);

        // Mixed labels in the title are not remote.
        let c = parse(&posting("Network Engineer, Hybrid Remote & On-Site OK", "/job/All-Oklahoma/x_3", "6 Locations", None)).unwrap();
        assert_eq!(listed(&c), Workplace::Hybrid);
    }
}
