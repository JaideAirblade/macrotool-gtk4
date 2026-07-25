//! Main application window — native GTK 4, follows system theme.
//!
//! Layout:
//!   ┌───────────────────────────────────────────────┐
//!   │ HeaderBar:  Macrotool            [_] [×]      │
//!   ├──────────────┬────────────────────────────────┤
//!   │  Sidebar     │  Notebook                       │
//!   │  game/class/ │  [Macros][Pixel Triggers]       │
//!   │  spec tree   │  [Buff Timers][Settings]        │
//!   │              │                                 │
//!   └──────────────┴────────────────────────────────┘

use std::sync::Arc;

use gtk4::prelude::*;
use gtk4::{
    ApplicationWindow, Button, HeaderBar, Label, Notebook, Orientation, Paned, PolicyType,
    ScrolledWindow,
};

use crate::config;
use crate::engine;
use crate::ui::buffs_tab::BuffsTab;
use crate::ui::macros_tab::MacrosTab;
use crate::ui::procs_tab::ProcsTab;
use crate::ui::settings_tab::SettingsTab;
use crate::ui::sidebar::Sidebar;
use crate::ui::overlay::Overlay;

pub struct MainWindow {
    window: ApplicationWindow,
    cfg: Arc<config::Manager>,
    engine: Arc<engine::EngineHub>,
    sidebar: Sidebar,
    macros_tab: MacrosTab,
    procs_tab: ProcsTab,
    buffs_tab: BuffsTab,
    settings_tab: SettingsTab,
    overlay: Overlay,
}

impl MainWindow {
    pub fn new(app: &gtk4::Application, cfg: Arc<config::Manager>, engine: Arc<engine::EngineHub>) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Macrotool")
            .default_width(900)
            .default_height(640)
            .build();
        window.set_size_request(720, 520);

        // Header bar — no theme toggle, let the desktop environment handle it
        let title_label = Label::new(Some("Macrotool"));
        let header = HeaderBar::builder().title_widget(&title_label).build();

        let min_btn = Button::with_label("—");
        min_btn.set_tooltip_text(Some("Minimize"));
        header.pack_end(&min_btn);

        let close_btn = Button::with_label("✕");
        close_btn.set_tooltip_text(Some("Hide to tray"));
        header.pack_end(&close_btn);

        window.set_titlebar(Some(&header));

        // Body: horizontal Paned (sidebar | notebook)
        let paned = Paned::new(Orientation::Horizontal);
        paned.set_position(240);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);

        // Sidebar
        let sidebar = Sidebar::new(cfg.clone(), engine.clone());

        // Notebook with 4 tabs
        let notebook = Notebook::new();
        notebook.set_tab_pos(gtk4::PositionType::Top);

        let macros_tab = MacrosTab::new(cfg.clone(), engine.clone());
        let procs_tab = ProcsTab::new(cfg.clone(), engine.clone());
        let buffs_tab = BuffsTab::new(cfg.clone(), engine.clone());
        let settings_tab = SettingsTab::new(cfg.clone(), engine.clone());

        append_tab(&notebook, "Macros", macros_tab.widget());
        append_tab(&notebook, "Pixel Triggers", procs_tab.widget());
        append_tab(&notebook, "Buff Timers", buffs_tab.widget());
        append_tab(&notebook, "Settings", settings_tab.widget());

        let side_sw = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .child(sidebar.widget())
            .build();

        paned.set_start_child(Some(&side_sw));
        paned.set_end_child(Some(&notebook));
        window.set_child(Some(&paned));

        // Minimize
        let win_min = window.clone();
        min_btn.connect_clicked(move |_| {
            win_min.minimize();
        });

        // Close → hide (tray)
        let win_close = window.clone();
        close_btn.connect_clicked(move |_| {
            win_close.set_visible(false);
        });

        let win_cr = window.clone();
        window.connect_close_request(move |_| {
            win_cr.set_visible(false);
            glib::Propagation::Stop
        });

        // ── Overlay (layer-shell, stays on top) ─────────────────────
        let overlay = Overlay::new(cfg.clone(), engine.clone());
        overlay.set_position("top-left");
        overlay.show();

        Self {
            window,
            cfg,
            engine,
            sidebar,
            macros_tab,
            procs_tab,
            buffs_tab,
            settings_tab,
            overlay,
        }
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn widget(&self) -> &ApplicationWindow {
        &self.window
    }

    pub fn refresh(&self) {
        self.macros_tab.refresh(&self.cfg);
        self.procs_tab.refresh(&self.cfg);
        self.buffs_tab.refresh(&self.cfg);
    }

    pub fn save_and_reload(&self) {
        let tree = self.cfg.tree();
        self.cfg.set_tree(tree);
        if let Err(e) = self.cfg.flush() {
            log::error!("[main_window] config flush failed: {e}");
        }
        self.engine.reload_profile();
    }
}

fn append_tab(notebook: &Notebook, label: &str, content: &gtk4::Widget) {
    let tab_label = Label::new(Some(label));
    notebook.append_page(content, Some(&tab_label));
}