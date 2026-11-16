use std::path::PathBuf;

use anyhow::Context;

/// Where the app keeps its data. Defaults to `~/Library/Application Support/JobSearch`;
/// `JOBSEARCH_DATA_DIR` overrides it (used for testing against a scratch directory).
#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub db_file: PathBuf,
    pub attachments_dir: PathBuf,
    pub employers_file: PathBuf,
    pub log_file: PathBuf,
}

impl Paths {
    pub fn init() -> anyhow::Result<Self> {
        let data_dir = match std::env::var_os("JOBSEARCH_DATA_DIR") {
            Some(dir) => PathBuf::from(dir),
            None => dirs::data_dir()
                .context("could not locate the Application Support folder")?
                .join(crate::APP_NAME),
        };
        let paths = Paths {
            db_file: data_dir.join("jobs.sqlite3"),
            attachments_dir: data_dir.join("attachments"),
            employers_file: data_dir.join("employers.toml"),
            log_file: data_dir.join("jobsearch.log"),
            data_dir,
        };
        std::fs::create_dir_all(&paths.attachments_dir)
            .with_context(|| format!("could not create {}", paths.attachments_dir.display()))?;
        Ok(paths)
    }
}
