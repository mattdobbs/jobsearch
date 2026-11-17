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

/// Tags that start a new line when a description is shown.
static BLOCK_TAGS: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)<br\s*/?>|</?(p|div|li|ul|ol|h[1-6]|tr|td|th|table|section|article|blockquote|dl|dt|dd|pre|hr)\b[^>]*>")
});

/// Like `plain_text`, but keeps the line breaks a reader would see, so a label and
/// the next label ("Travel: No", "Clearance: Secret") do not run together.
pub fn block_text(html: &str) -> String {
    let broken = BLOCK_TAGS.replace_all(html, "\n");
    let without_tags = TAGS.replace_all(&broken, " ");
    let decoded = html_escape::decode_html_entities(&without_tags);
    // Some career sites escape their text twice, leaving "&#xa;" where a line break
    // was meant. A second pass restores it.
    html_escape::decode_html_entities(&decoded).into_owned()
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

/// The entry of the title hide list that this title matches, if any.
fn hidden_title_keyword(title: &str, settings: &Settings) -> Option<String> {
    if settings.exclude_keywords.is_empty() {
        return None;
    }
    let title: HashSet<String> = title_words(title).into_iter().collect();
    settings.exclude_keywords.iter().find(|k| keyword_matches(&title, k)).cloned()
}

/// True when the title matches a role keyword the user explicitly listed.
pub fn names_role(title: &str, settings: &Settings) -> bool {
    let title: HashSet<String> = title_words(title).into_iter().collect();
    settings.role_keywords.iter().any(|k| keyword_matches(&title, k))
}

/// Finds wanted roles that could never produce a job because an entry in one of the
/// hide lists covers them: any title matching the role would match the hidden entry
/// too. Returns `(role, hidden entry)` pairs.
pub fn conflicting_keywords(settings: &Settings) -> Vec<(String, String)> {
    let mut conflicts = Vec::new();
    for role in &settings.role_keywords {
        let role_words: HashSet<String> = title_words(role).into_iter().collect();
        if role_words.is_empty() {
            continue;
        }
        let in_title = settings.exclude_keywords.iter().find(|hidden| keyword_matches(&role_words, hidden));
        // A phrase from the "anywhere" list hides a title that contains it as written.
        let role_tokens = tokens(role);
        let anywhere = || settings.exclude_terms.iter().find(|term| mentions(&role_tokens, term));
        if let Some(hidden) = in_title.or_else(anywhere) {
            conflicts.push((role.clone(), hidden.clone()));
        }
    }
    conflicts
}

// ---- phrases hidden anywhere in a posting -----------------------------------

/// A posting broken into words, keeping only the punctuation that tells a label from
/// its value and one statement from the next.
#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Colon,
    /// A line break or the end of a sentence.
    Break,
}

fn tokens(text: &str) -> Vec<Token> {
    fn flush(word: &mut String, out: &mut Vec<Token>) {
        if !word.is_empty() {
            out.push(Token::Word(std::mem::take(word)));
        }
    }
    fn end_statement(out: &mut Vec<Token>) {
        if !matches!(out.last(), Some(Token::Break) | None) {
            out.push(Token::Break);
        }
    }

    let mut out = Vec::new();
    let mut word = String::new();
    let mut previous: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let next = chars.peek().copied();
        if c.is_alphanumeric() {
            word.extend(c.to_lowercase());
        } else if (c == '+' || c == '#') && !word.is_empty() {
            // "C++", "C#"
            word.push(c);
        } else if c == '.'
            && word.is_empty()
            && next.is_some_and(char::is_alphanumeric)
            && !previous.is_some_and(char::is_alphanumeric)
        {
            // ".NET"
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            match c {
                ':' => out.push(Token::Colon),
                '\n' | '!' | '?' | ';' | '|' | '\u{2022}' => end_statement(&mut out),
                '.' if next.is_none_or(char::is_whitespace) => end_statement(&mut out),
                _ => {}
            }
        }
        previous = Some(c);
    }
    flush(&mut word, &mut out);
    out
}

fn word_at(tokens: &[Token], index: usize) -> Option<&str> {
    match tokens.get(index) {
        Some(Token::Word(word)) => Some(word),
        _ => None,
    }
}

fn is_requirement(word: &str) -> bool {
    matches!(word, "require" | "requires" | "required" | "need" | "needs" | "needed" | "necessary")
}

