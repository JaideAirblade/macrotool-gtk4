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
    game_alive: bool,
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
            next_restart: Instant::now() + Duration::from_secs(1),
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
                // restart after a short delay to avoid a crash loop.
                self.child.take();
                self.next_restart = Instant::now() + Duration::from_secs(1);
                log::warn!("[overlay] Quickshell exited unexpectedly ({status}); restarting");
            }
            Some(Err(error)) => {
                log::warn!("[overlay] could not inspect QML child: {error}");
                if let Some(mut child) = self.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                self.next_restart = Instant::now() + Duration::from_secs(1);
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
                    self.next_restart = Instant::now() + Duration::from_secs(1);
                }
            }
        }
    }

    pub(crate) fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;

        if let Some(mut child) = self.child.take() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                Err(error) => {
                    log::warn!("[overlay] could not inspect QML child: {error}");
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
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
        game_alive: engine.detector.is_game_alive(),
        theme: ThemePalette::from_widget(theme_widget),
    }
}

impl ThemePalette {
    fn from_widget(widget: &gtk4::Widget) -> Self {
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

    Command::new(qs)
        .arg("--path")
        .arg(qml_path)
        .env("MACROTOOL_OVERLAY_STATE", state_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("could not start Quickshell: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{secure_runtime_dir, secure_runtime_dir_in, OverlayProcess};
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{Duration, Instant};

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
    fn stopping_overlay_process_terminates_its_child() {
        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn test child");
        let mut process = OverlayProcess::from_child(child);
        let pid = process.child_id().expect("child pid");
        process.stop();
        assert!(process.is_stopped());
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
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
