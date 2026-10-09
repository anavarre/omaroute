mod config;
mod picker;
mod source;
mod theme;
mod url;

use config::{Config, config_dir, data_dir, state_dir};
use gtk4::{gio, prelude::*};
use std::{env, fs, process::{Command, exit}};

#[cfg(test)]
mod test_support;

const SELF_ID: &str = "omaroute.desktop";
const DESKTOP: &str = include_str!("../data/omaroute.desktop");
const USAGE: &str = "\
Usage:
  omaroute <url> [--app NAME]                 open a link (this is what the system calls)
  omaroute list                               show rules
  omaroute browsers                           show installed browsers
  omaroute set [--app A] [--domain D] <browser>   add/replace a rule
  omaroute unset [--app A] [--domain D]       remove a rule
  omaroute default [browser|--clear]          get/set the fallback browser
  omaroute setup [--undo]                     register as default browser
Config: ~/.config/omaroute/config.toml";

fn fail(msg: &str) -> ! {
    eprintln!("omaroute: {msg}");
    // `--` stops option parsing and the escape keeps daemons that render Pango markup
    // from interpreting tags smuggled in via the URL (e.g. its scheme).
    let body = msg.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let _ = Command::new("notify-send").args(["-u", "critical", "--", "omaroute", &body]).status();
    exit(1)
}

fn normalize(id: &str) -> String {
    if id.ends_with(".desktop") { id.to_owned() } else { format!("{id}.desktop") }
}

/// Only real browsers: desktop entries in the freedesktop `WebBrowser` category.
fn is_browser(a: &gio::AppInfo) -> bool {
    a.downcast_ref::<gio_unix::DesktopAppInfo>()
        .and_then(|d| d.categories())
        .is_some_and(|c| c.split(';').any(|x| x == "WebBrowser"))
}

/// The desktop entry a link may be handed to: it must exist, be a web browser and not be
/// omaroute itself. Applied both when a rule is written and when one is used, so a URL from an
/// untrusted app only ever reaches a browser, whatever the config file says.
fn browser_info(id: &str) -> Result<gio_unix::DesktopAppInfo, String> {
    let info = if id == SELF_ID { None } else { gio_unix::DesktopAppInfo::new(id) };
    let info = info.ok_or_else(|| format!("browser '{id}' not found (see `omaroute browsers`)"))?;
    if !is_browser(info.upcast_ref()) {
        return Err(format!("'{id}' is not a web browser (see `omaroute browsers`)"));
    }
    Ok(info)
}

fn launch(id: &str, url: &str) {
    let info = browser_info(id).unwrap_or_else(|e| fail(&e));
    if let Err(e) = info.launch_uris(&[url], None::<&gio::AppLaunchContext>) {
        fail(&format!("could not launch {id}: {e}"));
    }
}

fn browsers() -> Vec<picker::Browser> {
    gio::AppInfo::recommended_for_type("x-scheme-handler/https")
        .into_iter()
        .filter(|a| a.should_show() && a.id().is_some_and(|id| id != SELF_ID))
        .filter(is_browser)
        .map(|a| picker::Browser { id: a.id().unwrap().to_string(), name: a.name().to_string(), icon: a.icon() })
        .collect()
}

/// Parse `--app X` / `--domain Y` style flags; returns (flags, positional args).
fn flags(args: &[String], names: &[&str]) -> (Vec<Option<String>>, Vec<String>) {
    let mut vals = vec![None; names.len()];
    let mut pos = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match names.iter().position(|n| a == n) {
            Some(i) => vals[i] = Some(it.next().unwrap_or_else(|| fail(&format!("{a} needs a value"))).to_lowercase()),
            None if a.starts_with("--") => fail(&format!("unknown option {a}")),
            None => pos.push(a.clone()),
        }
    }
    (vals, pos)
}