/// True when the mention at `first..=last` says the thing does not apply:
///
/// - a label whose value is empty: "Security Clearance Type: None/Not Required",
///   "Clearance Level Must Currently Possess: None";
/// - "no security clearance required", "does not require a security clearance";
/// - "security clearance is not required".
///
/// Deliberately narrow. "Candidates without an active clearance will not be
/// considered" is a requirement, and still counts as a mention.
fn says_it_does_not_apply(tokens: &[Token], first: usize, last: usize) -> bool {
    // Words leading up to the mention, nearest first, within the same statement.
    let before: Vec<(usize, &str)> = (0..first)
        .rev()
        .map_while(|i| word_at(tokens, i).map(|word| (i, word)))
        .take(4)
        .collect();
    if before.iter().take(2).any(|(_, word)| *word == "no") {
        return true;
    }
    if let Some((verb, _)) = before.iter().find(|(_, word)| is_requirement(word)) {
        // "not require", and "doesn't require" (which splits into "doesn" + "t").
        if verb.checked_sub(1).and_then(|i| word_at(tokens, i)).is_some_and(|word| matches!(word, "not" | "t")) {
            return true;
        }
    }

    // "... is not required"
    let mut after = last + 1;
    while after <= last + 2 && word_at(tokens, after).is_some_and(|word| matches!(word, "is" | "are" | "was")) {
        after += 1;
    }
    if word_at(tokens, after) == Some("not") && word_at(tokens, after + 1).is_some_and(is_requirement) {
        return true;
    }

    // The mention is part of a label; its value follows the colon, on the same line
    // or the next one.
    let colon = (last + 1..tokens.len())
        .take(9)
        .take_while(|&i| tokens[i] != Token::Break)
        .find(|&i| tokens[i] == Token::Colon);
    let Some(colon) = colon else {
        return false;
    };
    let value = if tokens.get(colon + 1) == Some(&Token::Break) { colon + 2 } else { colon + 1 };
    match word_at(tokens, value) {
        Some("none" | "nil" | "na") => true,
        Some("n") => word_at(tokens, value + 1) == Some("a"),
        Some("not") => word_at(tokens, value + 1).is_some_and(|word| is_requirement(word) || word == "applicable"),
        // A bare "No", not "No lower than Secret".
        Some("no") => !matches!(tokens.get(value + 1), Some(Token::Word(_))),
        _ => false,
    }
}

/// True when the text mentions the phrase: the same words, in the same order, as
/// whole words. Capitals and the punctuation between words are ignored, so "TS/SCI"
/// finds "TS / SCI" and "possess secret" finds "Possess: Secret". A mention that
/// says the thing does not apply is not counted.
fn mentions(text: &[Token], phrase: &str) -> bool {
    let wanted: Vec<String> = tokens(phrase)
        .into_iter()
        .filter_map(|token| match token {
            Token::Word(word) => Some(word),
            _ => None,
        })
        .collect();
    if wanted.is_empty() {
        return false;
    }
    let words: Vec<(usize, &str)> = text
        .iter()
        .enumerate()
        .filter_map(|(i, token)| match token {
            Token::Word(word) => Some((i, word.as_str())),
            _ => None,
        })
        .collect();
    words.windows(wanted.len()).any(|window| {
        window.iter().zip(&wanted).all(|((_, word), want)| word == want)
            && !says_it_does_not_apply(text, window[0].0, window[wanted.len() - 1].0)
    })
}

/// The entry of the "anywhere in the posting" hide list that this posting mentions,
/// if any. The title counts as part of the posting.
pub fn hidden_term(title: &str, description_html: &str, settings: &Settings) -> Option<String> {
    if settings.exclude_terms.is_empty() {
        return None;
    }
    let text = tokens(&format!("{title}\n{}", block_text(description_html)));
    settings.exclude_terms.iter().find(|term| mentions(&text, term)).cloned()
}

