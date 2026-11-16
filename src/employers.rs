//! The list of employers whose career sites are checked directly.
//!
//! The list lives in `employers.toml` in the data folder so it can be edited without
//! rebuilding the app. A default copy is compiled in and written out on first launch.

use serde::Deserialize;

use crate::filter::normalize_company;
use crate::paths::Paths;
use crate::{log_info, log_warn};

const DEFAULT_EMPLOYERS: &str = include_str!("../data/employers.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// The employer runs a hiring program: every remote job qualifies.
    Program,
    /// A job qualifies only when its own text says so.
    Signal,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Employer {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub mode: Mode,
    /// For program mode, what the claim rests on: "program" (a neurodiversity hiring
    /// program, the default) or "index" (a disability-inclusion index score).
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub ats: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub tenant: String,
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub public_base: String,
}

#[derive(Debug, Deserialize)]
struct EmployersFile {
    #[serde(default)]
    employer: Vec<Employer>,
}

impl Employer {
    /// The label a card from this employer carries.
    pub fn inclusion_kind(&self) -> &'static str {
        if self.kind == "index" { "index" } else { "program" }
    }

    /// True when `company` (as written on a job board) is this employer.
    pub fn is_named(&self, company: &str) -> bool {
        let wanted = normalize_company(company);
        !wanted.is_empty()
            && std::iter::once(&self.name)
                .chain(self.aliases.iter())
                .any(|name| normalize_company(name) == wanted)
    }
}

fn parse(text: &str) -> anyhow::Result<Vec<Employer>> {
    let file: EmployersFile = toml::from_str(text)?;
    Ok(file.employer)
}

/// Loads the user's employer list, creating it from the built-in default on first use.
/// A file that fails to parse falls back to the default so a typo cannot stop scans.
///
/// Alongside the list the app keeps a copy of the built-in default it was created
/// from. That is how an update can tell an untouched list (replaced with the newer
/// built-in one) from a list the user has edited (left alone).
pub fn load(paths: &Paths) -> Vec<Employer> {
    let path = &paths.employers_file;
    let origin_path = paths.data_dir.join(".employers-default.toml");
    let current = std::fs::read_to_string(path).ok();
    let origin = std::fs::read_to_string(&origin_path).ok();

    let replace = match (&current, &origin) {
        (None, _) => true,
        (Some(current), Some(origin)) => current == origin && origin != DEFAULT_EMPLOYERS,
        (Some(_), None) => false,
    };
    if replace {
        match std::fs::write(path, DEFAULT_EMPLOYERS).and_then(|()| std::fs::write(&origin_path, DEFAULT_EMPLOYERS)) {
            Ok(()) => log_info!("wrote the built-in employer list to {}", path.display()),
            Err(e) => log_warn!("could not write {}: {e}", path.display()),
        }
    } else if origin.is_none() && current.as_deref() == Some(DEFAULT_EMPLOYERS) {
        let _ = std::fs::write(&origin_path, DEFAULT_EMPLOYERS);
    }

    match std::fs::read_to_string(path).map_err(anyhow::Error::from).and_then(|t| parse(&t)) {
        Ok(list) => list,
        Err(e) => {
            log_warn!("{} is not usable ({e}); using the built-in list", path.display());
            parse(DEFAULT_EMPLOYERS).unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_list_parses_and_is_complete() {
        let list = parse(DEFAULT_EMPLOYERS).expect("built-in employers.toml parses");
        assert!(list.len() > 10);
        for e in &list {
            assert!(!e.name.is_empty());
            assert!(matches!(e.kind.as_str(), "" | "program" | "index"), "{} has unknown kind {}", e.name, e.kind);
            if e.mode == Mode::Program {
                assert!(!e.reason.is_empty(), "{} needs a reason", e.name);
                assert!(e.evidence.starts_with("https://"), "{} needs evidence", e.name);
            } else {
                assert!(e.kind.is_empty(), "{} is in signal mode and makes no claim", e.name);
            }
            match e.ats.as_str() {
                "" => {}
                "greenhouse" | "lever" | "ashby" => assert!(!e.token.is_empty(), "{} needs a token", e.name),
                "workday" => assert!(
                    !e.host.is_empty() && !e.tenant.is_empty() && !e.site.is_empty(),
                    "{} needs host, tenant and site",
                    e.name
                ),
                other => panic!("{} has unknown ats {other}", e.name),
            }
        }
    }

    #[test]
    fn no_employer_is_listed_twice() {
        let list = parse(DEFAULT_EMPLOYERS).unwrap();
        let mut names = std::collections::HashSet::new();
        let mut sites = std::collections::HashSet::new();
        for e in &list {
            assert!(names.insert(normalize_company(&e.name)), "{} is listed twice", e.name);
            if !e.ats.is_empty() {
                let site = format!("{}|{}|{}|{}|{}", e.ats, e.token, e.host, e.tenant, e.site);
                assert!(sites.insert(site), "{}'s career site is listed twice", e.name);
            }
        }
    }

    #[test]
    fn an_untouched_list_follows_app_updates_and_an_edited_one_is_kept() {
        let dir = std::env::temp_dir().join(format!("jobsearch-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = Paths {
            db_file: dir.join("jobs.sqlite3"),
            attachments_dir: dir.join("attachments"),
            employers_file: dir.join("employers.toml"),
            log_file: dir.join("jobsearch.log"),
            data_dir: dir.clone(),
        };
        let origin = dir.join(".employers-default.toml");

        // First launch: the built-in list is written out.
        assert!(!load(&paths).is_empty());
        assert_eq!(std::fs::read_to_string(&paths.employers_file).unwrap(), DEFAULT_EMPLOYERS);

        // An older, untouched list is replaced by the newer built-in one.
        let older = "[[employer]]\nname = 'Old Co'\nmode = 'signal'\n";
        std::fs::write(&paths.employers_file, older).unwrap();
        std::fs::write(&origin, older).unwrap();
        assert!(load(&paths).len() > 1);
        assert_eq!(std::fs::read_to_string(&paths.employers_file).unwrap(), DEFAULT_EMPLOYERS);

        // A list the user edited is theirs.
        let edited = "[[employer]]\nname = 'My Pick'\nmode = 'signal'\n";
        std::fs::write(&paths.employers_file, edited).unwrap();
        std::fs::write(&origin, older).unwrap();
        let list = load(&paths);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "My Pick");
        assert_eq!(std::fs::read_to_string(&paths.employers_file).unwrap(), edited);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn employers_are_matched_by_name_or_alias() {
        let list = parse(DEFAULT_EMPLOYERS).unwrap();
        let find = |company: &str| list.iter().find(|e| e.is_named(company)).map(|e| e.name.as_str());
        assert_eq!(find("Salesforce, Inc."), Some("Salesforce"));
        assert_eq!(find("JPMorganChase"), Some("JPMorgan Chase"));
        assert_eq!(find("Ernst & Young"), Some("EY"));
        assert_eq!(find("Some Other Company"), None);
        assert_eq!(find(""), None);
    }
}