fn handle_url(args: &[String]) {
    let (f, pos) = flags(args, &["--app"]);
    let [url] = pos.as_slice() else { fail("expected exactly one URL") };
    let host = url::host(url).unwrap_or_else(|e| fail(&e));
    let app = f[0].clone().or_else(source::focused_app);
    let mut cfg = Config::load().unwrap_or_else(|e| {
        eprintln!("omaroute: ignoring bad config: {e}");
        Config::default()
    });

    if let Some(id) = cfg.resolve(app.as_deref(), &host) {
        return launch(id, url);
    }
    let list = browsers();
    if list.is_empty() {
        fail("no browsers found");
    }
    let Some((id, scope)) = picker::run(app.as_deref(), &host, list) else { return };
    let (a, d) = (app.clone(), Some(host.clone()));
    match scope {
        picker::Scope::Once => {}
        picker::Scope::App => cfg.set(a, None, id.clone()),
        picker::Scope::AppDomain => cfg.set(a, d, id.clone()),
        picker::Scope::Domain => cfg.set(None, d, id.clone()),
    }
    if scope != picker::Scope::Once
        && let Err(e) = cfg.save()
    {
        eprintln!("omaroute: could not save config: {e}");
    }
    launch(&id, url);
}

fn load() -> Config {
    Config::load().unwrap_or_else(|e| fail(&e))
}

fn checked_browser(id: &str) -> String {
    let id = normalize(id);
    browser_info(&id).unwrap_or_else(|e| fail(&e));
    id
}

/// Quote a path for a desktop-entry Exec= field: backslash-escape the characters that are
/// special inside double quotes, then double `%` so nothing reads as a field code.
/// Paths made only of plain characters are left bare: `xdg-settings` doesn't unquote Exec= and
/// would reject `"/usr/bin/omaroute"` as a missing command.
fn exec_quote(path: &str) -> String {
    if path.chars().all(|c| c.is_ascii_alphanumeric() || "/._+-".contains(c)) {
        return path.to_owned();
    }
    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out.replace('%', "%%")
}

/// `xdg-settings` refuses to touch the default browser while $BROWSER is set (Omarchy sets it).
fn xdg_settings() -> Command {
    let mut c = Command::new("xdg-settings");
    c.env_remove("BROWSER");
    c
}

