//! JobSearch: a menu-bar app that looks for remote jobs at disability- and
//! neurodiversity-inclusive employers twice a day and keeps them on a local kanban
//! board.

mod db;
mod desktop;
mod employers;
mod filter;
mod logging;
mod login_item;
mod model;
mod paths;
mod scan;
mod scheduler;
mod server;
mod sources;
mod state;

use std::fs::TryLockError;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use anyhow::Context;

use crate::state::AppState;

pub const APP_NAME: &str = "JobSearch";

const USAGE: &str = "\
JobSearch - remote jobs at disability- and neurodiversity-inclusive employers

Usage: jobsearch [options]

  (no options)   Open the app.
  --background   Start in the menu bar without showing the window (used at login).
  --scan-once    Run one check, print what it found, and exit.
  --serve <port> Development only: no window; serve the board on http://127.0.0.1:<port>/.
  --help         Show this help.
";

enum Mode {
    Desktop { show_window: bool },
    ScanOnce,
    Serve { port: u16 },
}

fn parse_args() -> anyhow::Result<Option<Mode>> {
    let mut mode = Mode::Desktop { show_window: true };
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--background" => mode = Mode::Desktop { show_window: false },
            "--scan-once" => mode = Mode::ScanOnce,
            "--serve" => {
                let value = raw.next().context("--serve needs a port number")?;
                let port = value.parse().with_context(|| format!("{value} is not a valid port"))?;
                mode = Mode::Serve { port };
            }
            "--help" | "-h" => return Ok(None),
            // macOS passes a process serial number to apps launched from Finder.
            other if other.starts_with("-psn_") => {}
            other => anyhow::bail!("unknown option {other}\n\n{USAGE}"),
        }
    }
    Ok(Some(mode))
}

/// Makes sure only one copy of the app works on a data folder at a time. The lock is
/// released by the system when the process ends, however it ends.
fn single_instance(paths: &paths::Paths) -> anyhow::Result<Option<std::fs::File>> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(paths.data_dir.join("jobsearch.lock"))
        .context("could not create the lock file")?;
    match lock.try_lock() {
        Ok(()) => Ok(Some(lock)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e).context("could not lock the data folder"),
    }
}

fn main() -> anyhow::Result<()> {
    let Some(mode) = parse_args()? else {
        print!("{USAGE}");
        return Ok(());
    };
    let paths = paths::Paths::init()?;
    logging::init(&paths.log_file);

    let new_state = |paths: paths::Paths| -> anyhow::Result<Arc<AppState>> {
        Ok(Arc::new(AppState {
            db: db::Db::open(&paths.db_file)?,
            http: sources::http_client()?,
            paths,
            test_mode: desktop::test_mode(),
            scanning: AtomicBool::new(false),
            refresh: tokio::sync::Notify::new(),
            badge: OnceLock::new(),
        }))
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("could not start the async runtime")?;

    match mode {
        Mode::ScanOnce => {
            let state = new_state(paths)?;
            let ok = runtime.block_on(scheduler::scan_once(&state));
            if let Some(run) = state.db.last_run()? {
                println!("{}", serde_json::to_string_pretty(&run)?);
            }
            anyhow::ensure!(ok, "the check failed; see {}", state.paths.log_file.display());
            Ok(())
        }
        Mode::Serve { port } => {
            let listener = std::net::TcpListener::bind(("127.0.0.1", port))
                .with_context(|| format!("could not listen on port {port}"))?;
            listener.set_nonblocking(true)?;
            let state = new_state(paths)?;
            log_info!("serving the board at http://127.0.0.1:{port}/; data in {}", state.paths.data_dir.display());
            runtime.spawn(server::serve(listener, state.clone()));
            runtime.spawn(scheduler::run(state));
            runtime.block_on(async {
                let _ = tokio::signal::ctrl_c().await;
            });
            Ok(())
        }
        Mode::Desktop { show_window } => {
            // Held for the life of the process.
            let Some(_lock) = single_instance(&paths)? else {
                log_info!("JobSearch is already running");
                eprintln!("JobSearch is already running. Use its Dock or menu-bar icon to open the window.");
                return Ok(());
            };
            let state = new_state(paths)?;
            log_info!("JobSearch started; data in {}", state.paths.data_dir.display());
            runtime.spawn(scheduler::run(state.clone()));
            desktop::run(state, runtime.handle().clone(), show_window)
        }
    }
}
