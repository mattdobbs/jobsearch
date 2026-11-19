//! One scan: fetch every source, then decide which postings become cards.
//!
//! A posting becomes a card only if it clears every check, in this order:
//!
//! 1. not already on the board (in any column, including Pass);
//! 2. its listing shows it is a kind of job the user wants, or could still turn out
//!    to be one: remote only, unless they also chose hybrid or on-site in Settings;
//! 3. it matches the user's role and region settings;
//! 4. it is not another posting of a job the user has already dealt with: one whose
//!    card is in any column other than New and Evaluating;
//! 5. it is not a job board's copy of a posting already on the board;
//! 6. read together with its description it is still a kind of job the user wants.
//!    A description can show that a job listed as remote is hybrid or on-site;
//! 7. if it is hybrid or on-site, it is in one of the places the user can travel to;
//! 8. neither its title nor its description mentions a phrase the user has hidden;
//! 9. the employer runs a hiring program, or the posting was placed on a board for
//!    autistic job seekers, or the posting's own text welcomes neurodivergent or
//!    disabled applicants. The user can turn this last check off in Settings.

use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;
use std::time::Duration;

use tokio::task::JoinSet;

use crate::db::NewJob;
use crate::employers::{self, Employer, Mode};
use crate::filter;
use crate::model::{Candidate, Inclusion, NO_INCLUSION_CLAIM, Settings, SourceStats, Workplace};
use crate::sources;
use crate::state::AppState;
use crate::{log_info, log_warn};

/// How many full descriptions one source may have downloaded per scan. Postings over
/// the limit are picked up by the next scan.
const DETAIL_FETCHES_PER_SOURCE: usize = 60;

/// The pause between two description downloads from one site.
const DETAIL_PAUSE: Duration = Duration::from_millis(500);

/// How long to hold off when a site says it is getting too many requests: the first
/// time, then the second. After that the rest of its postings wait for the next scan.
const RATE_LIMIT_WAITS: [Duration; 2] = [Duration::from_secs(10), Duration::from_secs(30)];

/// Stored descriptions are capped; a posting longer than this is cut off.
const MAX_DESCRIPTION_BYTES: usize = 200_000;

/// Why a posting with no inclusion language at all is remembered as rejected. Those
/// rejections stop applying when the user turns the inclusive-employers rule off.
pub const NO_INCLUSION_SIGNAL: &str = "posting does not mention neurodiversity or disability inclusion";

static CLEANER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let mut builder = ammonia::Builder::default();
    // Images in job descriptions are mostly tracking pixels; none are loaded.
    builder.rm_tags(["img"]);
    builder.link_rel(Some("noopener noreferrer"));
    builder.set_tag_attribute_value("a", "target", "_blank");
    builder
});

/// Makes a description from the internet safe to show in the board.
fn sanitize(html: &str) -> String {
    let mut clean = CLEANER.clean(html).to_string();
    if clean.len() > MAX_DESCRIPTION_BYTES {
        let mut cut = MAX_DESCRIPTION_BYTES;
        while !clean.is_char_boundary(cut) {
            cut -= 1;
        }
        clean.truncate(cut);
        // Re-clean so the cut cannot leave a half-open tag behind.
        clean = CLEANER.clean(&clean).to_string();
    }
    clean
}

/// With `JOBSEARCH_DEBUG=1` every dropped posting is logged with the reason, which is
/// the way to answer "why is this job not on my board?".
fn explain(source: &str, candidate: &Candidate, why: &str) {
    static ENABLED: LazyLock<bool> = LazyLock::new(|| std::env::var_os("JOBSEARCH_DEBUG").is_some());
    if *ENABLED {
        log_info!("dropped [{source}] \"{}\" at {} ({}): {why}", candidate.title, candidate.company, candidate.location);
    }
}

/// A posting that cleared the cheap checks and is waiting for the final ones.
struct Pending {
    candidate: Candidate,
    key: String,
    fingerprint: String,
    source_index: usize,
    order: usize,
    detail_error: Option<String>,
    fetched_detail: bool,
    /// Its description was not downloaded because the site asked for fewer requests.
    postponed: bool,
}

