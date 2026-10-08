#![allow(deprecated)]
use gtk4::{self as gtk, gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};
use std::{cell::{Cell, RefCell}, rc::Rc};


#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Scope {
    Once,
    App,
    AppDomain,
    Domain,
}

pub struct Browser {
    pub id: String,
    pub name: String,
    pub icon: Option<gio::Icon>,
}

const STRUCTURE_CSS: &str = include_str!("../data/picker.css");

fn css() -> String {
    format!("{STRUCTURE_CSS}\n{}", crate::theme::css())
}

/// Show the picker; None if cancelled.
pub fn run(app: Option<&str>, host: &str, browsers: Vec<Browser>) -> Option<(String, Scope)> {
    let result: Rc<RefCell<Option<(String, Scope)>>> = Rc::default();
    let out = result.clone();
    let (app, host) = (app.map(str::to_owned), host.to_owned());
    let browsers = Rc::new(browsers);

    let application = gtk::Application::builder()
        .application_id("dev.omaroute.picker")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    application.connect_activate(move |a| build(a, app.as_deref(), &host, browsers.clone(), out.clone()));
    application.run_with_args::<&str>(&[]);
    result.take()
}

/// Human-friendly app name: `chrome-app.hey.com__-Default` -> `Hey`, `slack` -> `Slack`.
fn display_name(app: &str) -> String {
    let name = match app.split_once("__") {
        Some((head, _)) => {
            let host = head.split_once('-').map_or(head, |(_, h)| h);
            let labels: Vec<&str> = host.split('.').collect();
            labels.get(labels.len().saturating_sub(2)).copied().unwrap_or(host).to_owned()
        }
        None => app.to_owned(),
    };
    let mut c = name.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

/// The "remember this choice" options, in display order; the second (index 1) is preselected.
fn scopes_for(app: Option<&str>, host: &str) -> Vec<(String, Scope)> {
    match app {
        Some(a) => {
            let a = display_name(a);
            vec![
                ("Just this once".into(), Scope::Once),
                (format!("All links from {a}"), Scope::App),
                (format!("Only {host} links from {a}"), Scope::AppDomain),
            ]
        }
        None => vec![("Just this once".into(), Scope::Once), (format!("All {host} links"), Scope::Domain)],
    }
}

fn hint(browsers: usize) -> String {
    let n = browsers.min(9);
    let range = if n == 1 { "1".to_owned() } else { format!("1-{n}") };
    format!("↑↓ select · {range} open · Tab scope · Esc cancel")
}

fn build(
    application: &gtk::Application,
    app: Option<&str>,
    host: &str,
    browsers: Rc<Vec<Browser>>,
    result: Rc<RefCell<Option<(String, Scope)>>>,
) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css());
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    let window = gtk::ApplicationWindow::builder().application(application).resizable(false).build();
    window.add_css_class("omaroute");
    // No anchors: the compositor centers the surface on both axes.
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_namespace(Some("omaroute"));

    let scopes = scopes_for(app, host);
    let scope_idx = Rc::new(Cell::new(1));
    let selected = Rc::new(Cell::new(0usize));

    let root = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.add_css_class("root");
    let title = gtk::Label::builder()
        .label(app.map_or(format!("Open {host} link with"), |a| format!("Open {host} link from {} with", display_name(a))))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(50)
        .css_classes(["title"])
        .build();
    root.append(&title);

    let buttons: Rc<Vec<gtk::Button>> = Rc::new(browsers.iter().enumerate().map(|(i, b)| {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        if let Some(icon) = &b.icon {
            let img = gtk::Image::from_gicon(icon);
            img.set_pixel_size(24);
            row.append(&img);
        }
        row.append(&gtk::Label::builder().label(b.name.as_str()).xalign(0.0).hexpand(true).build());
        if i < 9 {
            row.append(&gtk::Label::builder().label((i + 1).to_string()).css_classes(["num"]).build());
        }
        let btn = gtk::Button::builder().child(&row).focusable(false).css_classes(["browser"]).build();
        root.append(&btn);
        btn
    }).collect());

    let refresh = {
        let (buttons, selected) = (buttons.clone(), selected.clone());
        move || for (i, b) in buttons.iter().enumerate() {
            if i == selected.get() { b.add_css_class("selected") } else { b.remove_css_class("selected") }
        }
    };
    refresh();

    let scope_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    scope_box.add_css_class("scopes");
    root.append(&scope_box);
    let radios: Rc<Vec<gtk::CheckButton>> = Rc::new({
        let mut v: Vec<gtk::CheckButton> = Vec::new();
        for (i, (label, _)) in scopes.iter().enumerate() {
            let r = gtk::CheckButton::builder().label(label.as_str()).focusable(false).build();
            if let Some(first) = v.first() { r.set_group(Some(first)) }
            r.set_active(i == scope_idx.get());
            let idx = scope_idx.clone();
            r.connect_toggled(move |r| if r.is_active() { idx.set(i) });
            scope_box.append(&r);
            v.push(r);
        }
        v
    });
    root.append(&gtk::Label::builder().label(hint(browsers.len())).xalign(0.0).css_classes(["hint"]).build());
    window.set_child(Some(&root));

    let finish: Rc<dyn Fn(usize)> = {
        let (window, browsers, scopes, scope_idx) = (window.clone(), browsers.clone(), scopes.clone(), scope_idx.clone());
        Rc::new(move |i| {
            *result.borrow_mut() = Some((browsers[i].id.clone(), scopes[scope_idx.get()].1));
            window.close();
        })
    };
    for (i, b) in buttons.iter().enumerate() {
        let f = finish.clone();
        b.connect_clicked(move |_| f(i));
    }

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let win = window.clone();
    let n = browsers.len();
    keys.connect_key_pressed(move |_, key, _, _| {
        match key {
            gdk::Key::Escape => win.close(),
            gdk::Key::Return | gdk::Key::KP_Enter => finish(selected.get()),
            gdk::Key::Down => { selected.set((selected.get() + 1) % n); refresh() }
            gdk::Key::Up => { selected.set((selected.get() + n - 1) % n); refresh() }
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => radios[(scope_idx.get() + 1) % radios.len()].set_active(true),
            k => match k.to_unicode().and_then(|c| c.to_digit(10)) {
                Some(d @ 1..=9) if (d as usize) <= n => finish(d as usize - 1),
                _ => return glib::Propagation::Proceed,
            },
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_plain_class() {
        assert_eq!(display_name("slack"), "Slack");
        assert_eq!(display_name("Slack"), "Slack");
        assert_eq!(display_name(""), "");
        assert_eq!(display_name("ünicode"), "Ünicode");
    }

    #[test]
    fn display_name_chrome_pwa_class() {
        assert_eq!(display_name("chrome-app.hey.com__-Default"), "Hey");
        assert_eq!(display_name("chrome-mail.google.com__-Profile 1"), "Google");
        assert_eq!(display_name("chrome-localhost__-Default"), "Localhost");
        assert_eq!(display_name("chrome-hey.com__x"), "Hey");
    }

    #[test]
    fn display_name_without_dash_prefix() {
        assert_eq!(display_name("app.hey.com__x"), "Hey");
    }

    #[test]
    fn scopes_with_app() {
        let s = scopes_for(Some("slack"), "github.com");
        let labels: Vec<_> = s.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Just this once", "All links from Slack", "Only github.com links from Slack"]);
        let kinds: Vec<_> = s.iter().map(|(_, k)| *k).collect();
        assert_eq!(kinds, [Scope::Once, Scope::App, Scope::AppDomain]);
    }

    #[test]
    fn scopes_without_app() {
        let s = scopes_for(None, "github.com");
        assert_eq!(s, vec![("Just this once".to_string(), Scope::Once), ("All github.com links".to_string(), Scope::Domain)]);
    }

    #[test]
    fn preselected_scope_index_is_valid_and_remembers() {
        // build() preselects index 1, so it must exist and never be "once".
        for app in [Some("a"), None] {
            let s = scopes_for(app, "x.com");
            assert!(s.len() > 1);
            assert_ne!(s[1].1, Scope::Once);
        }
    }

    #[test]
    fn hint_range_tracks_browser_count_capped_at_nine() {
        assert!(hint(1).contains(" 1 open"));
        assert!(hint(2).contains("1-2 open"));
        assert!(hint(9).contains("1-9 open"));
        assert!(hint(30).contains("1-9 open"));
    }

    #[test]
    fn structural_css_defines_classes_the_theme_targets() {
        for c in [".root", "button.browser", ".selected", ".title", ".hint", ".num", ".scopes"] {
            assert!(STRUCTURE_CSS.contains(c), "{c}");
        }
    }
}
