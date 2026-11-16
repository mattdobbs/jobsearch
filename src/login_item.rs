//! "Start at Login", implemented as a per-user LaunchAgent. Nothing is installed
//! unless the user turns the option on, and turning it off removes the file.

use std::path::PathBuf;

use anyhow::Context;

const LABEL: &str = "com.jobsearch.board";

fn plist_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
}

pub fn is_enabled() -> bool {
    plist_path().is_some_and(|path| path.exists())
}

fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn set_enabled(enabled: bool) -> anyhow::Result<()> {
    let path = plist_path().context("could not locate the home folder")?;
    if !enabled {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("could not remove {}", path.display())),
        };
    }
    let program = std::env::current_exe().context("could not locate the app itself")?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>--background</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
        xml_escape(&program.display().to_string())
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, plist).with_context(|| format!("could not write {}", path.display()))
}
