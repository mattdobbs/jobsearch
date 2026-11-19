use serde::{Deserialize, Serialize};

/// The columns every board starts with, as `(id, label)` in display order. They
/// cannot be renamed or removed; the user can add columns of their own beside them.
pub const BUILTIN_COLUMNS: [(&str, &str); 4] = [
    ("new", "New"),
    ("evaluating", "Evaluating"),
    ("applied", "Applied to"),
    ("pass", "Pass"),
];

/// Where new postings land.
pub const NEW_COLUMN: &str = "new";

/// A column added by the user is placed just before this one.
pub const LAST_COLUMN: &str = "pass";

/// The columns for cards the user has not acted on yet. A card in any other column
/// (Applied to, Pass, or one the user added) has been dealt with, and a later posting
/// of the same job is not added to the board again.
pub const UNDECIDED_COLUMNS: [&str; 2] = ["new", "evaluating"];

/// The `reason_kind` of a card that makes no claim about the employer. Only added
/// when the user has turned off the inclusive-employers-only setting.
pub const NO_INCLUSION_CLAIM: &str = "none";

/// How many columns a board can have, the built-in ones included.
pub const MAX_COLUMNS: usize = 12;

/// Longest column name, in characters.
pub const MAX_COLUMN_LABEL: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Column {
    pub id: String,
    pub label: String,
    pub builtin: bool,
}

/// What a source's own workplace field says about a posting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remote {
    /// A structured field on the listing says remote, or the source only lists remote jobs.
    Declared,
    /// No structured flag: the title or location text has to say.
    Unverified,
    /// The employer's own label says hybrid, or offers hybrid beside remote. This
    /// settles it, whatever the title says.
    Hybrid,
    /// The employer's own label says on-site, or something else that is not remote.
    /// This settles it too.
    OnSite,
}

/// Where a job is done, as far as its posting shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workplace {
    Remote,
    Hybrid,
    OnSite,
    /// The posting does not say. It is not shown to be remote, so it is treated as an
    /// on-site job when deciding what to show, but its card does not claim it is one.
    Unstated,
}

impl Workplace {
    /// How the workplace is stored on a card.
    pub fn id(self) -> &'static str {
        match self {
            Workplace::Remote => "remote",
            Workplace::Hybrid => "hybrid",
            Workplace::OnSite => "onsite",
            Workplace::Unstated => "unstated",
        }
    }
}

/// The kinds of job the user wants on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workplaces {
    pub remote: bool,
    pub hybrid: bool,
    /// On-site jobs, and jobs whose posting does not say where the work is done.
    pub onsite: bool,
}

impl Default for Workplaces {
    /// Remote only, which is all the app showed before this was a choice.
    fn default() -> Self {
        Workplaces {
            remote: true,
            hybrid: false,
            onsite: false,
        }
    }
}

impl Workplaces {
    pub fn any(self) -> bool {
        self.remote || self.hybrid || self.onsite
    }

    pub fn wants(self, workplace: Workplace) -> bool {
        match workplace {
            Workplace::Remote => self.remote,
            Workplace::Hybrid => self.hybrid,
            Workplace::OnSite | Workplace::Unstated => self.onsite,
        }
    }

    /// True when a posting whose listing reads as `listed` could still be wanted once
    /// its description has been read. A description can show that a job listed as
    /// remote, or not labelled at all, is in fact hybrid or on-site; it never makes a
    /// job remote, and it does not overrule a listing that says hybrid or on-site.
    pub fn might_want(self, listed: Workplace) -> bool {
        match listed {
            Workplace::Remote => self.any(),
            Workplace::Unstated => self.hybrid || self.onsite,
            Workplace::Hybrid => self.hybrid,
            Workplace::OnSite => self.onsite,
        }
    }
}

/// Why a posting counts as coming from a disability / neurodiversity-inclusive employer.
#[derive(Debug, Clone)]
pub enum Inclusion {
    /// Employer hiring program or a dedicated job board: every remote job qualifies.
    Known {
        kind: &'static str,
        reason: String,
        evidence_url: String,
    },
    /// Qualifies only when the posting's own text says so.
    NeedsSignal,
}

