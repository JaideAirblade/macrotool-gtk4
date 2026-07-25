//! Macros tab — edit macro list for the active spec.
//! Every widget is wired to write back to the config on change.

use crate::config::{ConfigTree, Macro, Spec};
use crate::ui::key_capture::KeyCapture;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, ComboBoxText, Entry, Label, Orientation, ScrolledWindow, SpinButton, Switch,
};
use std::sync::Arc;

pub struct MacrosTab {
    container: gtk4::Box,
    content_box: gtk4::Box,
}

impl MacrosTab {
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

        let tab = MacrosTab { container, content_box };
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

/// Rebuild the whole tab (header + macro cards) from config.
pub fn render(
    content_box: &gtk4::Box,
    cfg: &Arc<crate::config::Manager>,
    engine: &Arc<crate::engine::EngineHub>,
) {
    while let Some(child) = content_box.first_child() {
        content_box.remove(&child);
    }

    let tree = cfg.tree();

    // Header
    let header = GtkBox::new(Orientation::Horizontal, 12);
    header.set_hexpand(true);
    let title = Label::new(Some(&format!("Macros — {}", tree.active_spec)));
    title.add_css_class("tab-title");
    title.set_hexpand(true);
    title.set_halign(gtk4::Align::Start);
    header.append(&title);

    let content_for_add = content_box.clone();
    let cfg_add = cfg.clone();
    let engine_add = engine.clone();
    let add_btn = Button::with_label("+ Add Macro");
    add_btn.add_css_class("suggested-action");
    add_btn.connect_clicked(move |_| {
        add_macro(&cfg_add);
        engine_add.reload_profile();
        render(&content_for_add, &cfg_add, &engine_add);
    });
    header.append(&add_btn);
    content_box.append(&header);

    let spec = match get_active_spec(&tree) {
        Some(s) => s,
        None => {
            let empty = Label::new(Some("Select a spec to edit macros"));
            empty.add_css_class("dim-label");
            content_box.append(&empty);
            return;
        }
    };

    if spec.macros.is_empty() {
        let empty = Label::new(Some("No macros yet — click + Add Macro"));
        empty.add_css_class("dim-label");
        content_box.append(&empty);
        return;
    }

    for (idx, m) in spec.macros.iter().enumerate() {
        content_box.append(&make_macro_card(idx, m, cfg, engine, content_box));
    }
}

// ── Config mutation helpers ──────────────────────────────────────────────

/// Run a mutation on the active spec and persist the tree.
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
    cfg.set_tree(tree); // set_tree schedules a debounced save
}

/// Mutate one macro by index.
fn update_macro<F: FnOnce(&mut Macro)>(cfg: &Arc<crate::config::Manager>, idx: usize, f: F) {
    with_spec_mut(cfg, |spec| {
        if let Some(m) = spec.macros.get_mut(idx) {
            f(m);
        }
    });
}

fn add_macro(cfg: &Arc<crate::config::Manager>) {
    let delay = cfg.settings().default_delay;
    with_spec_mut(cfg, |spec| {
        spec.macros.push(Macro {
            name: "New Macro".into(),
            hotkey: String::new(),
            delay,
            mode: "press".into(),
            hold_mode: false,
            keys: Vec::new(),
            inter_key_delay: 0,
            enabled: true,
            max_hold_duration: 0,
            background: false,
        });
    });
}

fn delete_macro(cfg: &Arc<crate::config::Manager>, idx: usize) {
    with_spec_mut(cfg, |spec| {
        if idx < spec.macros.len() {
            spec.macros.remove(idx);
        }
    });
}

// ── Card builder ─────────────────────────────────────────────────────────

