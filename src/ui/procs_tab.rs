//! Pixel triggers tab — edit pixel trigger list for the active spec.
//! Every widget writes back to the config on change.

use crate::config::{Pixel, PixelTrigger, Spec};
use crate::ui::key_capture::KeyCapture;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, ComboBoxText, Entry, Label, Orientation, ScrolledWindow, SpinButton, Switch,
};
use std::sync::Arc;

pub struct ProcsTab {
    container: gtk4::Box,
    content_box: gtk4::Box,
}

impl ProcsTab {
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

        let tab = ProcsTab { container, content_box };
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
    let title = Label::new(Some(&format!("Pixel Triggers — {}", tree.active_spec)));
    title.add_css_class("tab-title");
    title.set_hexpand(true);
    title.set_halign(gtk4::Align::Start);
    header.append(&title);

    let content_for_add = content_box.clone();
    let cfg_add = cfg.clone();
    let engine_add = engine.clone();
    let add_btn = Button::with_label("+ Add Trigger");
    add_btn.add_css_class("suggested-action");
    add_btn.connect_clicked(move |_| {
        add_trigger(&cfg_add);
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

    if spec.pixel_triggers.is_empty() {
        let empty = Label::new(Some("No pixel triggers"));
        empty.add_css_class("dim-label");
        content_box.append(&empty);
        return;
    }

    for (idx, t) in spec.pixel_triggers.iter().enumerate() {
        content_box.append(&make_trigger_card(idx, t, cfg, engine, content_box));
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

fn update_trigger<F: FnOnce(&mut PixelTrigger)>(cfg: &Arc<crate::config::Manager>, idx: usize, f: F) {
    with_spec_mut(cfg, |spec| {
        if let Some(t) = spec.pixel_triggers.get_mut(idx) {
            f(t);
        }
    });
}

fn add_trigger(cfg: &Arc<crate::config::Manager>) {
    with_spec_mut(cfg, |spec| {
        spec.pixel_triggers.push(PixelTrigger {
            name: "New Trigger".into(),
            action_key: String::new(),
            pixels: Vec::new(),
            match_mode: "all".into(),
            inverse: false,
            enabled: true,
            cooldown: 1000,
            last_fired: 0,
            trigger_mode: "macro".into(),
            macro_hotkey: String::new(),
            capture_res: None,
            anchor: None,
            blocker: None,
        });
    });
}

fn delete_trigger(cfg: &Arc<crate::config::Manager>, idx: usize) {
    with_spec_mut(cfg, |spec| {
        if idx < spec.pixel_triggers.len() {
            spec.pixel_triggers.remove(idx);
        }
    });
}

// ── Card builder ─────────────────────────────────────────────────────────

fn make_trigger_card(
    idx: usize,
    t: &PixelTrigger,
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
    name_entry.set_text(&t.name);
    name_entry.set_hexpand(true);
    {
        let cfg = cfg.clone();
        name_entry.connect_changed(move |e| {
            let text = e.text().to_string();
            update_trigger(&cfg, idx, |t| t.name = text);
        });
    }
    header.append(&name_entry);

    let badge = Label::new(Some(if t.enabled { "ON" } else { "OFF" }));
    badge.add_css_class("badge");
    if !t.enabled {
        badge.add_css_class("badge-off");
    }
    header.append(&badge);

    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        let content_box = content_box.clone();
        let delete_btn = Button::with_label("✕");
        delete_btn.connect_clicked(move |_| {
            delete_trigger(&cfg, idx);
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

    let action_lbl = Label::new(Some("Action Key"));
    action_lbl.set_halign(gtk4::Align::Start);
    let action_cap = KeyCapture::new(&t.action_key, {
        let cfg = cfg.clone();
        let engine = engine.clone();
        Box::new(move |key: &str| {
            let key = key.to_string();
            update_trigger(&cfg, idx, |t| t.action_key = key);
            engine.reload_profile();
        })
    });
    grid.attach(&action_lbl, 0, 0, 1, 1);
    grid.attach(action_cap.widget(), 1, 0, 1, 1);

    let match_lbl = Label::new(Some("Match Mode"));
    match_lbl.set_halign(gtk4::Align::Start);
    let match_combo = ComboBoxText::new();
    match_combo.append(Some("all"), "All");
    match_combo.append(Some("any"), "Any");
    match_combo.set_active_id(Some(if t.match_mode == "any" { "any" } else { "all" }));
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        match_combo.connect_changed(move |c| {
            if let Some(id) = c.active_id() {
                let v = id.to_string();
                update_trigger(&cfg, idx, |t| t.match_mode = v);
                engine.reload_profile();
            }
        });
    }
    grid.attach(&match_lbl, 2, 0, 1, 1);
    grid.attach(&match_combo, 3, 0, 1, 1);

    let cd_lbl = Label::new(Some("Cooldown (ms)"));
    cd_lbl.set_halign(gtk4::Align::Start);
    let cd_spin = SpinButton::with_range(0.0, 100000.0, 1.0);
    cd_spin.set_value(t.cooldown as f64);
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        cd_spin.connect_value_changed(move |s| {
            let v = s.value() as i32;
            update_trigger(&cfg, idx, |t| t.cooldown = v);
            engine.reload_profile();
        });
    }
    grid.attach(&cd_lbl, 0, 1, 1, 1);
    grid.attach(&cd_spin, 1, 1, 1, 1);

    let tm_lbl = Label::new(Some("Trigger Mode"));
    tm_lbl.set_halign(gtk4::Align::Start);
    let tm_combo = ComboBoxText::new();
    tm_combo.append(Some("macro"), "Only when macro running");
    tm_combo.append(Some("always"), "Always");
    tm_combo.set_active_id(Some(if t.trigger_mode == "always" { "always" } else { "macro" }));
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        tm_combo.connect_changed(move |c| {
            if let Some(id) = c.active_id() {
                let v = id.to_string();
                update_trigger(&cfg, idx, |t| t.trigger_mode = v);
                engine.reload_profile();
            }
        });
    }
    grid.attach(&tm_lbl, 2, 1, 1, 1);
    grid.attach(&tm_combo, 3, 1, 1, 1);

    // Macro hotkey filter
    let mh_lbl = Label::new(Some("Macro Hotkey"));
    mh_lbl.set_halign(gtk4::Align::Start);
    let mh_cap = KeyCapture::new(&t.macro_hotkey, {
        let cfg = cfg.clone();
        let engine = engine.clone();
        Box::new(move |key: &str| {
            let key = key.to_string();
            update_trigger(&cfg, idx, |t| t.macro_hotkey = key);
            engine.reload_profile();
        })
    });
    grid.attach(&mh_lbl, 0, 2, 1, 1);
    grid.attach(mh_cap.widget(), 1, 2, 1, 1);

    card.append(&grid);

    // Toggles
    let toggles = GtkBox::new(Orientation::Horizontal, 24);

    let en_box = GtkBox::new(Orientation::Horizontal, 8);
    en_box.append(&Label::new(Some("Enabled")));
    let en_sw = Switch::new();
    en_sw.set_active(t.enabled);
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        let badge = badge.clone();
        en_sw.connect_state_set(move |_, state| {
            update_trigger(&cfg, idx, |t| t.enabled = state);
            engine.reload_profile();
            badge.set_text(if state { "ON" } else { "OFF" });
            if state {
                badge.remove_css_class("badge-off");
            } else {
                badge.add_css_class("badge-off");
            }
            glib::Propagation::Proceed
        });
    }
    en_box.append(&en_sw);
    toggles.append(&en_box);

    let inv_box = GtkBox::new(Orientation::Horizontal, 8);
    inv_box.append(&Label::new(Some("Inverse")));
    let inv_sw = Switch::new();
    inv_sw.set_active(t.inverse);
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        inv_sw.connect_state_set(move |_, state| {
            update_trigger(&cfg, idx, |t| t.inverse = state);
            engine.reload_profile();
            glib::Propagation::Proceed
        });
    }
    inv_box.append(&inv_sw);
    toggles.append(&inv_box);
    card.append(&toggles);

    // Pixels list
    let pixels_label = Label::new(Some(&format!("Pixels ({})", t.pixels.len())));
    pixels_label.set_halign(gtk4::Align::Start);
    card.append(&pixels_label);

    if !t.pixels.is_empty() {
        let pixels_box = GtkBox::new(Orientation::Horizontal, 4);
        pixels_box.set_halign(gtk4::Align::Start);
        for (pi, p) in t.pixels.iter().enumerate() {
            let color_str = if p.color.starts_with("0x") {
                format!("#{}", &p.color[2..])
            } else {
                p.color.clone()
            };
            let chip = Button::with_label(&format!("({}, {}) ✕", p.x, p.y));
            chip.set_tooltip_text(Some(&color_str));
            {
                let cfg = cfg.clone();
                let engine = engine.clone();
                let content_box = content_box.clone();
                chip.connect_clicked(move |_| {
                    update_trigger(&cfg, idx, |t| {
                        if pi < t.pixels.len() {
                            t.pixels.remove(pi);
                        }
                    });
                    engine.reload_profile();
                    render(&content_box, &cfg, &engine);
                });
            }
            pixels_box.append(&chip);
        }
        card.append(&pixels_box);
    }

    // Pick from screen button — uses platform screenshot + cursor read
    {
        let cfg = cfg.clone();
        let engine = engine.clone();
        let content_box = content_box.clone();
        let pick_btn = Button::with_label("🎯 Pick from screen");
        pick_btn.connect_clicked(move |_| {
            // Read current cursor position + pixel under it
            let (x, y) = crate::platform::get_cursor_pos();
            let color = crate::platform::get_pixel_color(x, y);
            let hex = format!("0x{:06X}", color & 0xFFFFFF);
            update_trigger(&cfg, idx, |t| {
                t.pixels.push(Pixel {
                    x,
                    y,
                    color: hex,
                    variation: 10,
                });
            });
            engine.reload_profile();
            render(&content_box, &cfg, &engine);
        });
        card.append(&pick_btn);
    }

    card
}