/// A posting as fetched from a source, before any filtering.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Stable source id, e.g. `greenhouse:gitlab` or `remotive`.
    pub source: String,
    /// Human label for the source, e.g. `GitLab careers`.
    pub source_label: String,
    pub external_id: String,
    pub title: String,
    pub company: String,
    pub location: String,
    /// Best link to the posting: the employer's own listing whenever the source has one.
    pub url: String,
    pub description_html: String,
    pub salary: String,
    pub posted_at: Option<String>,
    pub remote: Remote,
    pub inclusion: Inclusion,
    /// True when `url` is the employer's own listing rather than a job board's copy.
    pub direct: bool,
    /// JSON endpoint holding the full description, fetched only for postings that
    /// survive the cheap filters.
    pub detail_url: Option<String>,
}

impl Candidate {
    pub fn key(&self) -> String {
        format!("{}:{}", self.source, self.external_id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: i64,
    pub title: String,
    pub company: String,
    pub location: String,
    pub url: String,
    pub source: String,
    pub source_label: String,
    pub salary: String,
    pub reason: String,
    pub reason_kind: String,
    pub evidence_url: String,
    pub posted_at: Option<String>,
    pub found_at: String,
    pub status: String,
    pub status_changed_at: String,
    pub direct: bool,
    /// A `Workplace` id: "remote", "hybrid", "onsite" or "unstated".
    pub workplace: String,
    pub note_count: i64,
    pub attachment_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobDetail {
    #[serde(flatten)]
    pub job: Job,
    pub description: String,
    pub notes: Vec<Note>,
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Note {
    pub id: i64,
    pub job_id: i64,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Attachment {
    pub id: i64,
    pub job_id: i64,
    pub filename: String,
    #[serde(skip)]
    pub stored_name: String,
    pub size: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// A job title must contain at least one of these (empty = any role).
    pub role_keywords: Vec<String>,
    /// A job title containing any of these is dropped.
    pub exclude_keywords: Vec<String>,
    /// A job whose title or description mentions any of these phrases is dropped.
    pub exclude_terms: Vec<String>,
    /// Places the user can work from. A job with a stated region must match one (empty = anywhere).
    pub regions: Vec<String>,
    /// Which of remote, hybrid and on-site jobs to show.
    pub workplaces: Workplaces,
    /// Places the user can travel to. A hybrid or on-site job must name one (empty =
    /// anywhere in the regions). Remote jobs are not held to this.
    pub nearby: Vec<String>,
    /// True keeps the board to employers shown to welcome disabled and neurodivergent
    /// applicants. False adds every job that matches the settings above.
    pub inclusive_only: bool,
    /// False until the user has saved settings once; drives the first-run prompt.
    pub configured: bool,
}

impl Default for Settings {
    /// Also fills in whatever settings saved by an earlier version leave out, so a
    /// board that never chose stays remote-only and keeps the inclusive-employers rule.
    fn default() -> Self {
        Settings {
            role_keywords: Vec::new(),
            exclude_keywords: Vec::new(),
            exclude_terms: Vec::new(),
            regions: Vec::new(),
            workplaces: Workplaces::default(),
            nearby: Vec::new(),
            inclusive_only: true,
            configured: false,
        }
    }
}

/// A card kept off the board by one of the user's hide lists.
#[derive(Debug, Clone, Serialize)]
pub struct HiddenCard {
    pub id: i64,
    pub title: String,
    pub company: String,
    /// The entry that hid it, as the user typed it.
    pub hidden_by: String,
}

/// Per-source outcome of one scan, shown in the "last check" panel.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceStats {
    pub source: String,
    pub label: String,
    pub fetched: usize,
    pub added: usize,
    pub already_known: usize,
    pub not_remote: usize,
    pub not_inclusive: usize,
    pub filtered_out: usize,
    pub duplicate: usize,
    pub deferred: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub started_at: String,
    pub finished_at: Option<String>,
    pub added: i64,
    pub ok: bool,
    pub sources: Vec<SourceStats>,
}
