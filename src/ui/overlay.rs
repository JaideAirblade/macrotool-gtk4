//! Quickshell/QML overlay process.
//!
//! Macrotool owns the process and writes a small live-state JSON file at 20Hz.
//! QML owns presentation. Macrotool resolves its live GTK colors into the state
//! payload; Qt's `SystemPalette` is retained only as a portable fallback.

use gtk4::prelude::*;
use serde::Serialize;
use std::cell::RefCell;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config;
use crate::engine;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayMacro {
    name: String,
    hotkey: String,
    running: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayBuff {
    name: String,
    remaining_ms: f64,
    fraction: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DmsPalette {
    pub(crate) window: String,
    pub(crate) window_text: String,
    pub(crate) highlight: String,
    pub(crate) highlighted_text: String,
    pub(crate) mid: String,
}

impl DmsPalette {
    fn from_json(source: &str, dark: bool) -> Option<Self> {
        let mode = if dark { "dark" } else { "light" };
        let colors: serde_json::Value = serde_json::from_str(source).ok()?;
        let palette = colors.get("colors")?.get(mode)?;
        let color = |name: &str| palette.get(name)?.as_str().map(str::to_owned);

        Some(Self {
            window: color("background")?,
            window_text: color("on_background")?,
            highlight: color("primary")?,
            highlighted_text: color("on_primary")?,
            mid: color("outline")?,
        })
    }

    pub(crate) fn app_css(&self) -> String {
        // Keep this thin: the desktop theme already styles buttons, rows,
        // entries and switches natively (end4-pC's matugen writes a full
        // named-color set to ~/.config/gtk-4.0/gtk.css). Earlier versions
        // re-painted every widget class with flat alpha backgrounds, which
        // fought the native theme and looked broken. Only the window
        // surfaces (which plain themes get wrong for foreign apps) and the
        // accent hooks are styled here.
        format!(
            r#"
.macrotool-window,
.macrotool-window.background,
.macrotool-window paned,
.macrotool-window scrolledwindow,
.macrotool-window viewport,
.macrotool-window notebook > stack {{
    background-color: {window};
    color: {window_text};
}}
.macrotool-window headerbar,
.macrotool-window tabbox {{
    background-color: {window};
    color: {window_text};
}}
.macrotool-window button.suggested-action,
.macrotool-window switch:checked,
.macrotool-window scale highlight,
.macrotool-window progressbar progress {{
    color: {highlighted_text};
    background-color: {highlight};
}}
.macrotool-window selection,
.macrotool-window row:selected {{
    color: {highlighted_text};
    background-color: {highlight};
}}
"#,
            window = self.window,
            window_text = self.window_text,
            highlight = self.highlight,
            highlighted_text = self.highlighted_text,
        )
    }

    pub(crate) fn from_widget(widget: &gtk4::Widget) -> Option<Self> {
        let dark = gtk4::Settings::default()
            .map(|settings| settings.is_gtk_application_prefer_dark_theme())
            .unwrap_or_else(|| {
                let color = widget.style_context().color();
                color.red() + color.green() + color.blue() > 1.5
            });
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)?;

        // Prefer the desktop shell's own matugen palette (end4-pC
        // regenerates it on every wallpaper change). The DMS cache is a
        // legacy source: on hosts without DMS it can sit stale for weeks
        // and would otherwise win the theme with dead colors.
        if let Some(palette) =
            Self::from_shell_file(&home.join(".local/state/quickshell/user/generated/colors.json"), dark)
        {
            return Some(palette);
        }
        Self::from_dms_file(&home.join(".cache/DankMaterialShell/dms-colors.json"), dark)
    }

    /// Re-open the cache by path on every refresh. DMS publishes palette
    /// updates through atomic replacement, so retaining an existing file
    /// handle would otherwise keep Macrotool on the previous theme.
    fn from_dms_file(path: &Path, dark: bool) -> Option<Self> {
        Self::from_json(&std::fs::read_to_string(path).ok()?, dark)
    }

    /// Read the desktop shell's matugen M3 palette: a flat token → hex
    /// map (no `colors.{light,dark}` wrapper), published by end4-pC at
    /// `~/.local/state/quickshell/user/generated/colors.json`. The file
    /// always holds the active variant, so `dark` is only recorded for
    /// signature parity with the DMS path.
    fn from_shell_file(path: &Path, dark: bool) -> Option<Self> {
        let _ = dark;
        let colors: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        let color = |name: &str| colors.get(name)?.as_str().map(str::to_owned);

        Some(Self {
            window: color("surface_container")?,
            window_text: color("on_surface")?,
            highlight: color("primary")?,
            highlighted_text: color("on_primary")?,
            mid: color("outline")?,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThemePalette {
    window: Option<String>,
    window_text: Option<String>,
    highlight: Option<String>,
    highlighted_text: Option<String>,
    mid: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OverlayState {
    updated_at: u64,
    enabled: bool,
    active_game: String,
    active_class: String,
    active_spec: String,
    macros: Vec<OverlayMacro>,
    buffs: Vec<OverlayBuff>,
    game_active: bool,
    game_present: bool,
    game_in_focus: bool,
    overlay_position: String,
    theme: ThemePalette,
}

pub(crate) struct OverlayProcess {
    child: Option<Child>,
    stopped: bool,
    next_restart: Instant,
}

impl OverlayProcess {
    pub(crate) fn from_child(child: Child) -> Self {
        Self {
            child: Some(child),
            stopped: false,
            next_restart: Instant::now(),
        }
    }

    fn unavailable() -> Self {
        Self {
            child: None,
            stopped: false,
            next_restart: Instant::now() + Duration::from_secs(5),
        }
    }

    fn maintain(&mut self, state_path: &Path, qml_path: &Path) {
        self.maintain_with(|| spawn_qml_overlay(state_path, qml_path));
    }

    fn maintain_with<F>(&mut self, spawn: F)
    where
        F: FnOnce() -> Result<Child, String>,
    {
        if self.stopped {
            return;
        }

        let child_result = self.child.as_mut().map(|child| child.try_wait());
        match child_result {
            Some(Ok(Some(status))) => {
                // `try_wait` reaped the exited process. Drop the handle and
                // restart after a short delay to avoid a crash loop. 5s so a
                // quickshell that refuses to stay up cannot turn the
                // spawn/kill cycle into desktop-wide input churn.
                self.child.take();
                self.next_restart = Instant::now() + Duration::from_secs(5);
                log::warn!("[overlay] Quickshell exited unexpectedly ({status}); restarting in 5s");
            }
            Some(Err(error)) => {
                log::warn!("[overlay] could not inspect QML child: {error}");
                if let Some(child) = self.child.take() {
                    terminate_in_background(child);
                }
                self.next_restart = Instant::now() + Duration::from_secs(5);
            }
            Some(Ok(None)) => return,
            None => {}
        }

        if self.child.is_none() && Instant::now() >= self.next_restart {
            match spawn() {
                Ok(child) => {
                    log::info!("[overlay] Quickshell overlay restarted");
                    self.child = Some(child);
                }
                Err(error) => {
                    log::warn!("[overlay] could not restart Quickshell: {error}");
                    self.next_restart = Instant::now() + Duration::from_secs(5);
                }
            }
        }
    }

    pub(crate) fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;

        if let Some(child) = self.child.take() {
            terminate_in_background(child);
        }
    }

    #[cfg(test)]
    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped
    }

    #[cfg(test)]
    pub(crate) fn child_id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
}

impl Drop for OverlayProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Tear down the overlay child without ever blocking the GTK main thread.
///
/// Quit-time shell freezes (Bug A, 2026-09-25): `child.kill()` + blocking
/// `child.wait()` on the UI thread forced Hyprland to synchronously destroy
/// the overlay's layer surfaces while the shell holds its focus grab, and
/// the whole desktop stalled. Now: SIGTERM (graceful), a bounded wait on a
/// dedicated thread, then SIGKILL only if it ignores TERM.
fn terminate_in_background(child: Child) {
    let mut child = child;

    // Graceful ask first: SIGTERM so quickshell exits normally and tears
    // its layer surfaces down asynchronously, which is what the
    // compositor wants. (std's Child::kill() is SIGKILL, which is what
    // caused the freeze, so it is avoided here entirely.)
    unsafe {
        let _ = libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }

    std::thread::spawn(move || {
        // Bound the graceful wait, then escalate to SIGKILL.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return,
            }
        }
    });
}

struct OverlayInner {
    process: RefCell<OverlayProcess>,
    runtime_dir: tempfile::TempDir,
    state_path: PathBuf,
    qml_path: PathBuf,
    stopped: AtomicBool,
    cfg: Arc<config::Manager>,
    engine: Arc<engine::EngineHub>,
    theme_widget: gtk4::Widget,
}

impl OverlayInner {
    fn refresh(&self) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }

        let state = build_state(&self.cfg, &self.engine, &self.theme_widget);
        if let Err(error) = write_state(&self.state_path, &state) {
            log::warn!("[overlay] could not write live state: {error}");
        }
        self.process
            .borrow_mut()
            .maintain(&self.state_path, &self.qml_path);
    }

    fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }

        self.process.borrow_mut().stop();
        let _ = std::fs::remove_dir_all(self.runtime_dir.path());
    }
}

