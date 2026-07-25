//! Buff timers tab — edit buff timer list for the active spec.
//! Every widget writes back to the config on change.

use crate::config::{BuffTimer, Spec};
use crate::ui::key_capture::KeyCapture;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, ComboBoxText, Entry, Label, Orientation, ScrolledWindow, SpinButton, Switch,
};
use std::sync::Arc;

pub struct BuffsTab {
    container: gtk4::Box,
    content_box: gtk4::Box,
}

impl BuffsTab {
    pub fn new(cfg: Arc<crate::config::Manager>, engine: Arc<crate::engine::EngineHub>) -> Self {
        let container = gtk4::Box::new(Orientation::Vertical, 0);

        let scrolled = ScrolledWindow::new();
        scrolled.set_hexpand(true);
        scrolled.set_vexpand(true);

        let content_box = gtk4::Box::new(Orientation::Vertical, 12);
        content_box.set_margin_start(16);
        content_box.set_margin_end(16);
        content_box.set_margin_top(16);
        content_box.set_margin_bottom(16);

        scrolled.set_child(Some(&content_box));
        container.append(&scrolled);

        let tab = BuffsTab { container, content_box };
        tab.refresh(&cfg, &engine);
        tab
    }

    pub fn refresh(&self, cfg: &Arc<crate::config::Manager>, engine: &Arc<crate::engine::EngineHub>) {
        render(&self.content_box, cfg, engine);
    }

    pub fn widget(&self) -> &gtk4::Widget {
        self.container.upcast_ref()
    }
}

pub fn render(
    content_box: &gtk4::Box,
    cfg: &Arc<crate::config::Manager>,
    engine: &Arc<crate::engine::EngineHub>,
) {
    while let Some(child) = content_box.first_child() {
        content_box.remove(&child);
    }

    let tree = cfg.tree();

    let header = GtkBox::new(Orientation::Horizontal, 12);
    header.set_hexpand(true);
    let title = Label::new(Some(&format!("Buff Timers — {}", tree.active_spec)));
    title.add_css_class("tab-title");
    title.set_hexpand(true);
    title.set_halign(gtk4::Align::Start);
    header.append(&title);

    let content_for_add = content_box.clone();
    let cfg_add = cfg.clone();
    let engine_add = engine.clone();
    let add_btn = Button::with_label("+ Add Buff");
    add_btn.add_css_class("suggested-action");
    add_btn.connect_clicked(move |_| {
        add_buff(&cfg_add);
        engine_add.reload_profile();
        render(&content_for_add, &cfg_add, &engine_add);
    });
    header.append(&add_btn);
    content_box.append(&header);

    let spec = match super::macros_tab::get_active_spec(&tree) {
        Some(s) => s,
        None => {
            let empty = Label::new(Some("Select a spec"));
            empty.add_css_class("dim-label");
            content_box.append(&empty);
            return;
        }
    };

    if spec.buff_timers.is_empty() {
        let empty = Label::new(Some("No buff timers"));
        empty.add_css_class("dim-label");
        content_box.append(&empty);
        return;
    }

    for (idx, b) in spec.buff_timers.iter().enumerate() {
        content_box.append(&make_buff_card(idx, b, cfg, engine, content_box));
    }
}

// ── Config mutation helpers ──────────────────────────────────────────────

fn with_spec_mut<F: FnOnce(&mut Spec)>(cfg: &Arc<crate::config::Manager>, f: F) {
    let mut tree = cfg.tree();
    let g = tree.active_game.clone();
    let c = tree.active_class.clone();
    let s = tree.active_spec.clone();
    if let Some(game) = tree.games.get_mut(&g) {
        if let Some(class) = game.classes.get_mut(&c) {
            if let Some(spec) = class.specs.get_mut(&s) {
                f(spec);
            }
        }
    }
    cfg.set_tree(tree);
}

