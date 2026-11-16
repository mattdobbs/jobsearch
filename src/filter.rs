//! The rules that decide whether a posting becomes a card.
//!
//! Remote-only is a hard requirement, so the remote checks are deliberately strict:
//! when a listing is ambiguous it is dropped. Missing a genuinely remote job is the
//! acceptable failure; showing a hybrid or on-site one is not.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::model::{Remote, Settings};

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("built-in regex is valid")
}

static TAGS: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?is)<script[^>]*>.*?</script>|<style[^>]*>.*?</style>|<[^>]+>")
});
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));

/// Strips markup so the text rules below see what a reader would see.
pub fn plain_text(html: &str) -> String {
    let without_tags = TAGS.replace_all(html, " ");
    let decoded = html_escape::decode_html_entities(&without_tags);
    WHITESPACE.replace_all(&decoded, " ").trim().to_string()
}

// ---- remote ---------------------------------------------------------------

/// Words in a title or location that mean the job is not fully remote.
static NOT_REMOTE_LABEL: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b(hybrid|on[- ]?site|in[- ]office|in[- ]person|office[- ]based)\b")
});

/// A location field that names remote work. Locations describe a place, so any of
/// these words there is trustworthy.
static REMOTE_PLACE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b(remote|work from home|wfh|telecommut\w*|virtual|anywhere|home[- ]based|worldwide)\b")
});

/// A title that labels the job remote. "Remote" also appears in titles as a subject
/// ("Remote Sensing Scientist", "Remote Operations Center"), so it only counts when
/// it is set apart: in brackets, after a separator, or next to a region.
static REMOTE_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)(",
        r"\bfully remote\b|\b100% remote\b|\bremote[- ]first\b|\bwork from home\b|\bwfh\b|\btelecommut\w*",
        r"|[(\[][^)\]]*\bremote\b[^)\]]*[)\]]",
        r"|[-–—|,:/]\s*remote\s*($|[-–—|,:/(])",
        r"|^\s*remote\s*[-–—|,:/]",
        r"|\bremote\s*[-–—,]?\s*(us|usa|u\.s\.?a?\.?|united states|uk|canada|emea|europe|americas|apac|latam|worldwide|global|anywhere|eligible)\b",
        r"|\b(us|usa|united states|uk|canada|emea|europe|americas)\s*[-–—,]?\s*remote\b",
        r")"
    ))
});

/// Phrases in a description that say this particular job is not remote.
static NOT_REMOTE_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)\b(",
        r"this (is|role is|position is|job is) (a |an )?(hybrid|on[- ]?site|in[- ]office|in[- ]person|office[- ]based)",
        r"|(hybrid|on[- ]?site|in[- ]office|office[- ]based) (role|position|job|opportunity)",
        r"|hybrid (work(ing)? )?schedule",
        r"|(is|are) not (a )?(fully )?remote",
        r"|not (eligible|available) for remote",
        r"|(days?|times?) (per|a|each) week (in|at|from) (the|our|an?) [a-z ]{0,24}office",
        r"|days? (in|at) (the|our) office",
        r"|(must|required to|need to|expected to) (be able to )?(commute|relocate|work on[- ]?site|be on[- ]?site|come into (the|our) office|work from (the|our) [a-z ]{0,24}office)",
        r")\b"
    ))
});

/// Phrases that describe a company's general way of working rather than this job.
/// On their own they count against the posting; they are overridden only when the
/// description also states outright that this role is remote.
static HYBRID_COMPANY_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\bhybrid (work(ing)? )?(model|arrangement|environment)\b")
});

/// A description stating outright that this role is remote.
static REMOTE_AFFIRMED: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)\b(",
        r"this (is|role is|position is|job is) (a |an )?(fully |100% |completely |entirely )?remote",
        r"|(fully|100%|completely|entirely) remote",
        r"|this remote (role|position|job)",
        r"|remote (role|position|job|opportunity) (open|based|for|within|in|available)",
        r"|work(ing)? (fully |entirely |completely )?remotely from",
        r"|work from (home|anywhere)",
        r")\b"
    ))
});