/// Runs one scan and returns how many cards were added.
pub async fn run(state: &AppState) -> anyhow::Result<usize> {
    let run_id = state.db.start_run()?;
    match scan(state).await {
        Ok((added, stats)) => {
            let ok = stats.iter().any(|s| s.error.is_none());
            state.db.finish_run(run_id, added, ok, &stats)?;
            Ok(added)
        }
        Err(e) => {
            // Record the failure so the scheduler waits before trying again.
            let _ = state.db.finish_run(run_id, 0, false, &[]);
            Err(e)
        }
    }
}

async fn scan(state: &AppState) -> anyhow::Result<(usize, Vec<SourceStats>)> {
    let settings = state.db.settings();
    let employers = employers::load(&state.paths);
    let results = sources::fetch_all(&state.http, &employers, settings.workplaces).await;

    let mut known = state.db.known_keys()?;
    let rejected = state.db.rejected_keys()?;
    let mut fingerprints = state.db.fingerprints()?;
    let decided = state.db.decided_fingerprints()?;

    // ---- cheap checks, no network ------------------------------------------------
    let mut stats: Vec<SourceStats> = Vec::with_capacity(results.len());
    let mut pending: Vec<Pending> = Vec::new();
    let mut queued: HashSet<String> = HashSet::new();
    let mut order = 0;
    for (source_index, result) in results.into_iter().enumerate() {
        let mut s = SourceStats {
            source: result.source,
            label: result.label,
            error: result.error,
            ..SourceStats::default()
        };
        let mut detail_budget = DETAIL_FETCHES_PER_SOURCE;
        for candidate in result.candidates {
            s.fetched += 1;
            let key = candidate.key();
            if known.contains(&key) || rejected.contains(&key) || !queued.insert(key.clone()) {
                s.already_known += 1;
                continue;
            }
            // Dropped here only when nothing its description could say would make
            // it a kind of job the user wants.
            let (listed, why) = filter::workplace_by_listing(&candidate.title, &candidate.location, candidate.remote);
            if !settings.workplaces.might_want(listed) {
                s.not_remote += 1;
                explain(&s.label, &candidate, why);
                continue;
            }
            // A listing whose details are still to be fetched may not name its country
            // yet, so its place is judged after the fetch instead. A listing that does
            // not read as remote never becomes remote, so it can be held to the
            // places the user can travel to straight away.
            let place_known = candidate.detail_url.is_none();
            let out_of_reach = place_known
                && (!filter::in_region(&candidate.location, &settings)
                    || (listed != Workplace::Remote && !filter::is_nearby(&candidate.location, &settings)));
            if !filter::wants_role(&candidate.title, &settings) || out_of_reach {
                s.filtered_out += 1;
                explain(&s.label, &candidate, "outside your role or place settings");
                continue;
            }
            // A hidden phrase in the title settles it before anything is downloaded.
            if let Some(term) = filter::hidden_term(&candidate.title, "", &settings) {
                s.filtered_out += 1;
                explain(&s.label, &candidate, &format!("the title mentions \u{201c}{term}\u{201d}, which you have hidden"));
                continue;
            }
            let fingerprint = filter::fingerprint(&candidate.company, &candidate.title);
            // Employers repost a job under a new id, and list one job once per
            // location. Once the user has applied or passed, the rest stay away.
            if already_decided(&fingerprint, &decided) {
                s.duplicate += 1;
                explain(&s.label, &candidate, "a card for the same job is already in a column other than New or Evaluating");
                continue;
            }
            if !candidate.direct && fingerprints.contains(&fingerprint) {
                s.duplicate += 1;
                continue;
            }
            let needs_detail = candidate.detail_url.is_some() && candidate.description_html.is_empty();
            if needs_detail {
                if detail_budget == 0 {
                    s.deferred += 1;
                    continue;
                }
                detail_budget -= 1;
            }
            pending.push(Pending {
                candidate,
                key,
                fingerprint,
                source_index,
                order,
                detail_error: None,
                fetched_detail: false,
                postponed: false,
            });
            order += 1;
        }
        stats.push(s);
    }

    // ---- full descriptions, one source at a time per site, sites in parallel -------
    let (need_detail, mut ready): (Vec<Pending>, Vec<Pending>) = pending
        .into_iter()
        .partition(|p| p.candidate.detail_url.is_some() && p.candidate.description_html.is_empty());
    let mut by_source: BTreeMap<usize, Vec<Pending>> = BTreeMap::new();
    for p in need_detail {
        by_source.entry(p.source_index).or_default().push(p);
    }
    let mut tasks: JoinSet<Vec<Pending>> = JoinSet::new();
    for (_, group) in by_source {
        let http = state.http.clone();
        tasks.spawn(async move {
            let label = group.first().map(|p| p.candidate.source_label.clone()).unwrap_or_default();
            let mut waits = RATE_LIMIT_WAITS.iter();
            let mut postponed = 0;
            let mut done = Vec::with_capacity(group.len());
            for mut p in group {
                if postponed > 0 {
                    p.postponed = true;
                    postponed += 1;
                    done.push(p);
                    continue;
                }
                loop {
                    match sources::fetch_detail(&http, &mut p.candidate).await {
                        Ok(()) => p.fetched_detail = true,
                        // Hold off and ask again; a site that keeps saying so is left alone.
                        Err(e) if sources::is_rate_limited(&e) => match waits.next() {
                            Some(wait) => {
                                tokio::time::sleep(*wait).await;
                                continue;
                            }
                            None => {
                                p.postponed = true;
                                postponed = 1;
                            }
                        },
                        Err(e) => p.detail_error = Some(format!("{e:#}")),
                    }
                    break;
                }
                done.push(p);
                tokio::time::sleep(DETAIL_PAUSE).await;
            }
            if postponed > 0 {
                log_warn!("{label}: the site asked for fewer requests; {postponed} posting(s) left for the next check");
            }
            done
        });
    }
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(group) => ready.extend(group),
            Err(e) => log_warn!("a detail task failed: {e}"),
        }
    }
    // Back to source order: employers' own listings are decided before board copies.
    ready.sort_by_key(|p| p.order);

    // ---- final checks and insert ---------------------------------------------------
    let mut added_total = 0;
    for p in ready {
        let s = &mut stats[p.source_index];
        let c = p.candidate;
        if p.postponed {
            s.deferred += 1;
            continue;
        }
        if let Some(error) = p.detail_error {
            log_warn!("{}: could not read \"{}\": {error}", s.label, c.title);
            s.deferred += 1;
            continue;
        }
        // Only a rejection based on the full posting is remembered, so that posting
        // is not downloaded again on every check. One caused by the user's settings
        // is remembered only until they change them.
        let remember_as = |reason: &str, by_settings: bool| {
            if p.fetched_detail {
                if let Err(e) = state.db.remember_rejected(&p.key, reason, by_settings) {
                    log_warn!("could not record a rejected posting: {e}");
                }
            }
        };
        let remember = |reason: &str| remember_as(reason, false);

        // The listing is read again: a detail page carries the employer's
        // authoritative workplace label, and the description has its say too.
        let description = filter::plain_text(&c.description_html);
        let (workplace, why) = filter::workplace(&c.title, &c.location, c.remote, &description);
        if !settings.workplaces.wants(workplace) {
            s.not_remote += 1;
            explain(&s.label, &c, &why);
            // Kept until the user changes which kinds of job they want.
            remember(&why);
            continue;
        }
        if p.fetched_detail && !filter::in_region(&c.location, &settings) {
            s.filtered_out += 1;
            explain(&s.label, &c, "outside your region settings");
            remember_as("outside the region settings", true);
            continue;
        }
        if workplace != Workplace::Remote && !filter::is_nearby(&c.location, &settings) {
            s.filtered_out += 1;
            explain(&s.label, &c, "not in one of the places you can travel to");
            remember_as("outside the places you can travel to", true);
            continue;
        }
        if let Some(term) = filter::hidden_term(&c.title, &c.description_html, &settings) {
            s.filtered_out += 1;
            let why = format!("the posting mentions \u{201c}{term}\u{201d}, which you have hidden");
            explain(&s.label, &c, &why);
            remember_as(&why, true);
            continue;
        }
        if !c.direct && fingerprints.contains(&p.fingerprint) {
            s.duplicate += 1;
            continue;
        }
        let Some(verdict) = verdict_for(&c, &description, &employers, &settings) else {
            s.not_inclusive += 1;
            explain(&s.label, &c, "nothing shows the employer welcomes neurodivergent or disabled applicants");
            // A care-role rejection depends on the role settings, so only a posting
            // with no inclusion language at all is remembered.
            if filter::inclusion_signal(&description).is_none() {
                remember(NO_INCLUSION_SIGNAL);
            }
            continue;
        };

        let job = NewJob {
            dedupe_key: p.key.clone(),
            fingerprint: p.fingerprint.clone(),
            title: c.title,
            company: c.company,
            location: c.location,
            url: c.url,
            source: c.source,
            source_label: c.source_label,
            description: sanitize(&c.description_html),
            salary: c.salary,
            reason: verdict.reason,
            reason_kind: verdict.kind.to_string(),
            evidence_url: verdict.evidence_url,
            posted_at: c.posted_at,
            direct: c.direct,
            workplace,
        };
        if state.db.insert_job(&job)? {
            s.added += 1;
            added_total += 1;
            known.insert(p.key);
            fingerprints.insert(p.fingerprint);
        } else {
            s.already_known += 1;
        }
    }

    for s in &stats {
        match &s.error {
            Some(error) => log_warn!("{}: failed: {error}", s.label),
            None => log_info!(
                "{}: {} fetched, {} added, {} not a wanted workplace, {} not inclusive, {} filtered, {} seen before",
                s.label, s.fetched, s.added, s.not_remote, s.not_inclusive, s.filtered_out, s.already_known
            ),
        }
    }
    Ok((added_total, stats))
}