fn setup(undo: bool) {
    let desktop = data_dir().join("applications").join(SELF_ID);
    let tpl = config_dir().join("omarchy/themed/omaroute.css.tpl");
    let prev_file = state_dir().join("omaroute/previous-browser");
    if undo {
        if let Ok(prev) = fs::read_to_string(&prev_file) {
            let _ = xdg_settings().args(["set", "default-web-browser", prev.trim()]).status();
        }
        for f in [&desktop, &tpl, &prev_file] {
            let _ = fs::remove_file(f);
        }
        return println!("omaroute: removed");
    }
    let exe = env::current_exe().unwrap_or_else(|e| fail(&e.to_string()));
    // A desktop entry value cannot carry a newline or other control character; one in the path
    // would end the Exec= line early and let the rest be read as further keys.
    if exe.display().to_string().chars().any(char::is_control) {
        fail("the path to this binary contains control characters; move omaroute somewhere plain and re-run setup");
    }
    let write = |path: &std::path::Path, text: &str| {
        fs::create_dir_all(path.parent().unwrap()).and_then(|_| fs::write(path, text)).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())))
    };
    write(&desktop, &DESKTOP.replace("Exec=omaroute", &format!("Exec={}", exec_quote(&exe.display().to_string()))));
    let _ = fs::remove_file(&tpl); // legacy theme template; the picker now reads the shell theme directly
    let current = xdg_settings().args(["get", "default-web-browser"]).output().ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_default();
    if !current.is_empty() && current != SELF_ID && !prev_file.exists() {
        write(&prev_file, &current);
    }
    let _ = Command::new("update-desktop-database").arg(data_dir().join("applications")).status();
    if !xdg_settings().args(["set", "default-web-browser", SELF_ID]).status().is_ok_and(|s| s.success()) {
        fail("xdg-settings failed");
    }
    println!("omaroute: set as default browser (previous: {current})");
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(cmd) = args.first() else { return println!("{USAGE}") };
    let rest = &args[1..];
    match cmd.as_str() {
        "-h" | "--help" | "help" => println!("{USAGE}"),
        "-V" | "--version" => println!("omaroute {}", env!("CARGO_PKG_VERSION")),
        "list" => {
            let c = load();
            if !c.default.is_empty() { println!("default -> {}", c.default) }
            for r in &c.rules {
                println!("app={} domain={} -> {}", r.app.as_deref().unwrap_or("*"), r.domain.as_deref().unwrap_or("*"), r.browser);
            }
        }
        "browsers" => browsers().iter().for_each(|b| println!("{}\t{}", b.id, b.name)),
        "set" | "unset" => {
            let (f, pos) = flags(rest, &["--app", "--domain"]);
            if f[0].is_none() && f[1].is_none() { fail("need --app and/or --domain") }
            let mut c = load();
            if cmd == "set" {
                let [b] = pos.as_slice() else { fail("expected one browser") };
                c.set(f[0].clone(), f[1].clone(), checked_browser(b));
            } else if !c.unset(f[0].as_deref(), f[1].as_deref()) {
                fail("no such rule");
            }
            c.save().unwrap_or_else(|e| fail(&e));
        }
        "default" => {
            let mut c = load();
            match rest.first().map(String::as_str) {
                None => println!("{}", c.default),
                Some("--clear") => { c.default.clear(); c.save().unwrap_or_else(|e| fail(&e)) }
                Some(b) => { c.default = checked_browser(b); c.save().unwrap_or_else(|e| fail(&e)) }
            }
        }
        "setup" => setup(rest.first().is_some_and(|a| a == "--undo")),
        _ => handle_url(&args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn normalize_appends_suffix_once() {
        assert_eq!(normalize("firefox"), "firefox.desktop");
        assert_eq!(normalize("firefox.desktop"), "firefox.desktop");
        assert_eq!(normalize(""), ".desktop");
    }

    #[test]
    fn browser_info_never_resolves_self() {
        assert!(browser_info(SELF_ID).unwrap_err().contains("not found"));
    }

    #[test]
    fn flags_parses_values_and_positionals() {
        let (v, pos) = flags(&args(&["--app", "Slack", "https://a.com", "--domain", "GitHub.com"]), &["--app", "--domain"]);
        assert_eq!(v, vec![Some("slack".to_string()), Some("github.com".to_string())]);
        assert_eq!(pos, args(&["https://a.com"]));
    }

    #[test]
    fn flags_defaults_to_none_and_keeps_order() {
        let (v, pos) = flags(&args(&["b", "a"]), &["--app", "--domain"]);
        assert_eq!(v, vec![None, None]);
        assert_eq!(pos, args(&["b", "a"]));
        let (v, pos) = flags(&[], &["--app"]);
        assert_eq!((v, pos), (vec![None], vec![]));
    }

    #[test]
    fn flags_last_occurrence_wins() {
        let (v, _) = flags(&args(&["--app", "a", "--app", "b"]), &["--app"]);
        assert_eq!(v, vec![Some("b".to_string())]);
    }

    #[test]
    fn flags_lowercases_values_but_not_positionals() {
        let (v, pos) = flags(&args(&["--app", "ZED", "Firefox"]), &["--app"]);
        assert_eq!(v[0].as_deref(), Some("zed"));
        assert_eq!(pos, args(&["Firefox"]));
    }

    #[test]
    fn exec_quote_escapes_reserved_characters() {
        assert_eq!(exec_quote("/usr/bin/omaroute"), "/usr/bin/omaroute");
        assert_eq!(exec_quote("/a dir/omaroute"), "\"/a dir/omaroute\"");
        assert_eq!(exec_quote(r#"/a"b/$x/`y/\z"#), r#""/a\"b/\$x/\`y/\\z""#);
        assert_eq!(exec_quote("/opt/100%u/bin"), "\"/opt/100%%u/bin\"");
    }

    #[test]
    fn usage_documents_every_command() {
        for c in ["list", "browsers", "set", "unset", "default", "setup", "--app", "--domain"] {
            assert!(USAGE.contains(c), "{c}");
        }
    }

    #[test]
    fn bundled_desktop_entry_is_routable() {
        assert!(DESKTOP.contains("Exec=omaroute %u"));
        assert!(DESKTOP.contains("x-scheme-handler/http;"));
        assert!(DESKTOP.contains("x-scheme-handler/https;"));
        assert!(DESKTOP.contains("WebBrowser"));
    }
}