impl Drop for OverlayInner {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Clone)]
pub struct Overlay {
    inner: Option<Rc<OverlayInner>>,
}

impl Overlay {
    pub fn new(
        cfg: Arc<config::Manager>,
        engine: Arc<engine::EngineHub>,
        theme_widget: gtk4::Widget,
    ) -> Self {
        let runtime_dir = match secure_runtime_dir() {
            Ok(runtime_dir) => runtime_dir,
            Err(error) => {
                log::error!("[overlay] could not create private runtime directory: {error}");
                return Self { inner: None };
            }
        };
        let state_path = runtime_dir.path().join("state.json");
        let qml_dir = runtime_dir.path().join("qml");
        let qml_path = match prepare_qml_overlay(&qml_dir) {
            Ok(qml_path) => qml_path,
            Err(error) => {
                log::error!("[overlay] QML overlay unavailable: {error}");
                return Self { inner: None };
            }
        };
        let initial_state = build_state(&cfg, &engine, &theme_widget);
        if let Err(error) = write_state(&state_path, &initial_state) {
            log::warn!("[overlay] could not write initial state: {error}");
        }

        let process = match spawn_qml_overlay(&state_path, &qml_path) {
            Ok(child) => OverlayProcess::from_child(child),
            Err(error) => {
                log::error!("[overlay] QML overlay unavailable: {error}");
                OverlayProcess::unavailable()
            }
        };

        let inner = Rc::new(OverlayInner {
            process: RefCell::new(process),
            runtime_dir,
            state_path,
            qml_path,
            stopped: AtomicBool::new(false),
            cfg,
            engine,
            theme_widget,
        });

        let weak = Rc::downgrade(&inner);
        glib::timeout_add_local(Duration::from_millis(50), move || {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if inner.stopped.load(Ordering::Acquire) {
                return glib::ControlFlow::Break;
            }
            inner.refresh();
            glib::ControlFlow::Continue
        });

        Self { inner: Some(inner) }
    }

