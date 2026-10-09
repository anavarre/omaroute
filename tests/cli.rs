//! Black-box tests of the `omaroute` binary. The picker UI needs a Wayland compositor, so
//! every path here is one that resolves without showing it.
mod common;

use common::Sandbox;
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    thread,
};

fn sandbox() -> Sandbox {
    let sb = Sandbox::new();
    sb.add_browser("firefox", "Firefox");
    sb.add_browser("chrome", "Chrome");
    sb
}

// ---- meta -----------------------------------------------------------------------------------

#[test]
fn no_args_and_help_print_usage() {
    let sb = sandbox();
    for args in [&[][..], &["-h"], &["--help"], &["help"]] {
        let r = sb.run(args);
        r.assert_ok();
        assert!(r.stdout().starts_with("Usage:"), "{args:?}");
        assert!(r.stdout().contains("omaroute setup [--undo]"));
    }
}

#[test]
fn version_flags() {
    let sb = sandbox();
    for flag in ["-V", "--version"] {
        let r = sb.run(&[flag]);
        r.assert_ok();
        assert_eq!(r.stdout().trim(), format!("omaroute {}", env!("CARGO_PKG_VERSION")));
    }
}

// ---- list / browsers -------------------------------------------------------------------------

#[test]
fn list_is_empty_without_config() {
    let sb = sandbox();
    let r = sb.run(&["list"]);
    r.assert_ok();
    assert_eq!(r.stdout(), "");
}

#[test]
fn list_shows_default_and_rules_with_wildcards() {
    let sb = sandbox();
    sb.write_config(
        r#"default = "zen.desktop"
[[rule]]
app = "slack"
browser = "chrome.desktop"
[[rule]]
domain = "example.org"
browser = "firefox.desktop"
[[rule]]
app = "slack"
domain = "github.com"
browser = "firefox.desktop"
"#,
    );
    let r = sb.run(&["list"]);
    r.assert_ok();
    assert_eq!(
        r.stdout(),
        "default -> zen.desktop\napp=slack domain=* -> chrome.desktop\napp=* domain=example.org -> firefox.desktop\napp=slack domain=github.com -> firefox.desktop\n"
    );
}

#[test]
fn list_with_invalid_config_fails_and_notifies() {
    let sb = sandbox();
    sb.write_config("default = [");
    let r = sb.run(&["list"]);
    r.assert_fail("config.toml");
    assert!(r.stderr().starts_with("omaroute: "));
    assert!(sb.log("notifications").contains("-u critical -- omaroute"), "{}", sb.log("notifications"));
}

#[test]
fn browsers_lists_only_visible_web_browsers() {
    let sb = sandbox();
    sb.add_app("editor", "Editor", "Utility;TextEditor;", ""); // handles https but isn't a browser
    sb.add_app("nocat", "No Category", "", "");
    sb.add_app("hidden", "Hidden Browser", "Network;WebBrowser;", "NoDisplay=true");
    sb.add_browser("omaroute", "Omaroute"); // never offers itself
    let r = sb.run(&["browsers"]);
    r.assert_ok();
    let mut lines: Vec<_> = r.stdout().lines().map(str::to_owned).collect();
    lines.sort();
    assert_eq!(lines, ["chrome.desktop\tChrome", "firefox.desktop\tFirefox"]);
}

#[test]
fn browsers_is_empty_when_none_installed() {
    let sb = Sandbox::new();
    let r = sb.run(&["browsers"]);
    r.assert_ok();
    assert_eq!(r.stdout(), "");
}

// ---- set / unset -----------------------------------------------------------------------------

#[test]
fn set_requires_app_or_domain() {
    let sb = sandbox();
    sb.run(&["set", "firefox"]).assert_fail("need --app and/or --domain");
    assert!(!sb.config_file().exists());
}

#[test]
fn set_requires_exactly_one_browser() {
    let sb = sandbox();
    sb.run(&["set", "--app", "slack"]).assert_fail("expected one browser");
    sb.run(&["set", "--app", "slack", "firefox", "chrome"]).assert_fail("expected one browser");
}

#[test]
fn set_rejects_unknown_browsers_and_non_browsers_alike() {
    let sb = sandbox();
    sb.add_app("editor", "Editor", "Utility;TextEditor;", ""); // handles https but isn't a browser
    sb.add_app("nocat", "No Category", "", "");
    sb.run(&["set", "--app", "slack", "nope"]).assert_fail("browser 'nope.desktop' not found");
    sb.run(&["set", "--app", "slack", "omaroute"]).assert_fail("browser 'omaroute.desktop' not found");
    sb.run(&["set", "--app", "slack", "editor"]).assert_fail("'editor.desktop' is not a web browser");
    sb.run(&["set", "--domain", "a.com", "nocat.desktop"]).assert_fail("'nocat.desktop' is not a web browser");
    sb.run(&["default", "editor"]).assert_fail("'editor.desktop' is not a web browser");
    assert!(!sb.config_file().exists());
}