/// Checks the title and location. `Err` carries the reason the posting was dropped.
pub fn check_remote_labels(title: &str, location: &str, remote: Remote) -> Result<(), &'static str> {
    if NOT_REMOTE_LABEL.is_match(title) {
        return Err("title says hybrid or on-site");
    }
    if NOT_REMOTE_LABEL.is_match(location) {
        return Err("location says hybrid or on-site");
    }
    match remote {
        Remote::Declared => Ok(()),
        Remote::No => Err("the employer labels it as not remote"),
        Remote::Unverified => {
            if REMOTE_PLACE.is_match(location) || REMOTE_TITLE.is_match(title) {
                Ok(())
            } else {
                Err("listing does not say remote")
            }
        }
    }
}

/// Checks the description text for phrases that contradict a remote label.
pub fn check_remote_text(text: &str) -> Result<(), String> {
    if let Some(found) = NOT_REMOTE_TEXT.find(text) {
        return Err(format!("description says \"{}\"", found.as_str()));
    }
    if let Some(found) = HYBRID_COMPANY_TEXT.find(text) {
        if !REMOTE_AFFIRMED.is_match(text) {
            return Err(format!("description says \"{}\"", found.as_str()));
        }
    }
    Ok(())
}

// ---- inclusion ------------------------------------------------------------

/// Language an employer uses when it is actively recruiting neurodivergent or
/// disabled people. The standard equal-opportunity paragraph ("…without regard to
/// race, religion, disability…", "reasonable accommodation for individuals with
/// disabilities") is on nearly every posting and deliberately does not match.
static INCLUSION_SIGNAL: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)(",
        r"\bneuro-?divers\w*|\bneuro-?divergen\w*|\bneuro-?inclusi\w*|\bneuro-?distinct\w*",
        r"|\bautism at work\b|\bautism hiring\b",
        r"|\bautistic (talent|candidates?|applicants?|professionals?|adults?|employees?|colleagues?|people|individuals?)\b",
        r"|\bdisability confident\b|\bdisability:in\b|\bdisability equality index\b",
        r"|\bdisability[- ]inclusi\w+|\bdisability hiring\b",
        r"|\b(encourag\w+|welcom\w+) (applications? from |applicants |candidates )?(people|persons|individuals|applicants|candidates|those|job ?seekers) with (a )?disabilit\w+",
        r"|\b(people|persons|individuals|applicants|candidates|those|job ?seekers) with (a )?disabilit\w+ (are |is )?(strongly |especially |particularly )?(encouraged|welcome)",
        r"|\bdisabled (people|applicants|candidates|talent|job ?seekers) (are |is )?(strongly |especially |particularly )?(encouraged|welcome)",
        r")"
    ))
});

/// Titles for jobs that serve neurodivergent or disabled clients. Their postings talk
/// about autism and neurodiversity because of who the clients are, not because of who
/// the employer hires.
static CARE_ROLE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)\b(rbt|bcba|aba|behaviou?r(al)? (technician|therapist|analyst|interventionist|specialist|health)",
        r"|therapists?|psychotherapists?|counsell?ors?|(neuro)?psycholog\w+|psychiatr\w+|social workers?",
        r"|lcsw|lmft|lpc|speech[- ]language|slp|occupational therap\w+|special education|sped",
        r"|paraprofessionals?|direct support|dsp|caregivers?|support workers?|nurses?|pmhnp",
        r"|clinicians?|teachers?|tutors?)\b"
    ))
});

/// Returns the sentence fragment that shows the posting welcomes neurodivergent or
/// disabled applicants, or `None` when it says nothing beyond boilerplate.
pub fn inclusion_signal(text: &str) -> Option<String> {
    let found = INCLUSION_SIGNAL.find(text)?;
    Some(snippet(text, found.start(), found.end()))
}

pub fn is_care_role(title: &str) -> bool {
    CARE_ROLE.is_match(title)
}