/// The entry of either hide list that keeps this posting off the board, if any.
pub fn hidden_by(title: &str, description_html: &str, settings: &Settings) -> Option<String> {
    hidden_title_keyword(title, settings).or_else(|| hidden_term(title, description_html, settings))
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
            ..Settings::default()
        }
    }

    fn hiding(terms: &[&str]) -> Settings {
        Settings {
            exclude_terms: terms.iter().map(|s| s.to_string()).collect(),
            ..Settings::default()
        }
    }

    fn hidden(html: &str, terms: &[&str]) -> bool {
        hidden_term("Systems Engineer", html, &hiding(terms)).is_some()
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
        // The "anywhere" list hides titles too.
        let mut anywhere = settings(&["devops engineer", "developer"], &[], &[]);
        anywhere.exclude_terms = vec!["security clearance".into(), "DevOps".into()];
        assert_eq!(
            conflicting_keywords(&anywhere),
            vec![("devops engineer".to_string(), "DevOps".to_string())]
        );
    }

    #[test]
    fn hidden_phrases_are_found_anywhere_in_the_posting() {
        let terms = ["security clearance", "TS/SCI", "polygraph"];
        for html in [
            "<p>Candidate must be able to obtain + maintain an Interim Secret security clearance prior to a start date.</p>",
            "<ul><li>Active TS / SCI required.</li></ul>",
            "<p>Top Secret (TS-SCI) with a full-scope POLYGRAPH.</p>",
            "<p>Must obtain a Public Trust Security&nbsp;Clearance prior to start date.</p>",
            // A requirement phrased with a negative is still a requirement.
            "<p>Candidates without an active security clearance will not be considered.</p>",
            "<p>If you do not hold a security clearance today, we will sponsor one.</p>",
        ] {
            assert!(hidden(html, &terms), "should be hidden: {html}");
        }
        for html in [
            "<p>We build tools for developers.</p>",
            // Whole words, as written.
            "<p>Experience with secrets management and customs clearances.</p>",
            "<p>Our security team owns clearance of vulnerabilities.</p>",
        ] {
            assert!(!hidden(html, &terms), "should not be hidden: {html}");
        }
        // The title is part of the posting.
        assert!(hidden_term("DevOps Engineer (TS/SCI)", "", &hiding(&terms)).is_some());
        // Nothing is hidden until the user lists something.
        assert!(hidden_term("Cleared Engineer", "<p>security clearance</p>", &Settings::default()).is_none());
        // The entry is reported as the user typed it.
        assert_eq!(hidden_term("Engineer", "<p>Needs a polygraph.</p>", &hiding(&terms)).as_deref(), Some("polygraph"));
    }

    #[test]
    fn a_mention_that_says_it_does_not_apply_is_not_counted() {
        // How three employers on the board lay out a job with no clearance.
        let none_needed = concat!(
            "<p><u><b>Clearance Level Must Currently Possess:</b></u></p>None<p></p><p></p>",
            "<p><u><b>Clearance Level Must Be Able to Obtain:</b></u></p>None<p></p><p></p>",
            "<p><b><u>Public Trust/Other Required:</u></b></p>None<p></p><p><u><b>Job Family:</b></u></p>Software Engineering",
        );
        assert!(!hidden(none_needed, &["clearance", "public trust"]));
        let doubly_escaped = "Remote&amp;#xa;&amp;#xa;<p><b>Security Clearance Type: </b></p>None/Not Required&amp;#xa;&amp;#xa;<p><b>Security Clearance Status: </b></p>Not Required<p></p>";
        assert!(!hidden(doubly_escaped, &["security clearance", "clearance"]));
        for html in [
            "<p>No security clearance required.</p>",
            "<p>No active security clearance is needed for this role.</p>",
            "<p>This role does not require a security clearance.</p>",
            "<p>This role doesn't require a security clearance.</p>",
            "<p>Applicants are not required to hold a security clearance.</p>",
            "<p>A security clearance is not required.</p>",
            "<p>Security clearance: N/A</p>",
            "<p>Security Clearance Required: No</p>",
        ] {
            assert!(!hidden(html, &["security clearance"]), "should not be hidden: {html}");
        }

        // The same layouts with a clearance filled in.
        let obtain_secret = none_needed.replacen("Obtain:</b></u></p>None", "Obtain:</b></span></u></p>Interim Secret", 1);
        assert!(hidden(&obtain_secret, &["clearance"]));
        assert!(hidden(&obtain_secret, &["interim secret"]));
        let background_check = none_needed.replace("Required:</u></b></p>None", "Required:</u></b></p>SSBI (T5)");
        assert!(hidden(&background_check, &["public trust"]));
        assert!(!hidden(&background_check, &["clearance"]));
        let secret = "<p><b>Security Clearance Type: </b></p>DoD Clearance: Secret<p><b>Security Clearance Status: </b></p>Active and existing security clearance required after day 1";
        assert!(hidden(secret, &["security clearance"]));
        // A "No" that answers the label before it says nothing about the next one.
        assert!(hidden("<p>Relocation: No</p><p>Security clearance: Secret</p>", &["security clearance"]));
        assert!(hidden("<p>Security Clearance: No lower than Secret</p>", &["security clearance"]));
        assert!(hidden("<p>Security Clearance Required: Yes</p>", &["security clearance"]));
        // One real requirement is enough, whatever else the posting says.
        assert!(hidden("<p>No security clearance needed to apply.</p><p>You must obtain a security clearance within a year.</p>", &["security clearance"]));
    }

    #[test]
    fn phrases_keep_symbols_that_are_part_of_a_name() {
        assert!(hidden("<p>Five years of C++ and .NET.</p>", &["C++"]));
        assert!(hidden("<p>Five years of C++ and .NET.</p>", &[".net"]));
        assert!(!hidden("<p>Plan C is to grow net revenue.</p>", &["C++", ".NET"]));
        assert!(hidden("<p>Rotating on call duty.</p>", &["on-call"]));
    }

    #[test]
    fn either_hide_list_keeps_a_posting_off_the_board() {
        let s = Settings {
            exclude_keywords: vec!["intern".into()],
            exclude_terms: vec!["security clearance".into()],
            ..Settings::default()
        };
        assert_eq!(hidden_by("AI Intern", "", &s).as_deref(), Some("intern"));
        assert_eq!(hidden_by("AI Engineer", "<p>Secret security clearance.</p>", &s).as_deref(), Some("security clearance"));
        assert_eq!(hidden_by("Internal Tools Engineer", "<p>Remote.</p>", &s), None);
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