#[test]
fn rule_pointing_at_a_non_browser_is_refused_when_opening_links() {
    let sb = sandbox();
    sb.add_app("editor", "Editor", "Utility;TextEditor;", "");
    sb.write_config("default = \"editor.desktop\"\n[[rule]]\ndomain = \"a.com\"\nbrowser = \"editor.desktop\"\n");
    sb.run(&["https://a.com/"]).assert_fail("'editor.desktop' is not a web browser");
    sb.run(&["https://b.com/"]).assert_fail("'editor.desktop' is not a web browser");
    assert!(sb.log("notifications").contains("not a web browser"));
    assert!(sb.settle_launches().is_empty());
}

#[test]
fn set_rejects_unknown_options_and_missing_values() {
    let sb = sandbox();
    sb.run(&["set", "--bogus", "x", "firefox"]).assert_fail("unknown option --bogus");
    sb.run(&["set", "firefox", "--app"]).assert_fail("--app needs a value");
}

#[test]
fn set_writes_normalized_lowercase_rules_with_private_mode() {
    let sb = sandbox();
    sb.run(&["set", "--app", "Slack", "--domain", "GitHub.com", "firefox"]).assert_ok();
    sb.run(&["set", "--app", "slack", "chrome.desktop"]).assert_ok();
    sb.run(&["set", "--domain", "example.org", "firefox"]).assert_ok();
    let r = sb.run(&["list"]);
    assert_eq!(
        r.stdout(),
        "app=slack domain=github.com -> firefox.desktop\napp=slack domain=* -> chrome.desktop\napp=* domain=example.org -> firefox.desktop\n"
    );
    assert_eq!(fs::metadata(sb.config_file()).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn set_replaces_rule_with_same_key() {
    let sb = sandbox();
    sb.run(&["set", "--app", "slack", "firefox"]).assert_ok();
    sb.run(&["set", "--app", "slack", "chrome"]).assert_ok();
    assert_eq!(sb.run(&["list"]).stdout(), "app=slack domain=* -> chrome.desktop\n");
}

#[test]
fn set_preserves_default() {
    let sb = sandbox();
    sb.run(&["default", "firefox"]).assert_ok();
    sb.run(&["set", "--app", "slack", "chrome"]).assert_ok();
    assert!(sb.run(&["list"]).stdout().starts_with("default -> firefox.desktop\n"));
}

#[test]
fn set_refuses_to_overwrite_a_corrupt_config() {
    let sb = sandbox();
    sb.write_config("garbage = = =");
    sb.run(&["set", "--app", "slack", "firefox"]).assert_fail("config.toml");
    assert_eq!(sb.read_config(), "garbage = = =");
}

#[test]
fn unset_removes_exact_rule_only() {
    let sb = sandbox();
    sb.run(&["set", "--app", "slack", "firefox"]).assert_ok();
    sb.run(&["set", "--app", "slack", "--domain", "a.com", "chrome"]).assert_ok();
    sb.run(&["unset", "--app", "slack"]).assert_ok();
    assert_eq!(sb.run(&["list"]).stdout(), "app=slack domain=a.com -> chrome.desktop\n");
    sb.run(&["unset", "--app", "slack"]).assert_fail("no such rule");
    sb.run(&["unset", "--app", "SLACK", "--domain", "A.com"]).assert_ok();
    assert_eq!(sb.run(&["list"]).stdout(), "");
}

#[test]
fn unset_requires_a_selector() {
    let sb = sandbox();
    sb.run(&["unset"]).assert_fail("need --app and/or --domain");
}

// ---- default ---------------------------------------------------------------------------------

#[test]
fn default_get_set_clear() {
    let sb = sandbox();
    assert_eq!(sb.run(&["default"]).stdout(), "\n");
    sb.run(&["default", "firefox"]).assert_ok();
    assert_eq!(sb.run(&["default"]).stdout(), "firefox.desktop\n");
    sb.run(&["default", "--clear"]).assert_ok();
    assert_eq!(sb.run(&["default"]).stdout(), "\n");
    assert!(!sb.read_config().contains("default"));
}

#[test]
fn default_validates_browser() {
    let sb = sandbox();
    sb.run(&["default", "nope"]).assert_fail("browser 'nope.desktop' not found");
    assert_eq!(sb.run(&["default"]).stdout(), "\n");
}

#[test]
fn default_keeps_rules() {
    let sb = sandbox();
    sb.run(&["set", "--app", "slack", "chrome"]).assert_ok();
    sb.run(&["default", "firefox"]).assert_ok();
    sb.run(&["default", "--clear"]).assert_ok();
    assert_eq!(sb.run(&["list"]).stdout(), "app=slack domain=* -> chrome.desktop\n");
}

// ---- opening links ---------------------------------------------------------------------------

#[test]
fn matching_app_rule_launches_browser_with_untouched_url() {
    let sb = sandbox();
    sb.run(&["set", "--app", "slack", "chrome"]).assert_ok();
    let url = "https://Example.com/a%20b?q=1&r=2#frag";
    sb.run(&[url, "--app", "slack"]).assert_ok();
    assert_eq!(sb.wait_for_launches(1), [format!("chrome {url}")]);
}

#[test]
fn app_flag_position_and_case_do_not_matter() {
    let sb = sandbox();
    sb.run(&["set", "--app", "slack", "chrome"]).assert_ok();
    sb.run(&["--app", "SLACK", "https://a.com/"]).assert_ok();
    assert_eq!(sb.wait_for_launches(1), ["chrome https://a.com/"]);
}

#[test]
fn domain_rule_matches_subdomains_but_not_lookalikes() {
    let sb = sandbox();
    sb.run(&["set", "--domain", "github.com", "firefox"]).assert_ok();
    sb.run(&["--app", "x", "https://gist.github.com/z"]).assert_ok();
    assert_eq!(sb.wait_for_launches(1), ["firefox https://gist.github.com/z"]);
    // A lookalike doesn't match, so omaroute tries to show the picker; with no browsers
    // installed it stops before any UI.
    let empty = Sandbox::new();
    empty.write_config(&sb.read_config());
    empty.run(&["https://notgithub.com/"]).assert_fail("no browsers found");
}

#[test]
fn precedence_app_domain_over_app_over_domain_over_default() {
    let sb = sandbox();
    sb.run(&["default", "chrome"]).assert_ok();
    sb.run(&["set", "--domain", "a.com", "firefox"]).assert_ok();
    sb.run(&["--app", "z", "https://a.com/1"]).assert_ok();
    sb.run(&["--app", "z", "https://b.com/2"]).assert_ok();
    sb.run(&["set", "--app", "z", "chrome"]).assert_ok();
    sb.run(&["set", "--app", "z", "--domain", "a.com", "firefox"]).assert_ok();
    sb.run(&["--app", "z", "https://a.com/3"]).assert_ok();
    sb.run(&["--app", "z", "https://c.com/4"]).assert_ok();
    let mut got = sb.wait_for_launches(4);
    got.sort();
    assert_eq!(got, ["chrome https://b.com/2", "chrome https://c.com/4", "firefox https://a.com/1", "firefox https://a.com/3"]);
}

#[test]
fn default_browser_handles_unmatched_links() {
    let sb = sandbox();
    sb.run(&["default", "firefox"]).assert_ok();
    sb.run(&["https://anything.example/"]).assert_ok();
    assert_eq!(sb.wait_for_launches(1), ["firefox https://anything.example/"]);
}

#[test]
fn http_scheme_and_ports_and_userinfo_route_by_host() {
    let sb = sandbox();
    sb.run(&["set", "--domain", "localhost", "chrome"]).assert_ok();
    sb.run(&["http://user:pw@localhost:3000/x"]).assert_ok();
    assert_eq!(sb.wait_for_launches(1), ["chrome http://user:pw@localhost:3000/x"]);
}

#[test]
fn rule_pointing_at_missing_browser_fails_with_notification() {
    let sb = sandbox();
    sb.write_config("[[rule]]\ndomain = \"a.com\"\nbrowser = \"gone.desktop\"\n");
    sb.run(&["https://a.com/"]).assert_fail("browser 'gone.desktop' not found");
    assert!(sb.log("notifications").contains("not found"));
    assert!(sb.settle_launches().is_empty());
}

#[test]
fn rule_pointing_at_omaroute_itself_is_refused_to_prevent_loops() {
    let sb = sandbox();
    sb.add_browser("omaroute", "Omaroute");
    sb.write_config("default = \"omaroute.desktop\"\n");
    sb.run(&["https://a.com/"]).assert_fail("browser 'omaroute.desktop' not found");
    assert!(sb.settle_launches().is_empty());
}

#[test]
fn invalid_urls_are_rejected_before_anything_launches() {
    let sb = sandbox();
    sb.run(&["default", "firefox"]).assert_ok();
    for (u, why) in [
        ("file:///etc/passwd", "unsupported scheme"),
        ("javascript:alert(1)", "not an http(s) URL"),
        ("ftp://a.com", "unsupported scheme"),
        ("https://", "no host"),
        ("https://a.com/ x", "whitespace"),
        ("example.com", "not an http(s) URL"),
    ] {
        sb.run(&[u]).assert_fail(why);
    }
    assert!(sb.settle_launches().is_empty());
}

#[test]
fn backslash_tricks_route_by_the_host_the_browser_will_visit() {
    let sb = sandbox();
    sb.run(&["set", "--domain", "evil.com", "chrome"]).assert_ok();
    sb.run(&["set", "--domain", "good.com", "firefox"]).assert_ok();
    // Browsers parse '\' like '/', so this link really goes to evil.com.
    sb.run(&[r"https://evil.com\@good.com/"]).assert_ok();
    assert_eq!(sb.wait_for_launches(1), [r"chrome https://evil.com\@good.com/"]);
}

#[test]
fn notification_text_is_markup_escaped() {
    let sb = sandbox();
    sb.run(&["<b>x</b>://a.com/"]).assert_fail("unsupported scheme");
    let log = sb.log("notifications");
    assert!(log.contains("&lt;b&gt;x&lt;/b&gt;"), "{log}");
    assert!(!log.contains("<b>"), "{log}");
}

#[test]
fn url_argument_count_is_enforced() {
    let sb = sandbox();
    sb.run(&["--app", "slack"]).assert_fail("expected exactly one URL");
    sb.run(&["https://a.com", "https://b.com"]).assert_fail("expected exactly one URL");
}

#[test]
fn unknown_option_and_missing_value_fail() {
    let sb = sandbox();
    sb.run(&["https://a.com", "--nope"]).assert_fail("unknown option --nope");
    sb.run(&["https://a.com", "--app"]).assert_fail("--app needs a value");
}

#[test]
fn corrupt_config_is_ignored_when_opening_links() {
    let sb = Sandbox::new(); // no browsers: stops before the picker
    sb.write_config("default = [");
    let r = sb.run(&["https://a.com/"]);
    r.assert_fail("no browsers found");
    assert!(r.stderr().contains("ignoring bad config"), "{}", r.stderr());
    assert_eq!(sb.read_config(), "default = [", "must not rewrite a config it could not parse");
}

#[test]
fn unmatched_link_without_browsers_fails_cleanly() {
    let sb = Sandbox::new();
    sb.run(&["https://a.com/"]).assert_fail("no browsers found");
    assert!(sb.log("notifications").contains("no browsers found"));
}

// ---- focused-window detection ----------------------------------------------------------------

fn fake_hyprland(sb: &Sandbox, reply: &'static str) -> (String, thread::JoinHandle<String>) {
    let sig = "testsig";
    let dir = sb.path("runtime/hypr").join(sig);
    fs::create_dir_all(&dir).unwrap();
    let l = UnixListener::bind(dir.join(".socket.sock")).unwrap();
    let h = thread::spawn(move || {
        let (mut c, _) = l.accept().unwrap();
        let mut buf = [0u8; 64];
        let n = c.read(&mut buf).unwrap();
        c.write_all(reply.as_bytes()).unwrap();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    });
    (sig.to_owned(), h)
}

#[test]
fn source_app_is_detected_from_the_focused_hyprland_window() {
    let sb = sandbox();
    sb.run(&["set", "--app", "slack", "chrome"]).assert_ok();
    sb.run(&["default", "firefox"]).assert_ok();
    let (sig, handle) = fake_hyprland(&sb, r#"{"class":"Slack"}"#);
    sb.command().env("HYPRLAND_INSTANCE_SIGNATURE", sig).arg("https://a.com/").output().unwrap();
    assert_eq!(sb.wait_for_launches(1), ["chrome https://a.com/"]);
    assert_eq!(handle.join().unwrap(), "j/activewindow");
}

#[test]
fn explicit_app_flag_beats_focused_window() {
    let sb = sandbox();
    sb.run(&["set", "--app", "slack", "chrome"]).assert_ok();
    sb.run(&["set", "--app", "zed", "firefox"]).assert_ok();
    let (sig, _h) = fake_hyprland(&sb, r#"{"class":"slack"}"#);
    sb.command().env("HYPRLAND_INSTANCE_SIGNATURE", sig).args(["https://a.com/", "--app", "zed"]).output().unwrap();
    assert_eq!(sb.wait_for_launches(1), ["firefox https://a.com/"]);
}

#[test]
fn unresolvable_focus_falls_back_to_domain_and_default_rules() {
    let sb = sandbox();
    sb.run(&["set", "--app", "slack", "chrome"]).assert_ok();
    sb.run(&["default", "firefox"]).assert_ok();
    let (sig, _h) = fake_hyprland(&sb, "{}");
    sb.command().env("HYPRLAND_INSTANCE_SIGNATURE", sig).arg("https://a.com/").output().unwrap();
    assert_eq!(sb.wait_for_launches(1), ["firefox https://a.com/"]);
}

// ---- setup -----------------------------------------------------------------------------------

fn desktop_file(sb: &Sandbox) -> std::path::PathBuf {
    sb.path("data/applications/omaroute.desktop")
}

#[test]
fn setup_installs_entry_remembers_previous_browser_and_registers() {
    let sb = sandbox();
    fs::write(sb.path("log/default-browser"), "firefox.desktop\n").unwrap();
    let r = sb.run(&["setup"]);
    r.assert_ok();
    assert!(r.stdout().contains("set as default browser (previous: firefox.desktop)"), "{}", r.stdout());

    let entry = fs::read_to_string(desktop_file(&sb)).unwrap();
    let exe = env!("CARGO_BIN_EXE_omaroute");
    assert!(entry.contains(&format!("Exec={exe} %u")), "{entry}");
    assert!(entry.contains("x-scheme-handler/https"));
    assert_eq!(fs::read_to_string(sb.path("state/omaroute/previous-browser")).unwrap(), "firefox.desktop");
    assert_eq!(sb.log("default-browser").trim(), "omaroute.desktop");
    let calls = sb.log("calls");
    assert!(calls.contains("update-desktop-database"), "{calls}");
    assert!(calls.contains("xdg-settings set default-web-browser omaroute.desktop"), "{calls}");
}

#[test]
fn setup_is_idempotent_and_does_not_clobber_the_saved_browser() {
    let sb = sandbox();
    fs::write(sb.path("log/default-browser"), "firefox.desktop\n").unwrap();
    sb.run(&["setup"]).assert_ok();
    sb.run(&["setup"]).assert_ok(); // now the "current" default is omaroute itself
    assert_eq!(fs::read_to_string(sb.path("state/omaroute/previous-browser")).unwrap(), "firefox.desktop");
}

#[test]
fn setup_with_no_prior_default_records_nothing() {
    let sb = sandbox();
    sb.run(&["setup"]).assert_ok();
    assert!(!sb.path("state/omaroute/previous-browser").exists());
    assert!(desktop_file(&sb).exists());
}

#[test]
fn setup_removes_legacy_theme_template() {
    let sb = sandbox();
    let tpl = sb.path("config/omarchy/themed/omaroute.css.tpl");
    fs::create_dir_all(tpl.parent().unwrap()).unwrap();
    fs::write(&tpl, "old").unwrap();
    sb.run(&["setup"]).assert_ok();
    assert!(!tpl.exists());
}

#[test]
fn setup_fails_loudly_when_xdg_settings_fails() {
    let sb = sandbox();
    let r = sb.command().env("FAIL_XDG", "1").arg("setup").output().unwrap();
    assert_eq!(r.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&r.stderr).contains("xdg-settings failed"));
}

#[test]
fn setup_undo_restores_previous_browser_and_cleans_up() {
    let sb = sandbox();
    fs::write(sb.path("log/default-browser"), "firefox.desktop\n").unwrap();
    sb.run(&["setup"]).assert_ok();
    let r = sb.run(&["setup", "--undo"]);
    r.assert_ok();
    assert_eq!(r.stdout().trim(), "omaroute: removed");
    assert_eq!(sb.log("default-browser").trim(), "firefox.desktop");
    assert!(!desktop_file(&sb).exists());
    assert!(!sb.path("state/omaroute/previous-browser").exists());
}

#[test]
fn setup_undo_without_prior_setup_is_harmless() {
    let sb = sandbox();
    let r = sb.run(&["setup", "--undo"]);
    r.assert_ok();
    assert!(!sb.log("calls").contains("xdg-settings set"), "nothing to restore");
}

#[test]
fn setup_undo_keeps_user_config() {
    let sb = sandbox();
    sb.run(&["set", "--app", "slack", "chrome"]).assert_ok();
    sb.run(&["setup"]).assert_ok();
    sb.run(&["setup", "--undo"]).assert_ok();
    assert!(sb.read_config().contains("slack"));
}
