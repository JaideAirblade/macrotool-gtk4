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

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gtk4::prelude::*;
use gtk4::{
    ApplicationWindow, CssProvider, HeaderBar, Label, Notebook, Orientation, Paned, PolicyType,
    ScrolledWindow,
};

use crate::config;
use crate::engine;
use crate::ui::buffs_tab::BuffsTab;
use crate::ui::macros_tab::MacrosTab;
use crate::ui::overlay::{DmsPalette, Overlay};
use crate::ui::procs_tab::ProcsTab;
use crate::ui::settings_tab::SettingsTab;
use crate::ui::sidebar::Sidebar;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseDisposition {
    HideToTray,
    Quit,
}

fn close_disposition(minimize_to_tray: bool, tray_available: bool) -> CloseDisposition {
    if minimize_to_tray && tray_available {
        CloseDisposition::HideToTray
    } else {
        CloseDisposition::Quit
    }
}

fn handle_close(
    window: &ApplicationWindow,
    app: &gtk4::Application,
    cfg: &Arc<config::Manager>,
    tray_available: &Arc<AtomicBool>,
) {
    match close_disposition(
        cfg.settings().minimize_to_tray,
        tray_available.load(Ordering::Acquire),
    ) {
        CloseDisposition::HideToTray => window.set_visible(false),
        CloseDisposition::Quit => app.quit(),
    }
}

fn install_live_dms_theme(window: &ApplicationWindow) {
    let provider = CssProvider::new();
    gtk4::style_context_add_provider_for_display(
        &gtk4::prelude::WidgetExt::display(window),
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
    );

    let widget = window.clone().upcast::<gtk4::Widget>();
    let last_palette = Rc::new(RefCell::new(None::<DmsPalette>));
    let refresh = {
        let provider = provider.clone();
        let widget = widget.clone();
        let last_palette = last_palette.clone();
        move || {
            let Some(palette) = DmsPalette::from_widget(&widget) else {
                return;
            };
            if last_palette.borrow().as_ref() == Some(&palette) {
                return;
            }
            provider.load_from_data(&palette.app_css());
            *last_palette.borrow_mut() = Some(palette);
            widget.queue_draw();
        }
    };

    refresh();
    glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
        refresh();
        glib::ControlFlow::Continue
    });
}

pub struct MainWindow {
    window: ApplicationWindow,
    cfg: Arc<config::Manager>,
    engine: Arc<engine::EngineHub>,
    sidebar: Rc<Sidebar>,
    macros_tab: Rc<MacrosTab>,
    procs_tab: Rc<ProcsTab>,
    buffs_tab: Rc<BuffsTab>,
    settings_tab: SettingsTab,
    overlay: Overlay,
}

impl MainWindow {
    pub fn new(
        app: &gtk4::Application,
        cfg: Arc<config::Manager>,
        engine: Arc<engine::EngineHub>,
        tray_available: Arc<AtomicBool>,
    ) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Macrotool")
            .default_width(900)
            .default_height(640)
            .build();
        window.add_css_class("macrotool-window");
        install_live_dms_theme(&window);
        window.set_size_request(720, 520);

        // Header bar — no theme toggle, let the desktop environment handle it
        let title_label = Label::new(Some("Macrotool"));
        let header = HeaderBar::builder().title_widget(&title_label).build();
        header.set_show_title_buttons(true);
        window.set_titlebar(Some(&header));

        // Body: horizontal Paned (sidebar | notebook)
        let paned = Paned::new(Orientation::Horizontal);
        paned.set_position(240);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);

        // Notebook with 4 tabs (created before the sidebar so the sidebar's
        // on_changed callback can refresh them)
        let notebook = Notebook::new();
        notebook.set_tab_pos(gtk4::PositionType::Top);

        let macros_tab = Rc::new(MacrosTab::new(cfg.clone(), engine.clone()));
        let procs_tab = Rc::new(ProcsTab::new(cfg.clone(), engine.clone()));
        let buffs_tab = Rc::new(BuffsTab::new(cfg.clone(), engine.clone()));
        let settings_tab = SettingsTab::new(cfg.clone(), engine.clone());

        // Sidebar — on_changed refreshes the tabs + sidebar after any
        // selection/rename/add/delete
        let sidebar_slot = Rc::new(RefCell::new(std::rc::Weak::<Sidebar>::new()));
        let sidebar = Rc::new(Sidebar::new(cfg.clone(), engine.clone(), {
            let cfg = cfg.clone();
            let engine = engine.clone();
            let macros_tab = macros_tab.clone();
            let procs_tab = procs_tab.clone();
            let buffs_tab = buffs_tab.clone();
            let sidebar_slot = sidebar_slot.clone();
            Rc::new(move || {
                macros_tab.refresh(&cfg, &engine);
                procs_tab.refresh(&cfg, &engine);
                buffs_tab.refresh(&cfg, &engine);

                // Sidebar mutations can happen from a row's own signal. Wait
                // until that signal returns before replacing its rows.
                let sidebar = sidebar_slot.borrow().clone();
                glib::idle_add_local_once(move || {
                    if let Some(sidebar) = sidebar.upgrade() {
                        sidebar.refresh();
                    }
                });
            })
        }));
        *sidebar_slot.borrow_mut() = Rc::downgrade(&sidebar);

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

        // The native GTK close button hides only when a real tray is available
        // and the setting is enabled. Otherwise it genuinely quits.
        let win_cr = window.clone();
        let app_cr = app.clone();
        let cfg_cr = cfg.clone();
        let tray_available_cr = tray_available.clone();
        window.connect_close_request(move |_| {
            handle_close(&win_cr, &app_cr, &cfg_cr, &tray_available_cr);
            glib::Propagation::Stop
        });

        // ── QML overlay (Quickshell layer-shell child) ──────────────
        let overlay = Overlay::new(
            cfg.clone(),
            engine.clone(),
            window.clone().upcast::<gtk4::Widget>(),
        );

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
        self.macros_tab.refresh(&self.cfg, &self.engine);
        self.procs_tab.refresh(&self.cfg, &self.engine);
        self.buffs_tab.refresh(&self.cfg, &self.engine);
        self.sidebar.refresh();
    }

    pub fn save_and_reload(&self) {
        let tree = self.cfg.tree();
        self.cfg.set_tree(tree);
        if let Err(e) = self.cfg.flush() {
            log::error!("[main_window] config flush failed: {e}");
        }
        self.engine.reload_profile();
    }

    /// Destroy the overlay so it disappears immediately on exit.
    pub fn close(&self) {
        self.overlay.shutdown();
    }

    pub fn overlay_handle(&self) -> Overlay {
        self.overlay.clone()
    }
}

fn append_tab(notebook: &Notebook, label: &str, content: &gtk4::Widget) {
    let tab_label = Label::new(Some(label));
    notebook.append_page(content, Some(&tab_label));
}

#[cfg(test)]
mod tests {
    use super::{close_disposition, CloseDisposition};

    #[test]
    fn close_only_hides_when_minimize_to_tray_is_enabled_and_tray_exists() {
        assert_eq!(close_disposition(true, true), CloseDisposition::HideToTray);
        assert_eq!(close_disposition(false, true), CloseDisposition::Quit);
        assert_eq!(close_disposition(true, false), CloseDisposition::Quit);
        assert_eq!(close_disposition(false, false), CloseDisposition::Quit);
    }
}
