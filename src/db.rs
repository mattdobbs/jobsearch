use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use anyhow::Context;
use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::model::{
    Attachment, BUILTIN_COLUMNS, Column, HiddenCard, Job, JobDetail, LAST_COLUMN, NEW_COLUMN, Note, Run,
    Settings, SourceStats,
};

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
    direct            INTEGER NOT NULL DEFAULT 0,
    -- The hide-list entry keeping this card off the board; NULL when it is shown.
    hidden_by         TEXT
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

-- Postings that were fetched in full and rejected for what they say. Remembered so
-- the detail page is not re-downloaded on every scan. A rejection that depends on
-- the user's settings (region, hidden phrases) is marked, and forgotten as soon as
-- the settings change.
CREATE TABLE IF NOT EXISTS rejected (
    dedupe_key  TEXT PRIMARY KEY,
    reason      TEXT NOT NULL,
    seen_at     TEXT NOT NULL,
    by_settings INTEGER NOT NULL DEFAULT 0
);

-- Board columns, left to right. jobs.status holds a column id.
CREATE TABLE IF NOT EXISTS columns (
    id       TEXT PRIMARY KEY,
    label    TEXT NOT NULL,
    position INTEGER NOT NULL
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

/// Adds a column to a table created by an earlier version of the app.
fn add_column_if_missing(conn: &Connection, table: &str, column: &str, declaration: &str) -> rusqlite::Result<()> {
    let present = conn
        .prepare(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"))?
        .exists(params![column])?;
    if !present {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {declaration}"))?;
    }
    Ok(())
}

fn column_ids(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM columns ORDER BY position, id")?;
    let ids = stmt.query_map([], |r| r.get(0))?.collect();
    ids
}

/// Renumbers the columns to match `order`.
fn save_column_order(conn: &Connection, order: &[String]) -> rusqlite::Result<()> {
    for (position, id) in order.iter().enumerate() {
        conn.execute("UPDATE columns SET position = ?1 WHERE id = ?2", params![position as i64, id])?;
    }
    Ok(())
}

fn is_builtin(id: &str) -> bool {
    BUILTIN_COLUMNS.iter().any(|(builtin, _)| *builtin == id)
}

/// What happened to a request to change a column.
#[derive(Debug, PartialEq, Eq)]
pub enum ColumnChange {
    Done,
    NotFound,
    /// The built-in columns cannot be renamed, moved or removed.
    Builtin,
    /// Another column already has that name.
    NameTaken,
    /// It is already as far that way as it can go.
    AtEdge,
}

/// Which way to move a column on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("could not open database {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::prepare(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(conn: Connection) -> anyhow::Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        add_column_if_missing(&conn, "jobs", "hidden_by", "TEXT")?;
        add_column_if_missing(&conn, "rejected", "by_settings", "INTEGER NOT NULL DEFAULT 0")?;
        for (position, (id, label)) in BUILTIN_COLUMNS.iter().enumerate() {
            conn.execute(
                "INSERT OR IGNORE INTO columns(id, label, position) VALUES (?1, ?2, ?3)",
                params![id, label, position as i64],
            )?;
        }
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

    /// `by_settings` marks a rejection that the user's settings caused, which stops
    /// applying when they change them.
    pub fn remember_rejected(&self, key: &str, reason: &str, by_settings: bool) -> anyhow::Result<()> {
        self.conn().execute(
            "INSERT OR REPLACE INTO rejected(dedupe_key, reason, seen_at, by_settings) VALUES (?1, ?2, ?3, ?4)",
            params![key, reason, now(), by_settings],
        )?;
        Ok(())
    }

    /// Lets postings that the old settings left out be judged again.
    pub fn forget_settings_rejections(&self) -> anyhow::Result<()> {
        self.conn().execute("DELETE FROM rejected WHERE by_settings <> 0", [])?;
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
            "SELECT {JOB_COLUMNS} FROM jobs j WHERE j.hidden_by IS NULL
             ORDER BY j.status_changed_at DESC, j.id DESC"
        ))?;
        let rows = stmt.query_map([], job_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn count_new(&self) -> i64 {
        self.conn()
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE status = ?1 AND hidden_by IS NULL",
                params![NEW_COLUMN],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    // ---- hide lists -----------------------------------------------------

    /// Applies the user's hide lists to the cards already on the board. `judge` is
    /// given a card's title and description and returns the entry that hides it.
    ///
    /// Only cards the user has not touched are hidden: still in New, with no notes
    /// and no attachments. Nothing is deleted, so taking an entry off a list brings
    /// its cards back. Returns how many cards are now hidden.
    pub fn apply_hide_lists(&self, judge: impl Fn(&str, &str) -> Option<String>) -> anyhow::Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let hidden: Vec<(i64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT j.id, j.title, j.description FROM jobs j
                 WHERE j.status = ?1
                   AND NOT EXISTS (SELECT 1 FROM notes n WHERE n.job_id = j.id)
                   AND NOT EXISTS (SELECT 1 FROM attachments a WHERE a.job_id = j.id)",
            )?;
            let rows = stmt.query_map(params![NEW_COLUMN], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })?;
            let mut hidden = Vec::new();
            for row in rows {
                let (id, title, description) = row?;
                if let Some(entry) = judge(&title, &description) {
                    hidden.push((id, entry));
                }
            }
            hidden
        };
        tx.execute("UPDATE jobs SET hidden_by = NULL WHERE hidden_by IS NOT NULL", [])?;
        for (id, entry) in &hidden {
            tx.execute("UPDATE jobs SET hidden_by = ?1 WHERE id = ?2", params![entry, id])?;
        }
        tx.commit()?;
        Ok(hidden.len())
    }

    pub fn hidden_cards(&self) -> anyhow::Result<Vec<HiddenCard>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, title, company, hidden_by FROM jobs WHERE hidden_by IS NOT NULL
             ORDER BY hidden_by COLLATE NOCASE, company COLLATE NOCASE, title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(HiddenCard {
                id: r.get(0)?,
                title: r.get(1)?,
                company: r.get(2)?,
                hidden_by: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // ---- columns --------------------------------------------------------

    pub fn columns(&self) -> anyhow::Result<Vec<Column>> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT id, label FROM columns ORDER BY position, id")?;
        let rows = stmt.query_map([], |r| {
            let id: String = r.get(0)?;
            Ok(Column {
                builtin: is_builtin(&id),
                id,
                label: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Adds a column just before Pass, so Pass stays the last stop. Returns `None`
    /// when a column of that name already exists.
    pub fn add_column(&self, label: &str) -> anyhow::Result<Option<Column>> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let taken = tx
            .prepare("SELECT 1 FROM columns WHERE label = ?1 COLLATE NOCASE")?
            .exists(params![label])?;
        if taken {
            return Ok(None);
        }
        let id = format!("c{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
        let mut order = column_ids(&tx)?;
        let at = order.iter().position(|existing| existing == LAST_COLUMN).unwrap_or(order.len());
        order.insert(at, id.clone());
        tx.execute("INSERT INTO columns(id, label, position) VALUES (?1, ?2, 0)", params![id, label])?;
        save_column_order(&tx, &order)?;
        tx.commit()?;
        Ok(Some(Column {
            id,
            label: label.to_string(),
            builtin: false,
        }))
    }

    pub fn rename_column(&self, id: &str, label: &str) -> anyhow::Result<ColumnChange> {
        if is_builtin(id) {
            return Ok(ColumnChange::Builtin);
        }
        let conn = self.conn();
        let taken = conn
            .prepare("SELECT 1 FROM columns WHERE label = ?1 COLLATE NOCASE AND id <> ?2")?
            .exists(params![label, id])?;
        if taken {
            return Ok(ColumnChange::NameTaken);
        }
        let changed = conn.execute("UPDATE columns SET label = ?1 WHERE id = ?2", params![label, id])?;
        Ok(if changed > 0 { ColumnChange::Done } else { ColumnChange::NotFound })
    }

    /// Moves a column one place. New always stays first.
    pub fn move_column(&self, id: &str, direction: Direction) -> anyhow::Result<ColumnChange> {
        if is_builtin(id) {
            return Ok(ColumnChange::Builtin);
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut order = column_ids(&tx)?;
        let Some(at) = order.iter().position(|existing| existing == id) else {
            return Ok(ColumnChange::NotFound);
        };
        let target = match direction {
            Direction::Left if at > 0 && order[at - 1] != NEW_COLUMN => at - 1,
            Direction::Right if at + 1 < order.len() => at + 1,
            _ => return Ok(ColumnChange::AtEdge),
        };
        order.swap(at, target);
        save_column_order(&tx, &order)?;
        tx.commit()?;
        Ok(ColumnChange::Done)
    }

    /// Removes a column the user added. Its cards move to the column on its left.
    /// Returns that column's id and how many cards moved.
    pub fn delete_column(&self, id: &str) -> anyhow::Result<Result<(String, usize), ColumnChange>> {
        if is_builtin(id) {
            return Ok(Err(ColumnChange::Builtin));
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut order = column_ids(&tx)?;
        let Some(at) = order.iter().position(|existing| existing == id) else {
            return Ok(Err(ColumnChange::NotFound));
        };
        // New is always first, so a column the user added always has a neighbour.
        let destination = order[at.saturating_sub(1)].clone();
        let moved = tx.execute(
            "UPDATE jobs SET status = ?1, status_changed_at = ?2 WHERE status = ?3",
            params![destination, now(), id],
        )?;
        tx.execute("DELETE FROM columns WHERE id = ?1", params![id])?;
        order.remove(at);
        save_column_order(&tx, &order)?;
        tx.commit()?;
        Ok(Ok((destination, moved)))
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

    pub fn column_exists(&self, id: &str) -> anyhow::Result<bool> {
        Ok(self.conn().prepare("SELECT 1 FROM columns WHERE id = ?1")?.exists(params![id])?)
    }

    /// Moves a card to another column. Returns false when the card does not exist.
    pub fn set_status(&self, id: i64, status: &str) -> anyhow::Result<bool> {
        anyhow::ensure!(self.column_exists(status)?, "unknown column: {status}");
        // A card the user moves is one they want to see, whatever the hide lists say.
        let changed = self.conn().execute(
            "UPDATE jobs SET status = ?1, status_changed_at = ?2, hidden_by = NULL
             WHERE id = ?3 AND status <> ?1",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn job(key: &str, title: &str, description: &str) -> NewJob {
        NewJob {
            dedupe_key: key.to_string(),
            fingerprint: key.to_string(),
            title: title.to_string(),
            company: "Acme".to_string(),
            location: "Remote".to_string(),
            url: "https://example.com/job".to_string(),
            source: "test".to_string(),
            source_label: "Test".to_string(),
            description: description.to_string(),
            salary: String::new(),
            reason: String::new(),
            reason_kind: "program".to_string(),
            evidence_url: String::new(),
            posted_at: None,
            direct: true,
        }
    }

    fn labels(db: &Db) -> Vec<String> {
        db.columns().unwrap().into_iter().map(|column| column.label).collect()
    }

    fn id_of(db: &Db, title: &str) -> i64 {
        db.conn()
            .query_row("SELECT id FROM jobs WHERE title = ?1", params![title], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn a_board_starts_with_the_built_in_columns() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(labels(&db), ["New", "Evaluating", "Applied to", "Pass"]);
        assert!(db.columns().unwrap().iter().all(|column| column.builtin));
    }

    #[test]
    fn added_columns_go_before_pass_and_can_be_moved() {
        let db = Db::open_in_memory().unwrap();
        let interview = db.add_column("Interviewing").unwrap().expect("added");
        let offer = db.add_column("Offer").unwrap().expect("added");
        assert!(!interview.builtin);
        assert_eq!(labels(&db), ["New", "Evaluating", "Applied to", "Interviewing", "Offer", "Pass"]);
        // Names are unique, whatever the capitals.
        assert!(db.add_column("interviewing").unwrap().is_none());
        assert!(db.add_column("pass").unwrap().is_none());

        assert_eq!(db.move_column(&offer.id, Direction::Left).unwrap(), ColumnChange::Done);
        assert_eq!(labels(&db), ["New", "Evaluating", "Applied to", "Offer", "Interviewing", "Pass"]);
        assert_eq!(db.move_column(&interview.id, Direction::Right).unwrap(), ColumnChange::Done);
        assert_eq!(db.move_column(&interview.id, Direction::Right).unwrap(), ColumnChange::AtEdge);
        assert_eq!(labels(&db), ["New", "Evaluating", "Applied to", "Offer", "Pass", "Interviewing"]);
        // Nothing moves in front of New.
        for _ in 0..2 {
            assert_eq!(db.move_column(&offer.id, Direction::Left).unwrap(), ColumnChange::Done);
        }
        assert_eq!(db.move_column(&offer.id, Direction::Left).unwrap(), ColumnChange::AtEdge);
        assert_eq!(labels(&db), ["New", "Offer", "Evaluating", "Applied to", "Pass", "Interviewing"]);

        assert_eq!(db.rename_column(&offer.id, "Offers").unwrap(), ColumnChange::Done);
        assert_eq!(db.rename_column(&offer.id, "OFFERS").unwrap(), ColumnChange::Done);
        assert_eq!(db.rename_column(&offer.id, "Evaluating").unwrap(), ColumnChange::NameTaken);
        assert_eq!(db.rename_column("nope", "Anything").unwrap(), ColumnChange::NotFound);
        assert_eq!(db.move_column("nope", Direction::Left).unwrap(), ColumnChange::NotFound);
    }

    #[test]
    fn built_in_columns_cannot_be_changed() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.rename_column("pass", "Rejected").unwrap(), ColumnChange::Builtin);
        assert_eq!(db.move_column("evaluating", Direction::Right).unwrap(), ColumnChange::Builtin);
        assert_eq!(db.delete_column("new").unwrap(), Err(ColumnChange::Builtin));
        assert_eq!(labels(&db), ["New", "Evaluating", "Applied to", "Pass"]);
    }

    #[test]
    fn cards_can_move_to_an_added_column_and_survive_its_removal() {
        let db = Db::open_in_memory().unwrap();
        db.insert_job(&job("a", "Engineer", "")).unwrap();
        let id = id_of(&db, "Engineer");
        assert!(db.set_status(id, "nowhere").is_err());

        let interview = db.add_column("Interviewing").unwrap().expect("added");
        assert!(db.set_status(id, &interview.id).unwrap());
        assert_eq!(db.list_jobs().unwrap()[0].status, interview.id);

        // Its cards go to the column on its left.
        assert_eq!(db.delete_column(&interview.id).unwrap(), Ok(("applied".to_string(), 1)));
        assert_eq!(db.list_jobs().unwrap()[0].status, "applied");
        assert_eq!(labels(&db), ["New", "Evaluating", "Applied to", "Pass"]);
        assert_eq!(db.delete_column(&interview.id).unwrap(), Err(ColumnChange::NotFound));
    }

    #[test]
    fn hide_lists_only_hide_cards_the_user_has_not_touched() {
        let db = Db::open_in_memory().unwrap();
        for (key, title) in [("a", "Cleared Engineer"), ("b", "Cleared Analyst"), ("c", "Cleared Admin"), ("d", "Web Developer")] {
            db.insert_job(&job(key, title, "<p>text</p>")).unwrap();
        }
        // One has a note, one has been moved.
        db.add_note(id_of(&db, "Cleared Analyst"), "call them").unwrap();
        db.set_status(id_of(&db, "Cleared Admin"), "evaluating").unwrap();
        assert_eq!(db.count_new(), 3);

        let cleared = |title: &str, _description: &str| title.contains("Cleared").then(|| "cleared".to_string());
        assert_eq!(db.apply_hide_lists(cleared).unwrap(), 1);
        let shown: Vec<String> = db.list_jobs().unwrap().into_iter().map(|j| j.title).collect();
        assert_eq!(shown.len(), 3);
        assert!(!shown.contains(&"Cleared Engineer".to_string()));
        assert_eq!(db.count_new(), 2);
        let hidden = db.hidden_cards().unwrap();
        assert_eq!(hidden.len(), 1);
        assert_eq!((hidden[0].title.as_str(), hidden[0].hidden_by.as_str()), ("Cleared Engineer", "cleared"));
        // A hidden card is still known, so a later check does not add it again.
        assert!(db.known_keys().unwrap().contains("a"));

        // Emptying the list brings the card back.
        assert_eq!(db.apply_hide_lists(|_, _| None).unwrap(), 0);
        assert_eq!(db.list_jobs().unwrap().len(), 4);
        assert!(db.hidden_cards().unwrap().is_empty());
    }

    #[test]
    fn rejections_caused_by_settings_are_forgotten_when_settings_change() {
        let db = Db::open_in_memory().unwrap();
        db.remember_rejected("hybrid", "description says hybrid", false).unwrap();
        db.remember_rejected("cleared", "mentions a hidden phrase", true).unwrap();
        assert_eq!(db.rejected_keys().unwrap().len(), 2);
        db.forget_settings_rejections().unwrap();
        let left = db.rejected_keys().unwrap();
        assert!(left.contains("hybrid") && left.len() == 1);
    }

    #[test]
    fn a_database_from_an_earlier_version_is_brought_up_to_date() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE jobs (
                 id INTEGER PRIMARY KEY, dedupe_key TEXT NOT NULL UNIQUE, fingerprint TEXT NOT NULL,
                 title TEXT NOT NULL, company TEXT NOT NULL, location TEXT NOT NULL DEFAULT '',
                 url TEXT NOT NULL, source TEXT NOT NULL, source_label TEXT NOT NULL DEFAULT '',
                 description TEXT NOT NULL DEFAULT '', salary TEXT NOT NULL DEFAULT '',
                 reason TEXT NOT NULL DEFAULT '', reason_kind TEXT NOT NULL DEFAULT '',
                 evidence_url TEXT NOT NULL DEFAULT '', posted_at TEXT, found_at TEXT NOT NULL,
                 status TEXT NOT NULL DEFAULT 'new', status_changed_at TEXT NOT NULL,
                 direct INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE rejected (dedupe_key TEXT PRIMARY KEY, reason TEXT NOT NULL, seen_at TEXT NOT NULL);
             INSERT INTO jobs (dedupe_key, fingerprint, title, company, url, source, found_at, status, status_changed_at)
                 VALUES ('old', 'old', 'Old Card', 'Acme', 'https://example.com', 'test', '2026-11-01T00:00:00Z', 'pass', '2026-11-01T00:00:00Z');
             INSERT INTO rejected VALUES ('gone', 'not remote', '2026-11-01T00:00:00Z');",
        )
        .unwrap();
        let db = Db::prepare(conn).unwrap();
        let jobs = db.list_jobs().unwrap();
        assert_eq!((jobs[0].title.as_str(), jobs[0].status.as_str()), ("Old Card", "pass"));
        assert_eq!(labels(&db), ["New", "Evaluating", "Applied to", "Pass"]);
        assert!(db.rejected_keys().unwrap().contains("gone"));
        db.forget_settings_rejections().unwrap();
        assert!(db.rejected_keys().unwrap().contains("gone"));
        // Opening it a second time changes nothing.
        db.apply_hide_lists(|_, _| None).unwrap();
        assert_eq!(db.list_jobs().unwrap().len(), 1);
    }
}
