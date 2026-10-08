//! Sandbox for driving the real `omaroute` binary: private HOME/XDG dirs, fake browsers whose
//! launches are logged to a file, and stub `xdg-settings` / `notify-send` / `update-desktop-database`.
#![allow(dead_code)]
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct Sandbox {
    pub root: PathBuf,
}

pub struct Run {
    pub out: Output,
}

impl Run {
    pub fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }
    pub fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }
    pub fn ok(&self) -> bool {
        self.out.status.success()
    }
    #[track_caller]
    pub fn assert_ok(&self) -> &Self {
        assert!(self.ok(), "expected success, stderr: {}", self.stderr());
        self
    }
    #[track_caller]
    pub fn assert_fail(&self, needle: &str) -> &Self {
        assert_eq!(self.out.status.code(), Some(1), "stdout: {} stderr: {}", self.stdout(), self.stderr());
        assert!(self.stderr().contains(needle), "stderr {:?} lacks {needle:?}", self.stderr());
        self
    }
}

fn write_exec(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("omaroute-it-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for d in ["home", "config", "state", "data", "share/applications", "bin", "log", "runtime"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let sb = Sandbox { root };
        sb.install_stubs();
        sb
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    pub fn config_file(&self) -> PathBuf {
        self.path("config/omaroute/config.toml")
    }

    pub fn write_config(&self, text: &str) {
        fs::create_dir_all(self.config_file().parent().unwrap()).unwrap();
        fs::write(self.config_file(), text).unwrap();
    }

    pub fn read_config(&self) -> String {
        fs::read_to_string(self.config_file()).unwrap_or_default()
    }

    fn install_stubs(&self) {
        let log = self.path("log");
        // xdg-settings: `get` prints the stored default, `set` stores it. FAIL_XDG=1 makes `set` fail.
        write_exec(
            &self.path("bin/xdg-settings"),
            &format!(
                r#"echo "xdg-settings $*" >> "{log}/calls"
case "$1" in
  get) [ -f "{log}/default-browser" ] && read -r v < "{log}/default-browser" && echo "$v" ;;
  set) [ -n "$FAIL_XDG" ] && exit 1; echo "$3" > "{log}/default-browser" ;;
esac
exit 0"#,
                log = log.display()
            ),
        );
        write_exec(&self.path("bin/update-desktop-database"), &format!(r#"echo "update-desktop-database $*" >> "{}/calls""#, log.display()));
        write_exec(&self.path("bin/notify-send"), &format!(r#"echo "$*" >> "{}/notifications""#, log.display()));
        // Fake browser launcher: records `<name> <args…>` per launch.
        write_exec(&self.path("bin/launch"), &format!(r#"echo "$*" >> "{}/launches""#, log.display()));
    }

    /// Install a desktop entry. `categories` empty means no Categories line.
    pub fn add_app(&self, id: &str, name: &str, categories: &str, extra: &str) {
        let exec = format!("{} {} %u", self.path("bin/launch").display(), id);
        let cats = if categories.is_empty() { String::new() } else { format!("Categories={categories}\n") };
        let text = format!(
            "[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n{cats}MimeType=x-scheme-handler/http;x-scheme-handler/https;\n{extra}\n"
        );
        fs::write(self.path("share/applications").join(format!("{id}.desktop")), text).unwrap();
        self.refresh_mime_cache();
    }

    pub fn add_browser(&self, id: &str, name: &str) {
        self.add_app(id, name, "Network;WebBrowser;", "");
    }

    /// Rebuild mimeinfo.cache from the desktop entries present.
    fn refresh_mime_cache(&self) {
        let dir = self.path("share/applications");
        let mut ids: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.ends_with(".desktop"))
            .collect();
        ids.sort();
        let list = ids.iter().map(|i| format!("{i};")).collect::<String>();
        fs::write(
            dir.join("mimeinfo.cache"),
            format!("[MIME Cache]\nx-scheme-handler/http={list}\nx-scheme-handler/https={list}\n"),
        )
        .unwrap();
    }

    pub fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_omaroute"));
        c.env_clear()
            .env("HOME", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_DATA_DIRS", self.path("share"))
            .env("XDG_CONFIG_DIRS", self.path("nonexistent"))
            .env("XDG_RUNTIME_DIR", self.path("runtime"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.path("bin").display()));
        c
    }

    pub fn run(&self, args: &[&str]) -> Run {
        Run { out: self.command().args(args).output().unwrap() }
    }

    pub fn log(&self, name: &str) -> String {
        fs::read_to_string(self.path("log").join(name)).unwrap_or_default()
    }

    /// Wait for the (asynchronous) browser launch to be recorded.
    pub fn wait_for_launches(&self, count: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let lines: Vec<String> = self.log("launches").lines().map(str::to_owned).collect();
            if lines.len() >= count || Instant::now() > deadline {
                return lines;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    /// Give a wrongly-launched browser a moment to show up, then return what was recorded.
    pub fn settle_launches(&self) -> Vec<String> {
        thread::sleep(Duration::from_millis(300));
        self.log("launches").lines().map(str::to_owned).collect()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
