//! Browser profiles: discovery and the command-line flags that select one.
//! Supported: Chromium-family browsers (Chrome, Chromium, Brave, Edge, Vivaldi) and Firefox.
use crate::config::config_dir;
use std::{env, fs, path::PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// What gets stored in the config and passed to the browser: the profile directory for
    /// Chromium-family browsers (`Default`, `Profile 1`), the profile name for Firefox.
    pub id: String,
    /// What the user sees (`Work`, `Perso`).
    pub name: String,
}

enum Kind {
    Chromium(PathBuf),
    Firefox(Vec<PathBuf>),
}

fn kind(browser: &str) -> Option<Kind> {
    let id = browser.strip_suffix(".desktop").unwrap_or(browser);
    let cfg = config_dir();
    let chromium = |dir: &str| Some(Kind::Chromium(cfg.join(dir)));
    match id {
        "google-chrome" | "google-chrome-stable" => chromium("google-chrome"),
        "google-chrome-beta" => chromium("google-chrome-beta"),
        "google-chrome-unstable" => chromium("google-chrome-unstable"),
        "chromium" | "chromium-browser" => chromium("chromium"),
        "brave" | "brave-browser" => chromium("BraveSoftware/Brave-Browser"),
        "microsoft-edge" | "microsoft-edge-stable" => chromium("microsoft-edge"),
        "vivaldi" | "vivaldi-stable" => chromium("vivaldi"),
        "firefox" | "firefox-esr" => {
            let home = PathBuf::from(env::var_os("HOME").unwrap_or_default());
            Some(Kind::Firefox(vec![home.join(".mozilla/firefox"), cfg.join("mozilla/firefox")]))
        }
        _ => None,
    }
}

/// Profiles named in Chromium's `Local State` (`profile.info_cache`).
fn chromium_profiles(user_data: &std::path::Path) -> Vec<Profile> {
    let Ok(text) = fs::read_to_string(user_data.join("Local State")) else { return vec![] };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return vec![] };
    let Some(cache) = json.pointer("/profile/info_cache").and_then(|v| v.as_object()) else { return vec![] };
    cache
        .iter()
        .map(|(id, v)| Profile { id: id.clone(), name: v.get("name").and_then(|n| n.as_str()).unwrap_or(id).to_owned() })
        .collect()
}

/// `Name=` of every `[ProfileN]` section in the first `profiles.ini` found.
fn firefox_profiles(dirs: &[PathBuf]) -> Vec<Profile> {
    let Some(text) = dirs.iter().find_map(|d| fs::read_to_string(d.join("profiles.ini")).ok()) else { return vec![] };
    let mut out = Vec::new();
    let mut in_profile = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_profile = line.starts_with("[Profile");
        } else if let (true, Some(name)) = (in_profile, line.strip_prefix("Name=")) {
            out.push(Profile { id: name.to_owned(), name: name.to_owned() });
        }
    }
    out
}

/// The profiles of a browser (desktop-entry ID); empty when it has none we can discover.
pub fn list(browser: &str) -> Vec<Profile> {
    match kind(browser) {
        Some(Kind::Chromium(dir)) => chromium_profiles(&dir),
        Some(Kind::Firefox(dirs)) => firefox_profiles(&dirs),
        None => vec![],
    }
}

/// Find the profile the user means: by ID or (case-insensitively) by name.
pub fn find(browser: &str, wanted: &str) -> Result<Profile, String> {
    if kind(browser).is_none() {
        return Err(format!("profiles are not supported for '{browser}'"));
    }
    let all = list(browser);
    let w = wanted.to_lowercase();
    all.iter()
        .find(|p| p.id == wanted)
        .or_else(|| all.iter().find(|p| p.name.to_lowercase() == w || p.id.to_lowercase() == w))
        .cloned()
        .ok_or_else(|| {
            let names: Vec<_> = all.iter().map(|p| format!("'{}'", p.name)).collect();
            format!("profile '{wanted}' not found for '{browser}' (available: {})", names.join(", "))
        })
}

/// The extra arguments that open `profile` (an ID from `find`). Inserted before the URL.
pub fn args(browser: &str, profile: &Profile) -> Vec<String> {
    match kind(browser) {
        Some(Kind::Chromium(_)) => vec![format!("--profile-directory={}", profile.id)],
        Some(Kind::Firefox(_)) => vec!["-P".into(), profile.id.clone()],
        None => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDir, with_home};

    fn write(path: PathBuf, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    const CHROME: &str = r#"{"profile":{"info_cache":{"Default":{"name":"Work"},"Profile 1":{"name":"Perso"},"Profile 2":{}}}}"#;

    #[test]
    fn chromium_profiles_come_from_local_state() {
        let t = TempDir::new("prof-chrome");
        with_home(t.path(), || {
            write(config_dir().join("google-chrome/Local State"), CHROME);
            let p = list("google-chrome.desktop");
            let got: Vec<_> = p.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect();
            assert_eq!(got, [("Default", "Work"), ("Profile 1", "Perso"), ("Profile 2", "Profile 2")]);
        });
    }

    #[test]
    fn firefox_profiles_come_from_profiles_ini() {
        let t = TempDir::new("prof-ff");
        with_home(t.path(), || {
            write(
                t.path().join(".mozilla/firefox/profiles.ini"),
                "[Install4F96]\nDefault=x\n\n[Profile1]\nName=work\nPath=a\n\n[Profile0]\nName=default\nPath=b\n\n[General]\nStartWithLastProfile=1\n",
            );
            let names: Vec<_> = list("firefox").into_iter().map(|p| p.name).collect();
            assert_eq!(names, ["work", "default"]);
        });
    }

    #[test]
    fn find_accepts_id_or_name_in_any_case() {
        let t = TempDir::new("prof-find");
        with_home(t.path(), || {
            write(config_dir().join("google-chrome/Local State"), CHROME);
            assert_eq!(find("google-chrome", "Profile 1").unwrap().name, "Perso");
            assert_eq!(find("google-chrome.desktop", "perso").unwrap().id, "Profile 1");
            let e = find("google-chrome", "nope").unwrap_err();
            assert!(e.contains("'Work', 'Perso'"), "{e}");
        });
    }

    #[test]
    fn unsupported_browser_is_reported() {
        assert!(find("epiphany", "x").unwrap_err().contains("not supported"));
        assert!(list("epiphany").is_empty());
    }

    #[test]
    fn args_select_the_profile_by_family() {
        let p = Profile { id: "Profile 1".into(), name: "Perso".into() };
        assert_eq!(args("google-chrome.desktop", &p), ["--profile-directory=Profile 1"]);
        assert_eq!(args("firefox", &p), ["-P", "Profile 1"]);
    }
}