/// Cuts the sentence containing the matched range out of `text`, as a readable quote.
fn snippet(text: &str, start: usize, end: usize) -> String {
    const BEFORE: usize = 160;
    const AFTER: usize = 200;

    let mut window_start = start.saturating_sub(BEFORE);
    while !text.is_char_boundary(window_start) {
        window_start -= 1;
    }
    let mut window_end = (end + AFTER).min(text.len());
    while !text.is_char_boundary(window_end) {
        window_end += 1;
    }

    let before = &text[window_start..start];
    let from = match before.rfind(['.', '!', '?', '\u{2022}']) {
        Some(pos) => {
            let terminator = before[pos..].chars().next().map_or(1, char::len_utf8);
            window_start + pos + terminator
        }
        None => window_start,
    };
    let after = &text[end..window_end];
    let to = match after.find(['.', '!', '?']) {
        Some(pos) => end + pos + 1,
        None => window_end,
    };

    let mut quote = text[from..to].trim().to_string();
    if from == window_start && window_start > 0 {
        quote.insert(0, '\u{2026}');
    }
    if to == window_end && window_end < text.len() {
        quote.push('\u{2026}');
    }
    quote
}

// ---- the user's own filters -------------------------------------------------

/// Case-insensitive containment, used for places. Short terms ("US", "UK") must match
/// a whole word so they do not fire inside longer words.
fn contains_term(haystack_lower: &str, term: &str) -> bool {
    let term = term.trim().to_lowercase();
    if term.is_empty() {
        return false;
    }
    if term.chars().count() > 3 {
        return haystack_lower.contains(&term);
    }
    let mut search_from = 0;
    while let Some(pos) = haystack_lower[search_from..].find(&term) {
        let begin = search_from + pos;
        let finish = begin + term.len();
        let before_ok = haystack_lower[..begin]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = haystack_lower[finish..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        search_from = finish;
    }
    false
}

/// Folds the common ways job titles abbreviate or inflect a word into one form, so
/// "Sr. Systems Admin" and "senior system administrator" compare equal.
fn canonical_words(word: &str) -> Vec<String> {
    let fixed: &[&str] = match word {
        "sr" | "snr" => &["senior"],
        "jr" => &["junior"],
        "admin" | "admins" | "administration" | "administrators" => &["administrator"],
        "sysadmin" | "sysadmins" => &["system", "administrator"],
        "eng" | "engr" | "engineering" | "engineers" => &["engineer"],
        "dev" | "devs" | "developers" => &["developer"],
        "mgr" | "managers" => &["manager"],
        "ops" => &["operation"],
        "interns" | "internship" | "internships" => &["intern"],
        _ => &[],
    };
    if !fixed.is_empty() {
        return fixed.iter().map(|w| w.to_string()).collect();
    }
    // A plain plural: "systems" -> "system". Words like "status" or "analysis" are
    // left alone.
    let plural = word.len() > 3
        && word.ends_with('s')
        && !word.ends_with("ss")
        && !word.ends_with("us")
        && !word.ends_with("is");
    vec![if plural { word[..word.len() - 1].to_string() } else { word.to_string() }]
}

/// Splits a job title, or a keyword the user typed, into comparable words.
fn title_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '+' || c == '#'))
        .filter(|word| !word.is_empty())
        .flat_map(canonical_words)
        .collect()
}

/// A keyword matches a title when every word of the keyword appears in the title, in
/// any order: "system engineer" matches "Senior Systems Engineer" and
/// "Engineer, Billing Systems". Whole words only, so "intern" does not match
/// "Internal".
fn keyword_matches(title: &HashSet<String>, keyword: &str) -> bool {
    let words = title_words(keyword);
    !words.is_empty() && words.iter().all(|word| title.contains(word))
}

/// True when the title matches the roles the user asked for.
pub fn wants_role(title: &str, settings: &Settings) -> bool {
    let title: HashSet<String> = title_words(title).into_iter().collect();
    if settings.exclude_keywords.iter().any(|k| keyword_matches(&title, k)) {
        return false;
    }
    settings.role_keywords.is_empty() || settings.role_keywords.iter().any(|k| keyword_matches(&title, k))
}

/// True when the title matches a role keyword the user explicitly listed.
pub fn names_role(title: &str, settings: &Settings) -> bool {
    let title: HashSet<String> = title_words(title).into_iter().collect();
    settings.role_keywords.iter().any(|k| keyword_matches(&title, k))
}