    pub fn shutdown(&self) {
        if let Some(inner) = &self.inner {
            inner.shutdown();
        }
    }
}

fn build_state(
    cfg: &Arc<config::Manager>,
    engine: &Arc<engine::EngineHub>,
    theme_widget: &gtk4::Widget,
) -> OverlayState {
    let tree = cfg.tree();
    let enabled = engine.macro_enabled.load(Ordering::Acquire);

    let macros = cfg
        .get_macros()
        .into_iter()
        .filter(|item| item.enabled)
        .map(|item| OverlayMacro {
            running: engine.macros.is_running(&item.hotkey),
            name: item.name,
            hotkey: item.hotkey,
        })
        .collect();

    let buff_configs = cfg.get_buff_timers();
    let buffs = engine
        .buffs
        .get_active_timers()
        .into_iter()
        .map(|(name, remaining)| {
            let duration = buff_configs
                .iter()
                .find(|item| item.name == name)
                .map(|item| item.duration.max(1) as f64)
                .unwrap_or(5000.0);
            OverlayBuff {
                fraction: (remaining / duration).clamp(0.0, 1.0),
                name,
                remaining_ms: remaining,
            }
        })
        .collect();

    OverlayState {
        updated_at: now_millis(),
        enabled,
        active_game: tree.active_game,
        active_class: tree.active_class,
        active_spec: tree.active_spec,
        macros,
        buffs,
        game_active: engine.detector.is_active(cfg),
        game_present: engine.detector.is_game_alive(),
        game_in_focus: engine.detector.is_active(cfg),
        overlay_position: cfg.settings().overlay_position,
        theme: ThemePalette::from_widget(theme_widget),
    }
}

