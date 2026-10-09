use serde::{Deserialize, Serialize};
use std::{env, fs, io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf};

#[derive(Default, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Config {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
    #[serde(default, rename = "rule", skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Rule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    pub browser: String,
}

fn dir(var: &str, fallback: &str) -> PathBuf {
    match env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(fallback),
    }
}

pub fn config_dir() -> PathBuf {
    dir("XDG_CONFIG_HOME", ".config")
}

pub fn state_dir() -> PathBuf {
    dir("XDG_STATE_HOME", ".local/state")
}

pub fn data_dir() -> PathBuf {
    dir("XDG_DATA_HOME", ".local/share")
}

pub fn config_path() -> PathBuf {
    config_dir().join("omaroute/config.toml")
}

/// True when `host` is `domain` or a subdomain of it.
fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|p| p.ends_with('.'))
}

impl Config {
    pub fn load() -> Result<Config, String> {
        match fs::read_to_string(config_path()) {
            Ok(s) => toml::from_str(&s).map_err(|e| format!("{}: {e}", config_path().display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", config_path().display())),
        }
    }

    /// Atomic write (temp file + rename), mode 0600.
    pub fn save(&self) -> Result<(), String> {
        let path = config_path();
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        // Per-process temp name so concurrent saves never share a file, and O_EXCL (create_new)
        // so a pre-existing path, such as a planted symlink, is never followed or truncated and
        // the 0600 mode always applies. A stale leftover is removed first so O_EXCL can succeed.
        let tmp = parent.join(format!(".config.toml.{}.tmp", std::process::id()));
        let _ = fs::remove_file(&tmp);
        let write = || -> std::io::Result<()> {
            let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &path)
        };
        write().map_err(|e| {
            let _ = fs::remove_file(&tmp);
            e.to_string()
        })
    }

    /// Precedence: app+domain, app, domain, default.
    pub fn resolve(&self, app: Option<&str>, host: &str) -> Option<&str> {
        let app_rules = || self.rules.iter().filter(|r| r.app.is_some() && r.app.as_deref() == app);
        let dom = |r: &&Rule| r.domain.as_deref().is_some_and(|d| domain_matches(host, d));
        app_rules()
            .find(dom)
            .or_else(|| app_rules().find(|r| r.domain.is_none()))
            .or_else(|| self.rules.iter().filter(|r| r.app.is_none()).find(dom))
            .map(|r| r.browser.as_str())
            .or((!self.default.is_empty()).then_some(self.default.as_str()))
    }

    /// Insert or replace the rule with the same (app, domain) key.
    pub fn set(&mut self, app: Option<String>, domain: Option<String>, browser: String) {
        self.unset(app.as_deref(), domain.as_deref());
        self.rules.push(Rule { app, domain, browser });
    }

    pub fn unset(&mut self, app: Option<&str>, domain: Option<&str>) -> bool {
        let before = self.rules.len();
        self.rules.retain(|r| !(r.app.as_deref() == app && r.domain.as_deref() == domain));
        self.rules.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        let mut c = Config::default();
        c.set(Some("slack".into()), None, "chrome.desktop".into());
        c.set(Some("slack".into()), Some("github.com".into()), "firefox.desktop".into());
        c.set(None, Some("example.org".into()), "brave.desktop".into());
        c
    }

    #[test]
    fn precedence() {
        let c = cfg();
        assert_eq!(c.resolve(Some("slack"), "github.com"), Some("firefox.desktop"));
        assert_eq!(c.resolve(Some("slack"), "gist.github.com"), Some("firefox.desktop"));
        assert_eq!(c.resolve(Some("slack"), "notgithub.com"), Some("chrome.desktop"));
        assert_eq!(c.resolve(Some("slack"), "example.org"), Some("chrome.desktop"));
        assert_eq!(c.resolve(Some("other"), "example.org"), Some("brave.desktop"));
        assert_eq!(c.resolve(None, "example.org"), Some("brave.desktop"));
        assert_eq!(c.resolve(Some("other"), "nope.com"), None);
    }

    #[test]
    fn default_fallback() {
        let mut c = cfg();
        c.default = "zen.desktop".into();
        assert_eq!(c.resolve(None, "nope.com"), Some("zen.desktop"));
    }

    #[test]
    fn set_replaces_and_roundtrips() {
        let mut c = cfg();
        c.set(Some("slack".into()), None, "zen.desktop".into());
        assert_eq!(c.rules.len(), 3);
        let text = toml::to_string_pretty(&c).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), c);
        assert!(c.unset(Some("slack"), None));
        assert!(!c.unset(Some("slack"), None));
    }

    #[test]
    fn app_rule_with_domain_does_not_leak_to_other_domains() {
        let mut c = Config::default();
        c.set(Some("slack".into()), Some("github.com".into()), "firefox.desktop".into());
        assert_eq!(c.resolve(Some("slack"), "github.com"), Some("firefox.desktop"));
        assert_eq!(c.resolve(Some("slack"), "example.org"), None);
        assert_eq!(c.resolve(None, "github.com"), None);
        assert_eq!(c.resolve(Some("other"), "github.com"), None);
    }

    #[test]
    fn app_domain_beats_app_only_beats_domain_only_beats_default() {
        let mut c = Config { default: "d.desktop".into(), rules: vec![] };
        assert_eq!(c.resolve(Some("a"), "x.com"), Some("d.desktop"));
        c.set(None, Some("x.com".into()), "dom.desktop".into());
        assert_eq!(c.resolve(Some("a"), "x.com"), Some("dom.desktop"));
        c.set(Some("a".into()), None, "app.desktop".into());
        assert_eq!(c.resolve(Some("a"), "x.com"), Some("app.desktop"));
        c.set(Some("a".into()), Some("x.com".into()), "both.desktop".into());
        assert_eq!(c.resolve(Some("a"), "x.com"), Some("both.desktop"));
    }

    #[test]
    fn rule_order_does_not_change_precedence() {
        let mut c = Config::default();
        c.rules.push(Rule { app: Some("a".into()), domain: Some("x.com".into()), browser: "both".into() });
        c.rules.push(Rule { app: Some("a".into()), domain: None, browser: "app".into() });
        c.rules.push(Rule { app: None, domain: Some("x.com".into()), browser: "dom".into() });
        assert_eq!(c.resolve(Some("a"), "x.com"), Some("both"));
        c.rules.reverse();
        assert_eq!(c.resolve(Some("a"), "x.com"), Some("both"));
    }

    #[test]
    fn no_app_never_matches_app_rules() {
        let mut c = Config::default();
        c.set(Some("slack".into()), None, "chrome.desktop".into());
        assert_eq!(c.resolve(None, "x.com"), None);
    }

    #[test]
    fn rule_without_app_or_domain_is_inert() {
        let mut c = Config::default();
        c.set(None, None, "x.desktop".into());
        assert_eq!(c.resolve(None, "a.com"), None);
        assert_eq!(c.resolve(Some("a"), "a.com"), None);
    }

    #[test]
    fn empty_config_resolves_nothing() {
        assert_eq!(Config::default().resolve(Some("a"), "a.com"), None);
    }

    #[test]
    fn domain_matching_is_label_aware() {
        assert!(domain_matches("github.com", "github.com"));
        assert!(domain_matches("gist.github.com", "github.com"));
        assert!(domain_matches("a.b.github.com", "github.com"));
        assert!(!domain_matches("notgithub.com", "github.com"));
        assert!(!domain_matches("github.com.evil.org", "github.com"));
        assert!(!domain_matches("com", "github.com"));
        assert!(!domain_matches("", "github.com"));
    }

    #[test]
    fn set_keeps_distinct_keys_apart() {
        let mut c = Config::default();
        c.set(Some("a".into()), None, "1".into());
        c.set(None, Some("a".into()), "2".into());
        c.set(Some("a".into()), Some("a".into()), "3".into());
        assert_eq!(c.rules.len(), 3);
    }

    #[test]
    fn unset_only_removes_exact_key() {
        let mut c = cfg();
        assert!(!c.unset(Some("slack"), Some("nope.com")));
        assert!(!c.unset(None, None));
        assert_eq!(c.rules.len(), 3);
        assert!(c.unset(Some("slack"), Some("github.com")));
        assert_eq!(c.resolve(Some("slack"), "github.com"), Some("chrome.desktop"));
        assert!(c.unset(None, Some("example.org")));
        assert_eq!(c.rules.len(), 1);
    }

    #[test]
    fn parses_documented_toml_format() {
        let c: Config = toml::from_str(
            r#"
            default = "zen.desktop"

            [[rule]]
            app = "slack"
            browser = "chrome.desktop"

            [[rule]]
            domain = "example.org"
            browser = "brave.desktop"

            [[rule]]
            app = "slack"
            domain = "github.com"
            browser = "firefox.desktop"
            "#,
        )
        .unwrap();
        assert_eq!(c.default, "zen.desktop");
        assert_eq!(c.rules.len(), 3);
        assert_eq!(c.rules[1].app, None);
        assert_eq!(c.resolve(Some("slack"), "github.com"), Some("firefox.desktop"));
    }

    #[test]
    fn parse_defaults_and_rejects_bad_input() {
        assert_eq!(toml::from_str::<Config>("").unwrap(), Config::default());
        assert!(toml::from_str::<Config>("[[rule]]\napp = \"a\"").is_err(), "browser is required");
        assert!(toml::from_str::<Config>("default = 3").is_err());
        assert!(toml::from_str::<Config>("not toml [").is_err());
    }

    #[test]
    fn serialization_omits_empty_fields() {
        let text = toml::to_string_pretty(&Config::default()).unwrap();
        assert_eq!(text.trim(), "");
        let mut c = Config::default();
        c.set(None, Some("a.com".into()), "b.desktop".into());
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(!text.contains("default"));
        assert!(!text.contains("app ="));
        assert!(text.contains("[[rule]]"));
    }

    mod fs {
        use super::*;
        use crate::test_support::{TempDir, with_env, with_home};
        use std::{ffi::OsStr, os::unix::fs::PermissionsExt};

        #[test]
        fn dirs_honor_xdg_variables() {
            with_env(
                &[
                    ("XDG_CONFIG_HOME", Some(OsStr::new("/x/c"))),
                    ("XDG_STATE_HOME", Some(OsStr::new("/x/s"))),
                    ("XDG_DATA_HOME", Some(OsStr::new("/x/d"))),
                ],
                || {
                    assert_eq!(config_dir(), PathBuf::from("/x/c"));
                    assert_eq!(state_dir(), PathBuf::from("/x/s"));
                    assert_eq!(data_dir(), PathBuf::from("/x/d"));
                    assert_eq!(config_path(), PathBuf::from("/x/c/omaroute/config.toml"));
                },
            );
        }

        #[test]
        fn dirs_fall_back_to_home_when_unset_or_empty() {
            with_env(
                &[
                    ("HOME", Some(OsStr::new("/h"))),
                    ("XDG_CONFIG_HOME", None),
                    ("XDG_STATE_HOME", Some(OsStr::new(""))),
                    ("XDG_DATA_HOME", None),
                ],
                || {
                    assert_eq!(config_dir(), PathBuf::from("/h/.config"));
                    assert_eq!(state_dir(), PathBuf::from("/h/.local/state"));
                    assert_eq!(data_dir(), PathBuf::from("/h/.local/share"));
                },
            );
        }

        #[test]
        fn load_missing_file_is_default() {
            let t = TempDir::new("cfg-missing");
            with_home(t.path(), || assert_eq!(Config::load().unwrap(), Config::default()));
        }

        #[test]
        fn save_then_load_roundtrips() {
            let t = TempDir::new("cfg-roundtrip");
            with_home(t.path(), || {
                let mut c = cfg();
                c.default = "zen.desktop".into();
                c.save().unwrap();
                assert_eq!(Config::load().unwrap(), c);
            });
        }

        #[test]
        fn save_creates_parents_with_private_mode_and_leaves_no_temp() {
            let t = TempDir::new("cfg-save");
            with_home(t.path(), || {
                cfg().save().unwrap();
                let path = config_path();
                assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
                let names: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
                assert_eq!(names, vec![OsStr::new("config.toml")]);
            });
        }

        #[test]
        fn save_overwrites_and_shrinks() {
            let t = TempDir::new("cfg-overwrite");
            with_home(t.path(), || {
                cfg().save().unwrap();
                let small = Config { default: "a.desktop".into(), rules: vec![] };
                small.save().unwrap();
                assert_eq!(Config::load().unwrap(), small);
            });
        }

        #[test]
        fn load_invalid_toml_reports_path() {
            let t = TempDir::new("cfg-invalid");
            with_home(t.path(), || {
                let path = config_path();
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, "default = [").unwrap();
                let err = Config::load().unwrap_err();
                assert!(err.starts_with(&path.display().to_string()), "{err}");
            });
        }

        #[test]
        fn load_unreadable_path_is_an_error_not_default() {
            let t = TempDir::new("cfg-unreadable");
            with_home(t.path(), || {
                std::fs::create_dir_all(config_path()).unwrap(); // a directory where the file should be
                assert!(Config::load().is_err());
            });
        }

        #[test]
        fn save_never_follows_a_planted_symlink_at_the_temp_path() {
            let t = TempDir::new("cfg-symlink");
            with_home(t.path(), || {
                let dir = config_path().parent().unwrap().to_path_buf();
                std::fs::create_dir_all(&dir).unwrap();
                let decoy = t.path().join("decoy");
                std::fs::write(&decoy, "keep me").unwrap();
                let tmp = dir.join(format!(".config.toml.{}.tmp", std::process::id()));
                std::os::unix::fs::symlink(&decoy, &tmp).unwrap();
                cfg().save().unwrap();
                assert_eq!(std::fs::read_to_string(&decoy).unwrap(), "keep me");
                assert!(!tmp.exists() && std::fs::symlink_metadata(&tmp).is_err());
                assert_eq!(Config::load().unwrap(), cfg());
                assert_eq!(std::fs::metadata(config_path()).unwrap().permissions().mode() & 0o777, 0o600);
            });
        }

        #[test]
        fn save_fails_when_parent_is_a_file() {
            let t = TempDir::new("cfg-blocked");
            with_home(t.path(), || {
                std::fs::create_dir_all(config_dir()).unwrap();
                std::fs::write(config_dir().join("omaroute"), "x").unwrap();
                assert!(cfg().save().is_err());
            });
        }
    }
}
