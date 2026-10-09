//! Follows the Omarchy shell theme the same way Quickshell surfaces do: the current theme's
//! `shell.toml`, with `~/.config/omarchy/shell.toml` layered on top. The picker uses the `[menu]`
//! tokens, so it looks like the launcher, clipboard and emoji menus under any theme.
use std::fs;

use crate::config::{config_dir, state_dir};

type Table = toml::Table;

fn load(path: std::path::PathBuf) -> Table {
    fs::read_to_string(path).ok().and_then(|s| s.parse().ok()).unwrap_or_default()
}

/// User keys win over theme keys, per key within a section.
fn merge(mut theme: Table, user: Table) -> Table {
    for (section, user) in user {
        match (theme.get_mut(&section), user) {
            (Some(toml::Value::Table(t)), toml::Value::Table(u)) => t.extend(u),
            (_, user) => { theme.insert(section, user); }
        }
    }
    theme
}

fn merged() -> Table {
    merge(
        load(state_dir().join("omarchy/current/theme/shell.toml")),
        load(config_dir().join("omarchy/shell.toml")),
    )
}

fn is_hex_color(s: &str) -> bool {
    s.strip_prefix('#').is_some_and(|h| matches!(h.len(), 3 | 4 | 6 | 8) && h.bytes().all(|b| b.is_ascii_hexdigit()))
}

struct Tokens(Table);

impl Tokens {
    fn str(&self, section: &str, key: &str) -> Option<&str> {
        self.0.get(section)?.as_table()?.get(key)?.as_str()
    }

    fn num(&self, section: &str, key: &str) -> Option<f64> {
        let v = self.0.get(section)?.as_table()?.get(key)?;
        // TOML allows `inf`/`nan`; neither is a usable size or alpha and both would render as
        // broken CSS, so they fall back to the default like any other bad value.
        v.as_float().or_else(|| v.as_integer().map(|i| i as f64)).filter(|n| n.is_finite())
    }

    /// A color token, following `hyprland.*` references; gradients collapse to their first color.
    /// Only strict hex colors are accepted: theme files are third-party content, and anything
    /// looser would let a theme inject arbitrary CSS into the picker.
    fn color(&self, section: &str, key: &str, default: &str) -> String {
        let mut value = self.str(section, key).unwrap_or(default);
        if let Some(name) = value.strip_prefix("hyprland.") {
            value = self.str("hyprland", name).unwrap_or(default);
        }
        value.split_whitespace().find(|p| is_hex_color(p)).unwrap_or(default).to_owned()
    }

    fn alpha(&self, section: &str, key: &str, default: f64) -> f64 {
        self.num(section, key).unwrap_or(default).clamp(0.0, 1.0)
    }
}

/// CSS color definitions plus the font size, ready to prepend to the structural stylesheet.
pub fn css() -> String {
    render(merged())
}