impl ThemePalette {
    fn from_widget(widget: &gtk4::Widget) -> Self {
        if let Some(dms) = DmsPalette::from_widget(widget) {
            return Self {
                window: Some(dms.window),
                window_text: Some(dms.window_text),
                highlight: Some(dms.highlight),
                highlighted_text: Some(dms.highlighted_text),
                mid: Some(dms.mid),
            };
        }

        let context = widget.style_context();
        let widget_text = rgba_to_hex(&context.color());
        let pick = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| context.lookup_color(name))
                .map(|color| rgba_to_hex(&color))
        };

        let window = pick(&[
            "window_bg_color",
            "theme_bg_color",
            "view_bg_color",
            "base_color",
        ]);
        // Only export GTK foreground/background together. If either side of
        // the pair is unavailable, QML falls back to Qt's complete pair.
        let window_text = window.as_ref().map(|_| {
            pick(&["window_fg_color", "theme_fg_color", "text_color"])
                .unwrap_or_else(|| widget_text.clone())
        });

        let gtk_highlight = pick(&["accent_bg_color", "accent_color", "theme_selected_bg_color"]);
        let gtk_highlighted_text = pick(&["accent_fg_color", "theme_selected_fg_color"]);
        let (highlight, highlighted_text) = match (gtk_highlight, gtk_highlighted_text) {
            (Some(background), Some(foreground)) => (Some(background), Some(foreground)),
            _ => (None, None),
        };
        let mid = window
            .as_ref()
            .and_then(|_| pick(&["borders", "border_color", "shade_color"]));

        Self {
            window,
            window_text,
            highlight,
            highlighted_text,
            mid,
        }
    }
}

fn rgba_to_hex(color: &gtk4::gdk::RGBA) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (color.red() * 255.0).round() as u8,
        (color.green() * 255.0).round() as u8,
        (color.blue() * 255.0).round() as u8
    )
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn secure_runtime_dir() -> Result<tempfile::TempDir, String> {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    secure_runtime_dir_in(base.as_deref())
}

fn secure_runtime_dir_in(base: Option<&Path>) -> Result<tempfile::TempDir, String> {
    let mut builder = tempfile::Builder::new();
    builder
        .prefix("macrotool-overlay-")
        .permissions(std::fs::Permissions::from_mode(0o700));
    match base {
        Some(base) => builder.tempdir_in(base).map_err(|error| error.to_string()),
        None => builder.tempdir().map_err(|error| error.to_string()),
    }
}

fn write_state(path: &Path, state: &OverlayState) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "overlay state path has no parent".to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("could not create private state file: {error}"))?;
    serde_json::to_writer(&mut temporary, state).map_err(|error| error.to_string())?;
    temporary.flush().map_err(|error| error.to_string())?;
    temporary
        .persist(path)
        .map_err(|error| format!("could not publish overlay state: {}", error.error))?;
    Ok(())
}