/// Finds wanted roles that could never produce a job because a hidden keyword covers
/// them. Any title matching the role contains every word of the hidden keyword too,
/// so it would always be hidden. Returns `(role, hidden keyword)` pairs.
pub fn conflicting_keywords(settings: &Settings) -> Vec<(String, String)> {
    let mut conflicts = Vec::new();
    for role in &settings.role_keywords {
        let role_words: HashSet<String> = title_words(role).into_iter().collect();
        if role_words.is_empty() {
            continue;
        }
        for hidden in &settings.exclude_keywords {
            if keyword_matches(&role_words, hidden) {
                conflicts.push((role.clone(), hidden.clone()));
                break;
            }
        }
    }
    conflicts
}

static OPEN_REGION: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b(worldwide|anywhere|global|international)\b")
});
static REMOTE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b(remote|virtual|work from home|wfh|fully|only|location|home[- ]based)\b|[^a-z]")
});

/// The ways a user might write "United States" in their region list.
static US_TERM: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)^\s*(us|u\.s\.?|usa|u\.s\.a\.?|united states|united states of america|america)\s*$")
});
static US_STATE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)\b(alabama|alaska|arizona|arkansas|california|colorado|connecticut|delaware|florida",
        r"|georgia|hawaii|idaho|illinois|indiana|iowa|kansas|kentucky|louisiana|maine|maryland",
        r"|massachusetts|michigan|minnesota|mississippi|missouri|montana|nebraska|nevada",
        r"|new hampshire|new jersey|new mexico|new york|north carolina|north dakota|ohio|oklahoma",
        r"|oregon|pennsylvania|rhode island|south carolina|south dakota|tennessee|texas|utah",
        r"|vermont|virginia|washington|west virginia|wisconsin|wyoming|district of columbia)\b"
    ))
});
/// Regions that include the United States without naming it.
static US_SUPERREGION: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b(namer|north america|americas|amer)\b")
});
/// A two-letter state code in the "City, ST" position. Case-sensitive on purpose.
static US_STATE_CODE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r",\s*(AL|AK|AZ|AR|CA|CO|CT|DE|FL|GA|HI|ID|IL|IN|IA|KS|KY|LA|ME|MD|MA|MI|MN|MS|MO|MT|NE|NV",
        r"|NH|NJ|NM|NY|NC|ND|OH|OK|OR|PA|RI|SC|SD|TN|TX|UT|VT|VA|WA|WV|WI|WY|DC)\b"
    ))
});

/// True when the job can be done from one of the user's regions. A location that
/// names no place at all ("Remote") is kept, since it cannot be ruled out. US
/// postings often name only a state, or a wider region such as "NAMER", so a US
/// region also matches those.
pub fn in_region(location: &str, settings: &Settings) -> bool {
    if settings.regions.is_empty() {
        return true;
    }
    let lower = location.to_lowercase();
    if OPEN_REGION.is_match(&lower) || REMOTE_WORDS.replace_all(&lower, "").is_empty() {
        return true;
    }
    if settings.regions.iter().any(|r| contains_term(&lower, r)) {
        return true;
    }
    settings.regions.iter().any(|r| US_TERM.is_match(r))
        && (US_STATE.is_match(location) || US_STATE_CODE.is_match(location) || US_SUPERREGION.is_match(location))
}

// ---- identity ---------------------------------------------------------------

static COMPANY_NOISE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"\b(the|inc|incorporated|llc|ltd|limited|corp|corporation|co|company|gmbh|plc|careers|group)\b")
});
static NON_ALNUM: LazyLock<Regex> = LazyLock::new(|| re(r"[^a-z0-9]+"));

/// Reduces a company name to a comparable form: "Dell Technologies, Inc." and
/// "dell technologies" compare equal.
pub fn normalize_company(name: &str) -> String {
    let lower = name.to_lowercase().replace('&', " and ");
    let spaced = NON_ALNUM.replace_all(&lower, " ");
    let trimmed = COMPANY_NOISE.replace_all(&spaced, " ");
    let collapsed = WHITESPACE.replace_all(trimmed.trim(), " ").to_string();
    if collapsed.is_empty() {
        WHITESPACE.replace_all(spaced.trim(), " ").to_string()
    } else {
        collapsed
    }
}