fn make_macro_card(
    idx: usize,
    m: &Macro,
    cfg: &Arc<crate::config::Manager>,
    engine: &Arc<crate::engine::EngineHub>,
    content_box: &gtk4::Box,
) -> gtk4::Box {
    let card = gtk4::Box::new(Orientation::Vertical, 8);
    card.add_css_class("card");
    card.set_margin_bottom(8);

    // ── Header row: name entry, ON/OFF badge, delete ──
    let header = GtkBox::new(Orientation::Horizontal, 8);

    let name_entry = Entry::new();
    name_entry.set_text(&m.name);
    name_entry.set_hexpand(true);
    {
        let cfg = cfg.clone();
        name_entry.connect_changed(move |e| {
            let text = e.text().to_string();
            update_macro(&cfg, idx, |m| m.name = text);
        });
    }
    header.append(&name_entry);

    let badge = Label::new(Some(if m.enabled { "ON" } else { "OFF" }));
    header.append(&badge);

    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        let content_box = content_box.clone();
        let delete_btn = Button::with_label("✕");
        delete_btn.set_tooltip_text(Some("Delete"));
        delete_btn.connect_clicked(move |_| {
            delete_macro(&cfg, idx);
            engine.reload_profile();
            render(&content_box, &cfg, &engine);
        });
        header.append(&delete_btn);
    }
    card.append(&header);

    // ── Grid of fields ──
    let grid = gtk4::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(8);

    // Hotkey (key capture)
    let hotkey_lbl = Label::new(Some("Hotkey"));
    hotkey_lbl.set_halign(gtk4::Align::Start);
    let hotkey_cap = KeyCapture::new(&m.hotkey, {
        let cfg = cfg.clone();
        let engine = engine.clone();
        Box::new(move |key: &str| {
            let key = key.to_string();
            update_macro(&cfg, idx, |m| m.hotkey = key);
            engine.reload_profile();
        })
    });
    hotkey_cap.widget().set_hexpand(true);
    grid.attach(&hotkey_lbl, 0, 0, 1, 1);
    grid.attach(hotkey_cap.widget(), 1, 0, 1, 1);

    // Mode
    let mode_lbl = Label::new(Some("Mode"));
    mode_lbl.set_halign(gtk4::Align::Start);
    let mode_combo = ComboBoxText::new();
    mode_combo.append(Some("press"), "Press");
    mode_combo.append(Some("hold"), "Hold");
    mode_combo.append(Some("toggle"), "Toggle");
    mode_combo.set_active_id(Some(match m.mode.as_str() {
        "hold" => "hold",
        "toggle" => "toggle",
        _ => "press",
    }));
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        mode_combo.connect_changed(move |c| {
            if let Some(id) = c.active_id() {
                let mode = id.to_string();
                update_macro(&cfg, idx, |m| m.mode = mode);
                engine.reload_profile();
            }
        });
    }
    grid.attach(&mode_lbl, 2, 0, 1, 1);
    grid.attach(&mode_combo, 3, 0, 1, 1);

    // Delay
    let delay_lbl = Label::new(Some("Delay (ms)"));
    delay_lbl.set_halign(gtk4::Align::Start);
    let delay_spin = SpinButton::with_range(0.0, 100000.0, 1.0);
    delay_spin.set_value(m.delay as f64);
    {
        let cfg = cfg.clone();
        delay_spin.connect_value_changed(move |s| {
            let v = s.value() as i32;
            update_macro(&cfg, idx, |m| m.delay = v);
        });
    }
    grid.attach(&delay_lbl, 0, 1, 1, 1);
    grid.attach(&delay_spin, 1, 1, 1, 1);

    // Inter-key delay
    let ikd_lbl = Label::new(Some("Inter-Key Delay"));
    ikd_lbl.set_halign(gtk4::Align::Start);
    let ikd_spin = SpinButton::with_range(0.0, 100000.0, 1.0);
    ikd_spin.set_value(m.inter_key_delay as f64);
    {
        let cfg = cfg.clone();
        ikd_spin.connect_value_changed(move |s| {
            let v = s.value() as i32;
            update_macro(&cfg, idx, |m| m.inter_key_delay = v);
        });
    }
    grid.attach(&ikd_lbl, 2, 1, 1, 1);
    grid.attach(&ikd_spin, 3, 1, 1, 1);

    card.append(&grid);

    // ── Toggles ──
    let toggles = GtkBox::new(Orientation::Horizontal, 24);

    let en_box = GtkBox::new(Orientation::Horizontal, 8);
    en_box.append(&Label::new(Some("Enabled")));
    let en_sw = Switch::new();
    en_sw.set_active(m.enabled);
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        en_sw.connect_state_set(move |_, state| {
            update_macro(&cfg, idx, |m| m.enabled = state);
            engine.reload_profile();
            glib::Propagation::Proceed
        });
    }
    en_box.append(&en_sw);
    toggles.append(&en_box);

    let bg_box = GtkBox::new(Orientation::Horizontal, 8);
    bg_box.append(&Label::new(Some("Background")));
    let bg_sw = Switch::new();
    bg_sw.set_active(m.background);
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        bg_sw.connect_state_set(move |_, state| {
            update_macro(&cfg, idx, |m| m.background = state);
            engine.reload_profile();
            glib::Propagation::Proceed
        });
    }
    bg_box.append(&bg_sw);
    toggles.append(&bg_box);
    card.append(&toggles);

    // ── Keys list ──
    let keys_header = GtkBox::new(Orientation::Horizontal, 8);
    keys_header.append(&Label::new(Some("Keys")));
    keys_header.set_halign(gtk4::Align::Start);
    card.append(&keys_header);

    let keys_box = GtkBox::new(Orientation::Horizontal, 6);
    keys_box.set_halign(gtk4::Align::Start);
    for (ki, k) in m.keys.iter().enumerate() {
        let chip = Button::with_label(&format!("{} ✕", k));
        {
            let cfg = cfg.clone();
            let engine = engine.clone();
            let content_box = content_box.clone();
            chip.connect_clicked(move |_| {
                update_macro(&cfg, idx, |m| {
                    if ki < m.keys.len() {
                        m.keys.remove(ki);
                    }
                });
                engine.reload_profile();
                render(&content_box, &cfg, &engine);
            });
        }
        keys_box.append(&chip);
    }
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        let content_box = content_box.clone();
        let add_key_btn = Button::with_label("+ Key");
        add_key_btn.connect_clicked(move |_| {
            // Append a placeholder key the user can edit in config or via picker later
            update_macro(&cfg, idx, |m| m.keys.push("a".into()));
            engine.reload_profile();
            render(&content_box, &cfg, &engine);
        });
        keys_box.append(&add_key_btn);
    }
    card.append(&keys_box);

    card
}

pub fn get_active_spec(tree: &ConfigTree) -> Option<Spec> {
    let game = tree.games.get(&tree.active_game)?;
    let class = game.classes.get(&tree.active_class)?;
    class.specs.get(&tree.active_spec).cloned()
}