fn update_buff<F: FnOnce(&mut BuffTimer)>(cfg: &Arc<crate::config::Manager>, idx: usize, f: F) {
    with_spec_mut(cfg, |spec| {
        if let Some(b) = spec.buff_timers.get_mut(idx) {
            f(b);
        }
    });
}

fn add_buff(cfg: &Arc<crate::config::Manager>) {
    with_spec_mut(cfg, |spec| {
        spec.buff_timers.push(BuffTimer {
            name: "New Buff".into(),
            watch_keys: Vec::new(),
            duration: 5000,
            action_key: String::new(),
            on_refresh: "reset".into(),
            extend_ms: 0,
            enabled: true,
            trigger_type: "keys".into(),
            trigger_pixels: Vec::new(),
            trigger_match_mode: "all".into(),
            capture_res: None,
        });
    });
}

fn delete_buff(cfg: &Arc<crate::config::Manager>, idx: usize) {
    with_spec_mut(cfg, |spec| {
        if idx < spec.buff_timers.len() {
            spec.buff_timers.remove(idx);
        }
    });
}

// ── Card builder ─────────────────────────────────────────────────────────

fn make_buff_card(
    idx: usize,
    b: &BuffTimer,
    cfg: &Arc<crate::config::Manager>,
    engine: &Arc<crate::engine::EngineHub>,
    content_box: &gtk4::Box,
) -> gtk4::Box {
    let card = gtk4::Box::new(Orientation::Vertical, 8);
    card.add_css_class("card");
    card.set_margin_bottom(8);

    // Header
    let header = GtkBox::new(Orientation::Horizontal, 8);
    let name_entry = Entry::new();
    name_entry.set_text(&b.name);
    name_entry.set_hexpand(true);
    {
        let cfg = cfg.clone();
        name_entry.connect_changed(move |e| {
            let text = e.text().to_string();
            update_buff(&cfg, idx, |b| b.name = text);
        });
    }
    header.append(&name_entry);

    let badge = Label::new(Some(if b.enabled { "ON" } else { "OFF" }));
    header.append(&badge);

    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        let content_box = content_box.clone();
        let delete_btn = Button::with_label("✕");
        delete_btn.connect_clicked(move |_| {
            delete_buff(&cfg, idx);
            engine.reload_profile();
            render(&content_box, &cfg, &engine);
        });
        header.append(&delete_btn);
    }
    card.append(&header);

    // Grid
    let grid = gtk4::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(8);

    let dur_lbl = Label::new(Some("Duration (ms)"));
    dur_lbl.set_halign(gtk4::Align::Start);
    let dur_spin = SpinButton::with_range(0.0, 999999.0, 100.0);
    dur_spin.set_value(b.duration as f64);
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        dur_spin.connect_value_changed(move |s| {
            let v = s.value() as i32;
            update_buff(&cfg, idx, |b| b.duration = v);
            engine.reload_profile();
        });
    }
    grid.attach(&dur_lbl, 0, 0, 1, 1);
    grid.attach(&dur_spin, 1, 0, 1, 1);

    let key_lbl = Label::new(Some("Action Key"));
    key_lbl.set_halign(gtk4::Align::Start);
    let key_cap = KeyCapture::new(&b.action_key, {
        let cfg = cfg.clone();
        let engine = engine.clone();
        Box::new(move |key: &str| {
            let key = key.to_string();
            update_buff(&cfg, idx, |b| b.action_key = key);
            engine.reload_profile();
        })
    });
    grid.attach(&key_lbl, 2, 0, 1, 1);
    grid.attach(key_cap.widget(), 3, 0, 1, 1);

    let refresh_lbl = Label::new(Some("On Refresh"));
    refresh_lbl.set_halign(gtk4::Align::Start);
    let refresh_combo = ComboBoxText::new();
    refresh_combo.append(Some("reset"), "Reset");
    refresh_combo.append(Some("extend"), "Extend");
    refresh_combo.append(Some("ignore"), "Ignore");
    refresh_combo.set_active_id(Some(match b.on_refresh.as_str() {
        "extend" => "extend",
        "ignore" => "ignore",
        _ => "reset",
    }));
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        refresh_combo.connect_changed(move |c| {
            if let Some(id) = c.active_id() {
                let v = id.to_string();
                update_buff(&cfg, idx, |b| b.on_refresh = v);
                engine.reload_profile();
            }
        });
    }
    grid.attach(&refresh_lbl, 0, 1, 1, 1);
    grid.attach(&refresh_combo, 1, 1, 1, 1);

    let ext_lbl = Label::new(Some("Extend (ms)"));
    ext_lbl.set_halign(gtk4::Align::Start);
    let ext_spin = SpinButton::with_range(0.0, 999999.0, 100.0);
    ext_spin.set_value(b.extend_ms as f64);
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        ext_spin.connect_value_changed(move |s| {
            let v = s.value() as i32;
            update_buff(&cfg, idx, |b| b.extend_ms = v);
            engine.reload_profile();
        });
    }
    grid.attach(&ext_lbl, 2, 1, 1, 1);
    grid.attach(&ext_spin, 3, 1, 1, 1);

    card.append(&grid);

    // Toggles
    let toggles = GtkBox::new(Orientation::Horizontal, 24);

    let en_box = GtkBox::new(Orientation::Horizontal, 8);
    en_box.append(&Label::new(Some("Enabled")));
    let en_sw = Switch::new();
    en_sw.set_active(b.enabled);
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        en_sw.connect_state_set(move |_, state| {
            update_buff(&cfg, idx, |b| b.enabled = state);
            engine.reload_profile();
            glib::Propagation::Proceed
        });
    }
    en_box.append(&en_sw);
    toggles.append(&en_box);

    let trig_box = GtkBox::new(Orientation::Horizontal, 8);
    trig_box.append(&Label::new(Some("Trigger")));
    let trig_combo = ComboBoxText::new();
    trig_combo.append(Some("keys"), "Keys");
    trig_combo.append(Some("pixel"), "Pixel");
    trig_combo.set_active_id(Some(if b.trigger_type == "pixel" { "pixel" } else { "keys" }));
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        trig_combo.connect_changed(move |c| {
            if let Some(id) = c.active_id() {
                let v = id.to_string();
                update_buff(&cfg, idx, |b| b.trigger_type = v);
                engine.reload_profile();
            }
        });
    }
    trig_box.append(&trig_combo);
    toggles.append(&trig_box);
    card.append(&toggles);

    // Watch keys
    let wk_label = Label::new(Some("Watch Keys"));
    wk_label.set_halign(gtk4::Align::Start);
    card.append(&wk_label);

    let wk_box = GtkBox::new(Orientation::Horizontal, 4);
    wk_box.set_halign(gtk4::Align::Start);
    for (ki, k) in b.watch_keys.iter().enumerate() {
        let chip = Button::with_label(&format!("{} ✕", k));
        {
            let cfg = cfg.clone();
            let engine = engine.clone();
            let content_box = content_box.clone();
            chip.connect_clicked(move |_| {
                update_buff(&cfg, idx, |b| {
                    if ki < b.watch_keys.len() {
                        b.watch_keys.remove(ki);
                    }
                });
                engine.reload_profile();
                render(&content_box, &cfg, &engine);
            });
        }
        wk_box.append(&chip);
    }
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        let content_box = content_box.clone();
        let add_wk_btn = Button::with_label("+ Key");
        add_wk_btn.connect_clicked(move |btn| {
            let cfg = cfg.clone();
            let engine = engine.clone();
            let content_box = content_box.clone();
            super::key_capture::show_capture_dialog(btn, move |key: &str| {
                let key = key.to_string();
                update_buff(&cfg, idx, |b| b.watch_keys.push(key));
                engine.reload_profile();
                render(&content_box, &cfg, &engine);
            });
        });
        wk_box.append(&add_wk_btn);
    }
    card.append(&wk_box);

    card
}