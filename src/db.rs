use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use anyhow::Context;
use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::model::{Attachment, Job, JobDetail, Note, Run, STATUSES, Settings, SourceStats};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS jobs (
    id                INTEGER PRIMARY KEY,
    dedupe_key        TEXT NOT NULL UNIQUE,
    fingerprint       TEXT NOT NULL,
    title             TEXT NOT NULL,
    company           TEXT NOT NULL,
    location          TEXT NOT NULL DEFAULT '',
    url               TEXT NOT NULL,
    source            TEXT NOT NULL,
    source_label      TEXT NOT NULL DEFAULT '',
    description       TEXT NOT NULL DEFAULT '',
    salary            TEXT NOT NULL DEFAULT '',
    reason            TEXT NOT NULL DEFAULT '',
    reason_kind       TEXT NOT NULL DEFAULT '',
    evidence_url      TEXT NOT NULL DEFAULT '',
    posted_at         TEXT,
    found_at          TEXT NOT NULL,
    status            TEXT NOT NULL DEFAULT 'new',
    status_changed_at TEXT NOT NULL,
    direct            INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS jobs_fingerprint ON jobs(fingerprint);
CREATE INDEX IF NOT EXISTS jobs_status ON jobs(status);

CREATE TABLE IF NOT EXISTS notes (
    id         INTEGER PRIMARY KEY,
    job_id     INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    body       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS notes_job ON notes(job_id);

CREATE TABLE IF NOT EXISTS attachments (
    id          INTEGER PRIMARY KEY,
    job_id      INTEGER NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    filename    TEXT NOT NULL,
    stored_name TEXT NOT NULL,
    size        INTEGER NOT NULL,
    created_at  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS attachments_job ON attachments(job_id);

-- Postings that were fetched in full and rejected for what they say (not remote,
-- no inclusion language). Remembered so the detail page is not re-downloaded on
-- every scan. Rejections that depend on the user's settings are never stored here.
CREATE TABLE IF NOT EXISTS rejected (
    dedupe_key TEXT PRIMARY KEY,
    reason     TEXT NOT NULL,
    seen_at    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS runs (
    id          INTEGER PRIMARY KEY,
    started_at  TEXT NOT NULL,
    finished_at TEXT,
    added       INTEGER NOT NULL DEFAULT 0,
    ok          INTEGER NOT NULL DEFAULT 0,
    summary     TEXT NOT NULL DEFAULT '[]'
);
";

pub fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// A posting that passed every filter and is ready to become a card.
pub struct NewJob {
    pub dedupe_key: String,
    pub fingerprint: String,
    pub title: String,
    pub company: String,
    pub location: String,
    pub url: String,
    pub source: String,
    pub source_label: String,
    pub description: String,
    pub salary: String,
    pub reason: String,
    pub reason_kind: String,
    pub evidence_url: String,
    pub posted_at: Option<String>,
    pub direct: bool,
}

pub struct Db {
    conn: Mutex<Connection>,
}

const JOB_COLUMNS: &str = "j.id, j.title, j.company, j.location, j.url, j.source, j.source_label,
    j.salary, j.reason, j.reason_kind, j.evidence_url, j.posted_at, j.found_at, j.status,
    j.status_changed_at, j.direct,
    (SELECT COUNT(*) FROM notes n WHERE n.job_id = j.id),
    (SELECT COUNT(*) FROM attachments a WHERE a.job_id = j.id)";

fn job_from_row(row: &Row) -> rusqlite::Result<Job> {
    Ok(Job {
        id: row.get(0)?,
        title: row.get(1)?,
        company: row.get(2)?,
        location: row.get(3)?,
        url: row.get(4)?,
        source: row.get(5)?,
        source_label: row.get(6)?,
        salary: row.get(7)?,
        reason: row.get(8)?,
        reason_kind: row.get(9)?,
        evidence_url: row.get(10)?,
        posted_at: row.get(11)?,
        found_at: row.get(12)?,
        status: row.get(13)?,
        status_changed_at: row.get(14)?,
        direct: row.get(15)?,
        note_count: row.get(16)?,
        attachment_count: row.get(17)?,
    })
}

fn note_from_row(row: &Row) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        job_id: row.get(1)?,
        body: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

fn attachment_from_row(row: &Row) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: row.get(0)?,
        job_id: row.get(1)?,
        filename: row.get(2)?,
        stored_name: row.get(3)?,
        size: row.get(4)?,
        created_at: row.get(5)?,
    })
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("could not open database {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        // A crash mid-scan leaves an unfinished run behind; it must not block the schedule.
        conn.execute("DELETE FROM runs WHERE finished_at IS NULL", [])?;
        Ok(Db {
            conn: Mutex::new(conn),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // ---- settings -------------------------------------------------------

    pub fn settings(&self) -> Settings {
        let raw: Option<String> = self
            .conn()
            .query_row("SELECT value FROM settings WHERE key = 'settings'", [], |r| r.get(0))
            .optional()
            .unwrap_or(None);
        raw.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &Settings) -> anyhow::Result<()> {
        self.conn().execute(
            "INSERT INTO settings(key, value) VALUES ('settings', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![serde_json::to_string(settings)?],
        )?;
        Ok(())
    }

    // ---- scan bookkeeping -----------------------------------------------

    fn string_set(&self, sql: &str) -> anyhow::Result<HashSet<String>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn known_keys(&self) -> anyhow::Result<HashSet<String>> {
        self.string_set("SELECT dedupe_key FROM jobs")
    }

    pub fn fingerprints(&self) -> anyhow::Result<HashSet<String>> {
        self.string_set("SELECT fingerprint FROM jobs")
    }

    pub fn rejected_keys(&self) -> anyhow::Result<HashSet<String>> {
        self.string_set("SELECT dedupe_key FROM rejected")
    }

    pub fn remember_rejected(&self, key: &str, reason: &str) -> anyhow::Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO rejected(dedupe_key, reason, seen_at) VALUES (?1, ?2, ?3)",
            params![key, reason, now()],
        )?;
        Ok(())
    }

    /// Returns false when a card with the same key already exists.
    pub fn insert_job(&self, job: &NewJob) -> anyhow::Result<bool> {
        let ts = now();
        let changed = self.conn().execute(
            "INSERT OR IGNORE INTO jobs (dedupe_key, fingerprint, title, company, location, url,
                source, source_label, description, salary, reason, reason_kind, evidence_url,
                posted_at, found_at, status, status_changed_at, direct)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 'new', ?15, ?16)",
            params![
                job.dedupe_key,
                job.fingerprint,
                job.title,
                job.company,
                job.location,
                job.url,
                job.source,
                job.source_label,
                job.description,
                job.salary,
                job.reason,
                job.reason_kind,
                job.evidence_url,
                job.posted_at,
                ts,
                job.direct,
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn start_run(&self) -> anyhow::Result<i64> {
        let conn = self.conn();
        conn.execute("INSERT INTO runs(started_at) VALUES (?1)", params![now()])?;
        Ok(conn.last_insert_rowid())
    }

    pub fn finish_run(&self, id: i64, added: usize, ok: bool, sources: &[SourceStats]) -> anyhow::Result<()> {
        self.conn().execute(
            "UPDATE runs SET finished_at = ?1, added = ?2, ok = ?3, summary = ?4 WHERE id = ?5",
            params![now(), added as i64, ok, serde_json::to_string(sources)?, id],
        )?;
        // Keep the history short; only the latest run is ever shown.
        self.conn().execute(
            "DELETE FROM runs WHERE id NOT IN (SELECT id FROM runs ORDER BY id DESC LIMIT 60)",
            [],
        )?;
        Ok(())
    }

    pub fn last_run(&self) -> anyhow::Result<Option<Run>> {
        let run = self
            .conn()
            .query_row(
                "SELECT started_at, finished_at, added, ok, summary FROM runs
                 WHERE finished_at IS NOT NULL ORDER BY id DESC LIMIT 1",
                [],
                |r| {
                    let summary: String = r.get(4)?;
                    Ok(Run {
                        started_at: r.get(0)?,
                        finished_at: r.get(1)?,
                        added: r.get(2)?,
                        ok: r.get(3)?,
                        sources: serde_json::from_str(&summary).unwrap_or_default(),
                    })
                },
            )
            .optional()?;
        Ok(run)
    }

    // ---- board ----------------------------------------------------------

    pub fn list_jobs(&self) -> anyhow::Result<Vec<Job>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {JOB_COLUMNS} FROM jobs j ORDER BY j.status_changed_at DESC, j.id DESC"
        ))?;
        let rows = stmt.query_map([], job_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn count_new(&self) -> i64 {
        self.conn()
            .query_row("SELECT COUNT(*) FROM jobs WHERE status = 'new'", [], |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn get_job(&self, id: i64) -> anyhow::Result<Option<JobDetail>> {
        let conn = self.conn();
        let found = conn
            .query_row(
                &format!("SELECT {JOB_COLUMNS}, j.description FROM jobs j WHERE j.id = ?1"),
                params![id],
                |r| Ok((job_from_row(r)?, r.get::<_, String>(18)?)),
            )
            .optional()?;
        let Some((job, description)) = found else {
            return Ok(None);
        };
        let mut stmt = conn.prepare(
            "SELECT id, job_id, body, created_at, updated_at FROM notes
             WHERE job_id = ?1 ORDER BY id DESC",
        )?;
        let notes = stmt.query_map(params![id], note_from_row)?.collect::<Result<_, _>>()?;
        let mut stmt = conn.prepare(
            "SELECT id, job_id, filename, stored_name, size, created_at FROM attachments
             WHERE job_id = ?1 ORDER BY id DESC",
        )?;
        let attachments = stmt
            .query_map(params![id], attachment_from_row)?
            .collect::<Result<_, _>>()?;
        Ok(Some(JobDetail {
            job,
            description,
            notes,
            attachments,
        }))
    }

    pub fn job_exists(&self, id: i64) -> anyhow::Result<bool> {
        Ok(self
            .conn()
            .query_row("SELECT 1 FROM jobs WHERE id = ?1", params![id], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Moves a card to another column. Returns false when the card does not exist.
    pub fn set_status(&self, id: i64, status: &str) -> anyhow::Result<bool> {
        anyhow::ensure!(STATUSES.contains(&status), "unknown column: {status}");
        let changed = self.conn().execute(
            "UPDATE jobs SET status = ?1, status_changed_at = ?2 WHERE id = ?3 AND status <> ?1",
            params![status, now(), id],
        )?;
        Ok(changed > 0 || self.job_exists(id)?)
    }

    // ---- notes ----------------------------------------------------------

    pub fn add_note(&self, job_id: i64, body: &str) -> anyhow::Result<Note> {
        let conn = self.conn();
        let ts = now();
        conn.execute(
            "INSERT INTO notes(job_id, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![job_id, body, ts],
        )?;
        Ok(Note {
            id: conn.last_insert_rowid(),
            job_id,
            body: body.to_string(),
            created_at: ts.clone(),
            updated_at: ts,
        })
    }

    pub fn update_note(&self, id: i64, body: &str) -> anyhow::Result<bool> {
        let changed = self.conn().execute(
            "UPDATE notes SET body = ?1, updated_at = ?2 WHERE id = ?3",
            params![body, now(), id],
        )?;
        Ok(changed > 0)
    }

    pub fn delete_note(&self, id: i64) -> anyhow::Result<bool> {
        Ok(self.conn().execute("DELETE FROM notes WHERE id = ?1", params![id])? > 0)
    }

    // ---- attachments ----------------------------------------------------

    pub fn add_attachment(
        &self,
        job_id: i64,
        filename: &str,
        stored_name: &str,
        size: i64,
    ) -> anyhow::Result<Attachment> {
        let conn = self.conn();
        let ts = now();
        conn.execute(
            "INSERT INTO attachments(job_id, filename, stored_name, size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![job_id, filename, stored_name, size, ts],
        )?;
        Ok(Attachment {
            id: conn.last_insert_rowid(),
            job_id,
            filename: filename.to_string(),
            stored_name: stored_name.to_string(),
            size,
            created_at: ts,
        })
    }

    pub fn get_attachment(&self, id: i64) -> anyhow::Result<Option<Attachment>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, job_id, filename, stored_name, size, created_at FROM attachments WHERE id = ?1",
                params![id],
                attachment_from_row,
            )
            .optional()?)
    }

    pub fn delete_attachment(&self, id: i64) -> anyhow::Result<bool> {
        Ok(self.conn().execute("DELETE FROM attachments WHERE id = ?1", params![id])? > 0)
    }
}