fn prepare_qml_overlay(qml_dir: &Path) -> Result<PathBuf, String> {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(qml_dir)
        .map_err(|error| error.to_string())?;
    let qml_path = qml_dir.join("shell.qml");
    let mut qml_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&qml_path)
        .map_err(|error| format!("could not create embedded QML: {error}"))?;
    qml_file
        .write_all(include_str!("../../qml/overlay/shell.qml").as_bytes())
        .map_err(|error| format!("could not prepare embedded QML: {error}"))?;
    Ok(qml_path)
}

fn spawn_qml_overlay(state_path: &Path, qml_path: &Path) -> Result<Child, String> {
    let qs = std::env::var_os("MACROTOOL_QS").unwrap_or_else(|| "qs".into());

    let mut cmd = Command::new(qs);
    cmd.arg("--path")
        .arg(qml_path)
        .env("MACROTOOL_OVERLAY_STATE", state_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());

    // The overlay's QML shells out to `mmsg get all-clients` to check
    // whether the game is visible on the active tag. mmsg needs
    // MANGO_INSTANCE_SIGNATURE pointing at mango's IPC socket. If the
    // var is already set (inherited from the DMS session), pass it
    // through. Otherwise auto-detect: scan /run/user/<uid>/ for
    // mango-<pid>.sock.
    if std::env::var_os("MANGO_INSTANCE_SIGNATURE").is_none() {
        if let Some(socket) = detect_mango_socket() {
            cmd.env("MANGO_INSTANCE_SIGNATURE", socket);
        }
    }

    cmd.spawn()
        .map_err(|error| format!("could not start Quickshell: {error}"))
}