/// True when the user has already dealt with this job: a card for the same employer
/// and title is in a column other than New and Evaluating. A posting that does not
/// name its employer never matches, because a title alone does not identify a job.
fn already_decided(fingerprint: &str, decided: &HashSet<String>) -> bool {
    !fingerprint.starts_with('|') && decided.contains(fingerprint)
}

struct Verdict {
    kind: &'static str,
    reason: String,
    evidence_url: String,
}

/// What a posting's card says about the employer, or `None` when the posting gets
/// no card. While the user keeps the board to inclusive employers, that is every
/// posting `judge_inclusion` turns down. Otherwise those get a card too, one that
/// claims nothing about the employer.
fn verdict_for(
    candidate: &Candidate,
    description: &str,
    employers: &[Employer],
    settings: &Settings,
) -> Option<Verdict> {
    judge_inclusion(candidate, description, employers, settings).or_else(|| {
        (!settings.inclusive_only).then(|| Verdict {
            kind: NO_INCLUSION_CLAIM,
            reason: "It matches your settings. Nothing shows that this employer specifically recruits neurodivergent or disabled applicants.".to_string(),
            evidence_url: String::new(),
        })
    })
}

/// Decides whether a posting belongs on a board of disability- and
/// neurodiversity-inclusive employers, and records why.
fn judge_inclusion(
    candidate: &Candidate,
    description: &str,
    employers: &[Employer],
    settings: &Settings,
) -> Option<Verdict> {
    let quote = filter::inclusion_signal(description);

    let known = match &candidate.inclusion {
        Inclusion::Known {
            kind,
            reason,
            evidence_url,
        } => Some(Verdict {
            kind: *kind,
            reason: reason.clone(),
            evidence_url: evidence_url.clone(),
        }),
        // A board posting from an employer on the program list.
        Inclusion::NeedsSignal => employers
            .iter()
            .find(|e| e.mode == Mode::Program && e.is_named(&candidate.company))
            .map(|e| Verdict {
                kind: e.inclusion_kind(),
                reason: e.reason.clone(),
                evidence_url: e.evidence.clone(),
            }),
    };

    match (known, quote) {
        (Some(mut verdict), Some(quote)) => {
            verdict.reason = format!("{}. The posting also says: “{quote}”", verdict.reason.trim_end_matches('.'));
            Some(verdict)
        }
        (Some(verdict), None) => Some(verdict),
        (None, Some(quote)) => {
            // Jobs serving neurodivergent clients mention neurodiversity for a different
            // reason. They are kept only when the user asked for that kind of role.
            if filter::is_care_role(&candidate.title) && !filter::names_role(&candidate.title, settings) {
                return None;
            }
            Some(Verdict {
                kind: "signal",
                reason: format!("The posting says: “{quote}”"),
                evidence_url: String::new(),
            })
        }
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Remote, Workplaces};

    fn candidate(company: &str, title: &str, inclusion: Inclusion) -> Candidate {
        Candidate {
            source: "test".into(),
            source_label: "Test".into(),
            external_id: "1".into(),
            title: title.into(),
            company: company.into(),
            location: "Remote".into(),
            url: "https://example.com/job/1".into(),
            description_html: String::new(),
            salary: String::new(),
            posted_at: None,
            remote: Remote::Declared,
            inclusion,
            direct: false,
            detail_url: None,
        }
    }

    fn employers() -> Vec<Employer> {
        vec![Employer {
            name: "Salesforce".into(),
            aliases: vec![],
            mode: Mode::Program,
            kind: String::new(),
            reason: "Runs a neurodiversity hiring program".into(),
            evidence: "https://example.com/evidence".into(),
            ats: String::new(),
            token: String::new(),
            host: String::new(),
            tenant: String::new(),
            site: String::new(),
            public_base: String::new(),
        }]
    }

    const BOILERPLATE: &str = "We are an equal opportunity employer and provide reasonable accommodation to individuals with disabilities.";
    const WELCOMING: &str = "We welcome neurodivergent applicants and will adapt our interview process.";

    #[test]
    fn a_board_posting_needs_the_posting_to_say_so() {
        let c = candidate("Acme", "Backend Engineer", Inclusion::NeedsSignal);
        assert!(judge_inclusion(&c, BOILERPLATE, &employers(), &Settings::default()).is_none());
        let v = judge_inclusion(&c, WELCOMING, &employers(), &Settings::default()).unwrap();
        assert_eq!(v.kind, "signal");
        assert!(v.reason.contains("neurodivergent applicants"), "{}", v.reason);
    }

    #[test]
    fn a_program_employer_is_recognised_on_a_board() {
        let c = candidate("Salesforce, Inc.", "Backend Engineer", Inclusion::NeedsSignal);
        let v = judge_inclusion(&c, BOILERPLATE, &employers(), &Settings::default()).unwrap();
        assert_eq!(v.kind, "program");
        assert_eq!(v.evidence_url, "https://example.com/evidence");
    }

    #[test]
    fn care_roles_need_to_be_asked_for() {
        let text = "Join our neurodiversity-affirming practice supporting autistic children.";
        let c = candidate("Bright Minds", "Behavior Technician (RBT)", Inclusion::NeedsSignal);
        assert!(judge_inclusion(&c, text, &employers(), &Settings::default()).is_none());
        let asked = Settings {
            role_keywords: vec!["behavior technician".into()],
            ..Settings::default()
        };
        assert!(judge_inclusion(&c, text, &employers(), &asked).is_some());
    }

    #[test]
    fn known_sources_qualify_and_quote_the_posting_when_it_adds_something() {
        let known = Inclusion::Known {
            kind: "board",
            reason: "Posted on an autism job board.".into(),
            evidence_url: "https://example.org".into(),
        };
        let c = candidate("Acme", "Analyst", known);
        let plain = judge_inclusion(&c, BOILERPLATE, &[], &Settings::default()).unwrap();
        assert_eq!(plain.reason, "Posted on an autism job board.");
        let quoted = judge_inclusion(&c, WELCOMING, &[], &Settings::default()).unwrap();
        assert!(quoted.reason.starts_with("Posted on an autism job board. The posting also says:"), "{}", quoted.reason);
    }

    #[test]
    fn the_inclusion_rule_can_be_turned_off() {
        let any_employer = Settings {
            inclusive_only: false,
            ..Settings::default()
        };
        let c = candidate("Acme", "Backend Engineer", Inclusion::NeedsSignal);
        // On, which is the default, a posting that shows nothing gets no card.
        assert!(Settings::default().inclusive_only);
        assert!(verdict_for(&c, BOILERPLATE, &employers(), &Settings::default()).is_none());
        // Off, it gets one that makes no claim about the employer.
        let plain = verdict_for(&c, BOILERPLATE, &employers(), &any_employer).unwrap();
        assert_eq!(plain.kind, NO_INCLUSION_CLAIM);
        assert!(plain.evidence_url.is_empty());

        // What is known about an employer is still said.
        let quoted = verdict_for(&c, WELCOMING, &employers(), &any_employer).unwrap();
        assert_eq!(quoted.kind, "signal");
        let program = candidate("Salesforce", "Backend Engineer", Inclusion::NeedsSignal);
        assert_eq!(verdict_for(&program, BOILERPLATE, &employers(), &any_employer).unwrap().kind, "program");

        // A care role mentions neurodiversity because of its clients. That is still
        // not a claim about the employer.
        let care = candidate("Bright Minds", "Behavior Technician (RBT)", Inclusion::NeedsSignal);
        let text = "Join our neurodiversity-affirming practice supporting autistic children.";
        assert_eq!(verdict_for(&care, text, &employers(), &any_employer).unwrap().kind, NO_INCLUSION_CLAIM);
    }

    #[test]
    fn settings_saved_before_the_inclusion_rule_was_optional_keep_it_on() {
        let saved: Settings = serde_json::from_str(r#"{"role_keywords":["developer"],"configured":true}"#).unwrap();
        assert!(saved.inclusive_only);
        assert_eq!(saved.role_keywords, ["developer"]);
        let off: Settings = serde_json::from_str(r#"{"inclusive_only":false}"#).unwrap();
        assert!(!off.inclusive_only);
        // The same goes for the kinds of job: remote only, until the user says otherwise.
        assert_eq!(saved.workplaces, Workplaces::default());
        assert!(saved.nearby.is_empty());
        let wider: Settings = serde_json::from_str(r#"{"workplaces":{"hybrid":true},"nearby":["Seattle"]}"#).unwrap();
        assert_eq!(wider.workplaces, Workplaces { remote: true, hybrid: true, onsite: false });
        assert_eq!(wider.nearby, ["Seattle"]);
    }

    #[test]
    fn the_kinds_of_job_wanted_decide_what_is_read_and_what_is_kept() {
        let remote_only = Workplaces::default();
        assert!(remote_only.wants(Workplace::Remote));
        assert!(!remote_only.wants(Workplace::Hybrid) && !remote_only.wants(Workplace::OnSite) && !remote_only.wants(Workplace::Unstated));
        // A listing that does not say remote is dropped before its description is fetched.
        assert!(remote_only.might_want(Workplace::Remote));
        assert!(!remote_only.might_want(Workplace::Unstated) && !remote_only.might_want(Workplace::Hybrid));

        let with_hybrid = Workplaces {
            hybrid: true,
            ..Workplaces::default()
        };
        assert!(with_hybrid.wants(Workplace::Hybrid) && !with_hybrid.wants(Workplace::Unstated));
        // An unlabelled listing may turn out to be hybrid once its description is read.
        assert!(with_hybrid.might_want(Workplace::Unstated));
        assert!(!with_hybrid.might_want(Workplace::OnSite));

        let onsite_only = Workplaces {
            remote: false,
            hybrid: false,
            onsite: true,
        };
        // A posting that does not say where the work is done is taken as on-site.
        assert!(onsite_only.wants(Workplace::OnSite) && onsite_only.wants(Workplace::Unstated));
        assert!(!onsite_only.wants(Workplace::Remote) && !onsite_only.wants(Workplace::Hybrid));
        // A description can show that a job listed as remote is in fact on-site.
        assert!(onsite_only.might_want(Workplace::Remote));
        assert!(!onsite_only.might_want(Workplace::Hybrid));
    }

    #[test]
    fn a_job_the_user_has_dealt_with_is_not_added_again() {
        let decided: HashSet<String> = [
            filter::fingerprint("General Dynamics Information Technology", "Software Developer"),
            filter::fingerprint("", "Support Engineer"),
        ]
        .into();
        let repost = |company: &str, title: &str| already_decided(&filter::fingerprint(company, title), &decided);
        // The same employer and title, however the posting writes them.
        assert!(repost("General Dynamics Information Technology", "Software Developer"));
        assert!(repost("General Dynamics Information Technology, Inc.", "SOFTWARE DEVELOPER"));
        // Another title at that employer, or that title somewhere else, is a different job.
        assert!(!repost("General Dynamics Information Technology", "Senior Software Developer"));
        assert!(!repost("General Motors", "Software Developer"));
        // With no employer named there is nothing to say it is the same job.
        assert!(!repost("", "Support Engineer"));
    }

    #[test]
    fn descriptions_are_made_safe() {
        let dirty = r#"<p onclick="x()">Hi <script>alert(1)</script><img src="https://t.example/p.gif"><a href="https://example.com">link</a> <a href="javascript:alert(1)">bad</a></p>"#;
        let clean = sanitize(dirty);
        assert!(!clean.contains("script"), "{clean}");
        assert!(!clean.contains("onclick"), "{clean}");
        assert!(!clean.contains("<img"), "{clean}");
        assert!(!clean.contains("javascript:"), "{clean}");
        assert!(clean.contains(r#"href="https://example.com""#), "{clean}");
        assert!(clean.contains("noopener"), "{clean}");
        assert!(clean.contains(r#"target="_blank""#), "{clean}");
    }
}