/// Identifies "the same job" across sources, so a board's copy of a posting does not
/// duplicate the employer's own listing.
pub fn fingerprint(company: &str, title: &str) -> String {
    let title = NON_ALNUM.replace_all(&title.to_lowercase(), " ").trim().to_string();
    format!("{}|{}", normalize_company(company), title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(roles: &[&str], exclude: &[&str], regions: &[&str]) -> Settings {
        Settings {
            role_keywords: roles.iter().map(|s| s.to_string()).collect(),
            exclude_keywords: exclude.iter().map(|s| s.to_string()).collect(),
            regions: regions.iter().map(|s| s.to_string()).collect(),
            configured: true,
        }
    }

    #[test]
    fn declared_remote_passes_unless_labelled_otherwise() {
        assert!(check_remote_labels("Backend Engineer", "USA", Remote::Declared).is_ok());
        assert!(check_remote_labels("Backend Engineer (Hybrid)", "USA", Remote::Declared).is_err());
        assert!(check_remote_labels("Backend Engineer", "Remote or Hybrid - NYC", Remote::Declared).is_err());
        assert!(check_remote_labels("Onsite Support Engineer", "Austin", Remote::Declared).is_err());
        assert!(check_remote_labels("Field Tech - On-Site", "", Remote::Declared).is_err());
        assert!(check_remote_labels("Analyst", "In-office, Chicago", Remote::Declared).is_err());
        // The employer's own label beats a title that says remote.
        assert!(check_remote_labels("Software Engineer (REMOTE)", "Remote, USA", Remote::No).is_err());
    }

    #[test]
    fn unverified_needs_a_remote_label() {
        let ok = |title: &str, location: &str| check_remote_labels(title, location, Remote::Unverified).is_ok();
        // Location carries the label.
        assert!(ok("Engineer", "Remote, United States"));
        assert!(ok("Engineer", "USA NH Remote Location"));
        assert!(ok("Counsel", "Massachusetts - Virtual"));
        assert!(ok("Engineer", "California - Remote"));
        // Title carries the label.
        assert!(ok("Multifamily Appraisals, Senior (Remote)", "6 Locations"));
        assert!(ok("Application Management Lead - remote US", "USA - ME - ANY CITY"));
        assert!(ok("Account Manager - Remote", "Texas"));
        assert!(ok("Remote - Claims Adjuster", "Ohio"));
        assert!(ok("Fully Remote Paralegal", "Ohio"));
        // No label at all.
        assert!(!ok("Engineer", "San Jose, CA"));
        assert!(!ok("Engineer", "14 Locations"));
        // "Remote" used as a subject, not a workplace.
        assert!(!ok("Remote Sensing Scientist", "Boulder, CO"));
        assert!(!ok("Scientist - Remote Sensing", "Boulder, CO"));
        assert!(!ok("Remote Operations Center Specialist", "Atlanta, GA"));
        // Mixed labels are dropped.
        assert!(!ok("Network Engineer, Hybrid Remote & On-Site OK", "6 Locations"));
        assert!(!ok("Engineer", "Remote / Hybrid"));
    }

    #[test]
    fn description_can_contradict_a_remote_label() {
        assert!(check_remote_text("This is a fully remote role open to anyone in the US.").is_ok());
        assert!(check_remote_text("Experience with hybrid cloud deployments required.").is_ok());
        assert!(check_remote_text("This is a hybrid role based in our Boston office.").is_err());
        assert!(check_remote_text("This position is on-site in Denver.").is_err());
        assert!(check_remote_text("We expect three days per week in the office.").is_err());
        assert!(check_remote_text("Please note this role is not remote.").is_err());
        // A company-wide phrase counts against the posting on its own...
        assert!(check_remote_text("We follow a hybrid work model.").is_err());
        assert!(check_remote_text("Our hybrid working environment keeps teams connected.").is_err());
        // ...unless the description states outright that this role is remote.
        assert!(check_remote_text("This is a remote position open to candidates in the US. We embrace a flexible hybrid model across the company.").is_ok());
        assert!(check_remote_text("This role is fully remote. Elsewhere we run a hybrid working model.").is_ok());
        // A statement about this job always wins over an affirmation elsewhere.
        assert!(check_remote_text("We are 100% remote friendly. This is a hybrid role in Austin.").is_err());
        assert!(check_remote_text("Fully remote teams exist, but this role has a hybrid schedule.").is_err());
        assert!(check_remote_text("Candidates must relocate to Austin.").is_err());
    }

    #[test]
    fn inclusion_language_is_detected() {
        for text in [
            "We welcome neurodivergent applicants and offer adjustments throughout hiring.",
            "Our Neurodiversity Hiring Program supports candidates on the spectrum.",
            "We are a neuro-inclusive employer.",
            "Part of our Autism at Work program.",
            "We are a Disability Confident employer.",
            "We strongly encourage people with disabilities to apply.",
            "Candidates with disabilities are encouraged to apply.",
            "Scored 100 on the Disability Equality Index.",
            "Committed to disability inclusion across the company.",
        ] {
            assert!(inclusion_signal(text).is_some(), "should match: {text}");
        }
    }

    #[test]
    fn equal_opportunity_boilerplate_is_not_a_signal() {
        for text in [
            "We are an equal opportunity employer and do not discriminate on the basis of race, religion, sex, age, disability or veteran status.",
            "We provide reasonable accommodation to qualified individuals with disabilities.",
            "We welcome applicants regardless of race, gender, sexual orientation, age or disability.",
            "If you need an accommodation due to a disability, contact recruiting.",
            "Experience supporting children with autism preferred.",
        ] {
            assert!(inclusion_signal(text).is_none(), "should not match: {text}");
        }
    }

    #[test]
    fn snippet_quotes_the_matching_sentence() {
        let text = "About us. We build tools. We welcome neurodivergent applicants and adapt our process. Benefits include health cover.";
        let quote = inclusion_signal(text).unwrap();
        assert!(quote.contains("neurodivergent applicants"), "{quote}");
        assert!(!quote.contains("Benefits"), "{quote}");
        // Must not panic on multi-byte text around the match.
        let unicode = format!("{} neurodiversity {}", "é".repeat(200), "ü".repeat(300));
        assert!(inclusion_signal(&unicode).is_some());
    }

    #[test]
    fn care_roles_are_recognised() {
        assert!(is_care_role("Behavior Technician (BT/RBT)"));
        assert!(is_care_role("Direct Support Professional"));
        assert!(is_care_role("Special Education Teacher"));
        assert!(is_care_role("Licensed Therapist - Remote"));
        assert!(is_care_role("Neuropsychologist - Contract (1099) - Texas"));
        assert!(!is_care_role("Senior Software Engineer"));
        assert!(!is_care_role("Customer Support Specialist"));
        assert!(!is_care_role("Product Manager, Clinical Tools"));
    }

    #[test]
    fn role_keywords_filter_titles() {
        let any = settings(&[], &[], &[]);
        assert!(wants_role("Anything At All", &any));
        let s = settings(&["engineer", "QA"], &["senior staff", "intern"], &[]);
        assert!(wants_role("Backend Engineer", &s));
        assert!(wants_role("QA Analyst", &s));
        assert!(!wants_role("Product Designer", &s));
        assert!(!wants_role("Engineering Intern", &s));
        // Short keywords match whole words only.
        assert!(!wants_role("Aquarium Keeper", &s));
        assert!(names_role("QA Analyst", &s));
        assert!(!names_role("QA Analyst", &any));
    }

    #[test]
    fn titles_match_despite_plurals_abbreviations_and_word_order() {
        let s = settings(
            &["senior infrastructure engineer", "system engineer", "system administrator", "devops engineer", "developer"],
            &[],
            &[],
        );
        for title in [
            "Senior Systems Engineer",
            "Systems Engineer II",
            "Engineer, Billing Systems",
            "Linux System Admin",
            "Sysadmin (Remote)",
            "Systems Administrator - Windows",
            "Sr. DevOps Engineer / DevOps Architect",
            "DevOps Engineering Lead",
            "Sr. Engineer, Infrastructure Security",
            "Senior Infrastructure Engineer",
            "Salesforce Developer",
            "Software Developers (Go)",
        ] {
            assert!(wants_role(title, &s), "should match: {title}");
        }
        for title in [
            // "senior" was asked for.
            "Infrastructure Engineer",
            "Staff Infrastructure Engineer",
            // Not the same word.
            "Business Development Representative",
            "Account Executive",
            "Site Reliability Manager",
        ] {
            assert!(!wants_role(title, &s), "should not match: {title}");
        }
    }

    #[test]
    fn hidden_keywords_match_whole_words() {
        let s = settings(&[], &["intern", "director"], &[]);
        assert!(!wants_role("Software Engineering Intern", &s));
        assert!(!wants_role("Internship - Data Science", &s));
        assert!(!wants_role("Director of Engineering", &s));
        // "Internal" and "International" are different words.
        assert!(wants_role("Internal Tools Engineer", &s));
        assert!(wants_role("International Payroll Specialist", &s));
    }

    #[test]
    fn settings_that_can_never_match_are_detected() {
        // The same title in both lists.
        let same = settings(&["devops engineer", "developer"], &["DevOps Engineer", "developer"], &[]);
        assert_eq!(conflicting_keywords(&same).len(), 2);
        // A hidden word that covers a wanted role.
        let covered = settings(&["devops engineer", "system administrator"], &["engineer"], &[]);
        assert_eq!(
            conflicting_keywords(&covered),
            vec![("devops engineer".to_string(), "engineer".to_string())]
        );
        // Spelling variants are still the same title.
        let variant = settings(&["Systems Admin"], &["system administrator"], &[]);
        assert_eq!(conflicting_keywords(&variant).len(), 1);
        // A narrower hidden keyword is a legitimate refinement.
        let fine = settings(&["engineer"], &["sales engineer", "intern"], &[]);
        assert!(conflicting_keywords(&fine).is_empty());
    }

    #[test]
    fn region_filter_keeps_matching_and_unspecified_locations() {
        let s = settings(&[], &[], &["United States", "USA", "US"]);
        assert!(in_region("Remote, United States", &s));
        assert!(in_region("USA", &s));
        assert!(in_region("US", &s));
        assert!(in_region("Worldwide", &s));
        assert!(in_region("Anywhere in the World", &s));
        assert!(in_region("Remote", &s));
        assert!(in_region("", &s));
        assert!(!in_region("Germany", &s));
        assert!(!in_region("Remote - UK", &s));
        // "US" must not match inside "Australia" or "Austria".
        assert!(!in_region("Australia", &s));
        // A US region also covers postings that name only a state.
        assert!(in_region("California Remote (+13 more)", &s));
        assert!(in_region("Brooklyn, NY (Remote)", &s));
        assert!(in_region("Remote - New Mexico", &s));
        assert!(!in_region("Toronto, ON", &s));
        // ...and regions that contain the US.
        assert!(in_region("NAMER; EMEA", &s));
        assert!(in_region("North America", &s));
        assert!(in_region("Americas", &s));
        assert!(!in_region("EMEA", &s));
        assert!(!in_region("LATAM", &s));
        // ...but only when the user asked for the US.
        assert!(!in_region("California", &settings(&[], &[], &["Canada"])));
        assert!(in_region("Germany", &settings(&[], &[], &[])));
    }

    #[test]
    fn fingerprints_match_across_naming_variants() {
        assert_eq!(normalize_company("Dell Technologies, Inc."), normalize_company("dell technologies"));
        assert_eq!(fingerprint("GitLab Inc.", "Senior Backend Engineer"), fingerprint("GitLab", "Senior  Backend Engineer"));
        assert_ne!(fingerprint("GitLab", "Backend Engineer"), fingerprint("GitLab", "Frontend Engineer"));
        // A name made only of noise words still yields something comparable.
        assert!(!normalize_company("The Company").is_empty());
    }

    #[test]
    fn plain_text_strips_markup() {
        assert_eq!(plain_text("<p>Hello&nbsp;<b>world</b></p><script>x()</script>"), "Hello world");
    }
}