fn render(table: Table) -> String {
    let t = Tokens(table);
    let m = |k: &str, d: &str| t.color("menu", k, d);
    let a = |k: &str, d: f64| t.alpha("menu", k, d);
    let font = t.num("font", "base-size").unwrap_or(12.0).clamp(1.0, 128.0);
    let fg = m("text", "#cdd6f4");
    format!(
        "@define-color menu_bg {bg};\n@define-color menu_text {fg};\n@define-color menu_border {border};\n\
         @define-color menu_sel_bg {sel_bg};\n@define-color menu_sel_text {sel_text};\n\
         @define-color menu_sel_border {sel_border};\n@define-color muted alpha({fg}, 0.55);\n\
         window.omaroute * {{ font-size: {font}px; }}\n\
         .root {{ background: alpha(@menu_bg, {bg_alpha}); border-color: alpha(@menu_border, {border_alpha}); }}\n\
         button.browser.selected {{ background: alpha(@menu_sel_bg, {sel_bg_alpha}); border-color: alpha(@menu_sel_border, {sel_border_alpha}); }}\n",
        bg = m("background", "#1e1e2e"),
        border = m("border", &fg),
        sel_bg = m("selected-background", &fg),
        sel_text = m("selected-text", "#89b4fa"),
        sel_border = m("selected-border", &fg),
        bg_alpha = a("background-alpha", 1.0),
        border_alpha = a("border-alpha", 1.0),
        sel_bg_alpha = a("selected-background-alpha", 0.08),
        sel_border_alpha = a("selected-border-alpha", 0.25),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDir, with_env, with_home};
    use std::ffi::OsStr;

    fn t(s: &str) -> Table {
        s.parse().unwrap()
    }

    #[test]
    fn empty_theme_uses_defaults() {
        let css = render(Table::new());
        assert!(css.contains("@define-color menu_bg #1e1e2e;"));
        assert!(css.contains("@define-color menu_text #cdd6f4;"));
        assert!(css.contains("@define-color menu_sel_text #89b4fa;"));
        assert!(css.contains("font-size: 12px;"));
        assert!(css.contains("alpha(@menu_bg, 1)"));
        assert!(css.contains("alpha(@menu_sel_bg, 0.08)"));
        assert!(css.contains("alpha(@menu_sel_border, 0.25)"));
    }

    #[test]
    fn non_finite_and_absurd_numbers_fall_back_to_defaults() {
        let css = render(t("[menu]\nbackground-alpha = nan\nborder-alpha = inf\n[font]\nbase-size = -inf"));
        assert!(css.contains("alpha(@menu_bg, 1)"), "{css}");
        assert!(css.contains("alpha(@menu_border, 1)"), "{css}");
        assert!(css.contains("font-size: 12px;"), "{css}");
        assert!(!css.contains("NaN") && !css.contains("inf"), "{css}");
        let css = render(t("[font]\nbase-size = 100000"));
        assert!(css.contains("font-size: 128px;"), "{css}");
    }

    #[test]
    fn border_and_selection_default_to_text_color() {
        let css = render(t("[menu]\ntext = \"#112233\""));
        assert!(css.contains("@define-color menu_border #112233;"));
        assert!(css.contains("@define-color menu_sel_bg #112233;"));
        assert!(css.contains("@define-color menu_sel_border #112233;"));
        assert!(css.contains("alpha(#112233, 0.55)"));
    }

    #[test]
    fn explicit_tokens_are_used() {
        let css = render(t(r##"
            [menu]
            background = "#000001"
            text = "#000002"
            border = "#000003"
            selected-background = "#000004"
            selected-text = "#000005"
            selected-border = "#000006"
            background-alpha = 0.5
            border-alpha = 0.6
            selected-background-alpha = 0.7
            selected-border-alpha = 0.8
            [font]
            base-size = 16
        "##));
        for (n, c) in [("bg", "1"), ("text", "2"), ("border", "3"), ("sel_bg", "4"), ("sel_text", "5"), ("sel_border", "6")] {
            assert!(css.contains(&format!("@define-color menu_{n} #00000{c};")), "{n}");
        }
        for a in ["0.5", "0.6", "0.7", "0.8"] {
            assert!(css.contains(&format!("alpha(@menu_bg, {a})")) || css.contains(&format!("alpha(@menu_border, {a})")) || css.contains(&format!("alpha(@menu_sel_bg, {a})")) || css.contains(&format!("alpha(@menu_sel_border, {a})")), "{a}");
        }
        assert!(css.contains("font-size: 16px;"));
    }

    #[test]
    fn hyprland_references_are_followed() {
        let css = render(t(r##"
            [hyprland]
            accent = "#abcdef"
            [menu]
            selected-text = "hyprland.accent"
            border = "hyprland.missing"
        "##));
        assert!(css.contains("@define-color menu_sel_text #abcdef;"));
        // dangling reference falls back to the token's default (the text color)
        assert!(css.contains("@define-color menu_border #cdd6f4;"));
    }

    #[test]
    fn gradients_collapse_to_first_color() {
        let css = render(t(r##"
            [menu]
            border = "rgba(1,2,3,1) #112233 #445566 45deg"
        "##));
        assert!(css.contains("@define-color menu_border #112233;"));
        let css = render(t(r##"
            [hyprland]
            g = "#aa0000 #00bb00 90deg"
            [menu]
            selected-border = "hyprland.g"
        "##));
        assert!(css.contains("@define-color menu_sel_border #aa0000;"));
    }

    #[test]
    fn values_without_a_hex_color_fall_back_to_default() {
        let css = render(t("[menu]\nbackground = \"red\""));
        assert!(css.contains("@define-color menu_bg #1e1e2e;"));
    }

    #[test]
    fn css_injection_attempts_fall_back_to_default() {
        for v in ["#fff;}window{opacity:0}", "#11223", "#gggggg", "#1e1e2e url(/etc/shadow)", "#1e1e2e9"] {
            let css = render(t(&format!("[menu]\nbackground = \"{v}\"")));
            assert!(css.contains("@define-color menu_bg #1e1e2e;"), "{v}");
            assert!(!css.contains("shadow") && !css.contains("opacity"), "{v}");
        }
    }

    #[test]
    fn hex_colors_of_every_valid_length_are_accepted() {
        for v in ["#abc", "#abcd", "#aabbcc", "#AABBCCDD"] {
            assert!(render(t(&format!("[menu]\nbackground = \"{v}\""))).contains(&format!("@define-color menu_bg {v};")), "{v}");
        }
    }

    #[test]
    fn alpha_is_clamped_and_accepts_integers() {
        let css = render(t("[menu]\nbackground-alpha = 7\nborder-alpha = -3\nselected-background-alpha = 1"));
        assert!(css.contains(".root { background: alpha(@menu_bg, 1);"));
        assert!(css.contains("border-color: alpha(@menu_border, 0);"));
        assert!(css.contains("background: alpha(@menu_sel_bg, 1);"));
    }

    #[test]
    fn wrong_typed_tokens_are_ignored() {
        let css = render(t("[menu]\nbackground = 5\nbackground-alpha = \"x\"\n[font]\nbase-size = \"big\""));
        assert!(css.contains("@define-color menu_bg #1e1e2e;"));
        assert!(css.contains("alpha(@menu_bg, 1)"));
        assert!(css.contains("font-size: 12px;"));
        // `menu` is not a table at all
        assert!(render(t("menu = 3")).contains("@define-color menu_bg #1e1e2e;"));
    }

    #[test]
    fn font_size_has_a_floor_and_accepts_floats() {
        assert!(render(t("[font]\nbase-size = 0")).contains("font-size: 1px;"));
        assert!(render(t("[font]\nbase-size = -4")).contains("font-size: 1px;"));
        assert!(render(t("[font]\nbase-size = 13.5")).contains("font-size: 13.5px;"));
    }

    #[test]
    fn user_keys_override_theme_keys_per_key() {
        let theme = t("[menu]\nbackground = \"#111111\"\ntext = \"#222222\"\n[font]\nbase-size = 10");
        let user = t("[menu]\ntext = \"#999999\"\n[extra]\nx = 1");
        let m = merge(theme, user);
        let menu = m["menu"].as_table().unwrap();
        assert_eq!(menu["background"].as_str(), Some("#111111"));
        assert_eq!(menu["text"].as_str(), Some("#999999"));
        assert_eq!(m["font"]["base-size"].as_integer(), Some(10));
        assert_eq!(m["extra"]["x"].as_integer(), Some(1));
    }

    #[test]
    fn user_non_table_replaces_theme_section() {
        let m = merge(t("[menu]\ntext = \"#222222\""), t("menu = 1"));
        assert_eq!(m["menu"].as_integer(), Some(1));
    }

    #[test]
    fn css_reads_theme_and_user_files() {
        let tmp = TempDir::new("theme-files");
        with_home(tmp.path(), || {
            let theme = state_dir().join("omarchy/current/theme");
            fs::create_dir_all(&theme).unwrap();
            fs::write(theme.join("shell.toml"), "[menu]\nbackground = \"#101010\"\ntext = \"#202020\"").unwrap();
            let user = config_dir().join("omarchy");
            fs::create_dir_all(&user).unwrap();
            fs::write(user.join("shell.toml"), "[menu]\ntext = \"#303030\"").unwrap();
            let css = css();
            assert!(css.contains("@define-color menu_bg #101010;"));
            assert!(css.contains("@define-color menu_text #303030;"));
        });
    }

    #[test]
    fn css_survives_missing_or_malformed_files() {
        let tmp = TempDir::new("theme-bad");
        with_home(tmp.path(), || {
            assert!(css().contains("@define-color menu_bg #1e1e2e;"));
            let theme = state_dir().join("omarchy/current/theme");
            fs::create_dir_all(&theme).unwrap();
            fs::write(theme.join("shell.toml"), "this is = = not toml").unwrap();
            assert!(css().contains("@define-color menu_bg #1e1e2e;"));
        });
        with_env(&[("HOME", Some(OsStr::new("/nonexistent-omaroute"))), ("XDG_CONFIG_HOME", None), ("XDG_STATE_HOME", None)], || {
            assert!(css().contains("font-size: 12px;"));
        });
    }
}