/// Find mango's IPC socket at /run/user/<uid>/mango-<pid>.sock.
/// Returns the full path if found.
fn detect_mango_socket() -> Option<String> {
    let uid = unsafe { libc::getuid() };
    let dir = std::path::PathBuf::from(format!("/run/user/{}", uid));
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("mango-") && name.ends_with(".sock") {
            return Some(entry.path().to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{secure_runtime_dir, secure_runtime_dir_in, DmsPalette, OverlayProcess};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn dms_palette_uses_the_active_mode_and_preserves_overlay_contrast() {
        let palette = DmsPalette::from_json(
            r##"{
                "colors": {
                    "light": {
                        "background": "#f7f9ff",
                        "on_background": "#151c24",
                        "primary": "#006497",
                        "on_primary": "#ffffff",
                        "outline": "#717782"
                    },
                    "dark": {
                        "background": "#0c141b",
                        "on_background": "#dbe3ed",
                        "primary": "#92ccff",
                        "on_primary": "#00344f",
                        "outline": "#8d929a"
                    }
                }
            }"##,
            true,
        )
        .expect("parse DMS palette");

        assert_eq!(palette.window, "#0c141b");
        assert_eq!(palette.window_text, "#dbe3ed");
        assert_eq!(palette.highlight, "#92ccff");
        assert_eq!(palette.highlighted_text, "#00344f");
        assert_eq!(palette.mid, "#8d929a");

        let css = palette.app_css();
        assert!(css.contains(".macrotool-window"));
        assert!(css.contains("background-color: #0c141b"));
        assert!(css.contains("color: #dbe3ed"));
        assert!(css.contains("background-color: #92ccff"));
        assert!(css.contains("color: #00344f"));
    }

    #[test]
    fn dms_palette_reopens_the_cache_after_an_atomic_theme_replace() {
        let directory = tempfile::tempdir().expect("create palette fixture directory");
        let cache = directory.path().join("dms-colors.json");
        let initial = r##"{
            "colors": {
                "dark": {
                    "background": "#101010",
                    "on_background": "#eeeeee",
                    "primary": "#ff0000",
                    "on_primary": "#000000",
                    "outline": "#777777"
                }
            }
        }"##;
        std::fs::write(&cache, initial).expect("write initial palette");
        assert_eq!(
            DmsPalette::from_dms_file(&cache, true)
                .expect("read initial palette")
                .highlight,
            "#ff0000"
        );

        let replacement = directory.path().join("dms-colors.next.json");
        let updated = initial.replace("#ff0000", "#00ff00");
        std::fs::write(&replacement, updated).expect("write replacement palette");
        std::fs::rename(&replacement, &cache).expect("atomically publish replacement palette");

        assert_eq!(
            DmsPalette::from_dms_file(&cache, true)
                .expect("read replacement palette")
                .highlight,
            "#00ff00"
        );
    }

    #[test]
    fn qml_overlay_prefers_gtk_colors_with_a_system_palette_fallback() {
        let qml = include_str!("../../qml/overlay/shell.qml");
        assert!(qml.contains("SystemPalette"));
        assert!(qml.contains("gtkTheme.window"));
        assert!(qml.contains("palette.window"));
        assert!(qml.contains("palette.windowText"));
        assert!(!qml.contains("#101018"));
        assert!(!qml.contains("#eeeeee"));
    }

    #[test]
    fn qml_overlay_follows_the_desktop_shell_matugen_palette() {
        let qml = include_str!("../../qml/overlay/shell.qml");
        // One palette source with the end4-pC shell: its matugen output is
        // read live (FileView + watchChanges), M3 tokens preferred over the
        // GTK state.theme fallback chain.
        assert!(qml.contains("user/generated/colors.json"));
        assert!(qml.contains("watchChanges: true"));
        assert!(qml.contains("m3colors.surface_container"));
        assert!(qml.contains("m3colors.on_surface"));
        assert!(qml.contains("m3colors.primary"));
        assert!(qml.contains("m3colors.on_primary"));
        assert!(qml.contains("m3colors.outline_variant"));
        // Shell-identical chrome details.
        assert!(qml.contains("Google Sans Flex"));
        assert!(qml.contains("radius: 18"));
    }

    #[test]
    fn dms_palette_is_the_legacy_fallback_not_the_primary_source() {
        // On a DMS host the shell writes dms-colors.json fresh, so the
        // chain still ends there. On end4-pC hosts (UwU) the shell file
        // wins first, which is what keeps a stale DMS cache from winning.
        let source = include_str!("../../src/ui/overlay.rs");
        let shell_marker = source.find("from_shell_file").expect("shell palette source");
        let dms_marker = source.find("from_dms_file(&home.join").expect("dms fallback");
        assert!(shell_marker < dms_marker);
    }

    #[test]
    fn qml_overlay_sizes_to_its_content_and_uses_the_saved_position() {
        let qml = include_str!("../../qml/overlay/shell.qml");
        assert!(qml.contains("implicitWidth: card.implicitWidth"));
        assert!(!qml.contains("implicitWidth: 320"));
        assert!(qml.contains("root.state.overlayPosition"));
        assert!(qml.contains("top-left"));
        assert!(qml.contains("top-right"));
        assert!(qml.contains("bottom-left"));
        assert!(qml.contains("bottom-right"));
    }

    #[test]
    fn qml_overlay_only_maps_when_the_game_client_is_visible_on_an_active_tag() {
        let qml = include_str!("../../qml/overlay/shell.qml");
        assert!(qml.contains("property bool gameVisibleOnActiveTag"));
        // Mango keeps the client-list probe; Hyprland (no mango IPC) maps
        // on the detector's gameInFocus alone, so the probe gates on the
        // focus flag instead of running unconditionally.
        assert!(qml.contains("command: [\"mmsg\", \"get\", \"all-clients\"]"));
        assert!(qml.contains("root.state.gameInFocus === true && gameVisibleOnActiveTag === false"));
        assert!(qml.contains("&& root.gameVisibleOnActiveTag"));
    }

    #[test]
    fn qml_overlay_visibility_probe_does_not_clear_a_known_visible_game_before_its_next_reply() {
        let qml = include_str!("../../qml/overlay/shell.qml");
        assert!(!qml.contains("gameVisibleOnActiveTag = false;\n        try"));
    }

    #[test]
    fn shell_palette_source_reads_the_flat_matugen_token_map() {
        let source = include_str!("../../src/ui/overlay.rs");
        assert!(source.contains("fn from_shell_file"));
        assert!(source.contains("user/generated/colors.json"));
        assert!(source.contains("color(\"surface_container\")?"));
        assert!(source.contains("color(\"on_surface\")?"));
        assert!(source.contains("color(\"primary\")?"));
        assert!(source.contains("color(\"on_primary\")?"));
        assert!(source.contains("color(\"outline\")?"));
    }

    #[test]
    fn app_css_leaves_native_widget_styling_to_the_theme() {
        // The broken-UI regression: earlier app_css re-painted button,
        // entry, list and row with flat alpha backgrounds. Those classes
        // must stay out; only surfaces and accent hooks remain.
        let css = include_str!("../../src/ui/overlay.rs");
        let css_start = css.find("pub(crate) fn app_css").expect("app_css present");
        let css_end = css.find("pub(crate) fn from_widget").expect("from_widget");
        let block = &css[css_start..css_end];
        assert!(block.contains(".macrotool-window,"));
        assert!(block.contains(".macrotool-window headerbar,"));
        assert!(!block.contains(".macrotool-window button,\n"));
        assert!(!block.contains(".macrotool-window entry"));
        assert!(!block.contains(".macrotool-window separator"));
        assert!(block.contains("button.suggested-action"));
    }

    #[test]
    fn qml_overlay_profile_header_contributes_its_full_natural_width() {
        let qml = include_str!("../../qml/overlay/shell.qml");
        assert!(qml.contains("id: profileHeader"));
        assert!(qml.contains("id: profileRow"));
        assert!(qml.contains(
            "implicitWidth: statusIndicator.implicitWidth + profileRow.spacing + profileTitle.implicitWidth"
        ));
        assert!(qml.contains(
            "implicitWidth: Math.max(body.implicitWidth, profileHeader.implicitWidth) + 24"
        ));
    }

    #[test]
    fn stopping_overlay_process_terminates_its_child() {
        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn test child");
        let mut process = OverlayProcess::from_child(child);
        let pid = process.child_id().expect("child pid");
        process.stop();
        assert!(process.is_stopped());
        // stop() now returns immediately (graceful TERM in the background);
        // poll briefly for the child to actually die so the test stays
        // deterministic without blocking the caller for a fixed time.
        let deadline = Instant::now() + Duration::from_secs(3);
        while PathBuf::from(format!("/proc/{pid}")).exists() {
            assert!(
                Instant::now() < deadline,
                "overlay child survived 3s after stop()"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn runtime_directory_is_private_and_unique() {
        let first = secure_runtime_dir().expect("first runtime directory");
        let second = secure_runtime_dir().expect("second runtime directory");
        let fallback = secure_runtime_dir_in(None).expect("fallback runtime directory");
        assert_ne!(first.path(), second.path());
        for runtime_dir in [&first, &second, &fallback] {
            assert_eq!(
                runtime_dir
                    .path()
                    .metadata()
                    .expect("runtime metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn exited_overlay_process_is_reaped_and_restarted() {
        let child = Command::new("sh")
            .args(["-c", "exit 23"])
            .spawn()
            .expect("spawn exiting child");
        let exited_pid = child.id();
        let mut process = OverlayProcess::from_child(child);
        std::thread::sleep(Duration::from_millis(50));
        process.next_restart = Instant::now();
        process.maintain_with(|| {
            Command::new("sleep")
                .arg("30")
                .spawn()
                .map_err(|error| error.to_string())
        });

        assert!(!PathBuf::from(format!("/proc/{exited_pid}")).exists());
        assert_ne!(process.child_id(), Some(exited_pid));
        process.stop();
    }
}
