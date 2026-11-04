//! `#[tauri::command]` functions: the only doors from the WebView into Rust.
//!
//! Scanning moves to a worker thread with `spawn_blocking`, so the window
//! stays responsive; progress flows back through a `Channel`.
//!
//! Live updates (phase 8) go the other way: nobody asked, Rust noticed a
//! file change. That is what *events* are for: `app.emit("atlas-changed", …)`
//! reaches every listener, with no command waiting for an answer.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::dto::{
    AtlasViewDto, EvidenceDto, LiveDto, LiveEvent, NodeDetailsDto, ProjectSummary, RecentProject,
    ScanEvent, SearchHitDto, TreeDto, ViewRequest,
};
use crate::error::{AppError, ErrorKind};
use crate::live;
use crate::project::{self, OpenProject};
use crate::recent;
use crate::state::AppState;
use crate::watch::{self, Batch};

/// The event the front end listens to for live updates.
pub const ATLAS_CHANGED: &str = "atlas-changed";

/// What the status bar shows: proof that the Rust side is alive and that the
/// tree-sitter toolchain inside java-atlas-scan works.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: &'static str,
    pub platform: &'static str,
    pub java_grammar_ok: bool,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        java_grammar_ok: java_atlas_scan::java_parser().is_ok(),
    }
}

fn join_error(e: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorKind::Internal, e.to_string())
}

fn open(state: &AppState) -> Result<Arc<OpenProject>, AppError> {
    state
        .get()
        .ok_or_else(|| AppError::new(ErrorKind::NoProjectOpen, "no project is open"))
}

#[tauri::command]
pub async fn open_project(
    path: String,
    on_progress: Channel<ScanEvent>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ProjectSummary, AppError> {
    // Rescanning the project on screen keeps its tiles where they are.
    let hint = state
        .get()
        .filter(|p| p.root == Path::new(&path))
        .map(|p| p.hint())
        .unwrap_or_default();
    let scanned = {
        let root = PathBuf::from(&path);
        tauri::async_runtime::spawn_blocking(move || {
            // A closed window drops the channel; a failed send is not our problem.
            project::scan(
                &root,
                &|event| {
                    let _ = on_progress.send(event);
                },
                &hint,
            )
        })
        .await
        .map_err(join_error)?
    };

    let project = match scanned {
        Ok(project) => project,
        Err(err) => {
            // A recent entry that no longer opens is dead weight.
            if matches!(err.kind, ErrorKind::NotFound | ErrorKind::NotAJavaProject) {
                recent::forget(&app, &path);
            }
            return Err(err);
        }
    };

    let summary = project.summary.clone();
    recent::remember(
        &app,
        RecentProject {
            name: summary.name.clone(),
            path,
            build_system: summary.build_system.clone(),
        },
    );
    let root = project.root.clone();
    state.set(project);
    start_watch(&app, root);
    Ok(summary)
}

/// Starts watching the open project's folder, replacing any previous watch.
/// A watch that cannot start (too many inotify watches, a network drive) is
/// not an error: the map works, just not live; `set_live` reports why.
fn start_watch(app: &AppHandle, root: PathBuf) {
    let state = app.state::<AppState>();
    // Drop the old watch outside the lock: its thread may be waiting for it.
    let old = state.live().watch.take();
    drop(old);

    let handle = app.clone();
    let watched = root.clone();
    let started = watch::start(root, move |batch| on_batch(&handle, &watched, batch));
    let mut live = state.live();
    live.pending = Batch::default();
    match started {
        Ok(w) => {
            live.watch = Some(w);
            live.error = None;
        }
        Err(e) => {
            live.watch = None;
            live.error = Some(e.to_string());
        }
    }
}

/// Runs on the watch thread, once per debounced batch.
fn on_batch(app: &AppHandle, watched: &PathBuf, batch: Batch) {
    let state = app.state::<AppState>();
    {
        let mut live = state.live();
        if !live.enabled {
            live.pending.merge(batch);
            return;
        }
    }
    apply_batch(app, &state, watched, batch);
}

fn apply_batch(app: &AppHandle, state: &AppState, watched: &PathBuf, batch: Batch) {
    let Some(current) = state.get() else {
        return;
    };
    // A batch from the previous project's watch, delivered late.
    if &current.root != watched {
        return;
    }
    match live::refresh(&current, &batch) {
        Ok(None) => {}
        Ok(Some((project, event))) => {
            if state.replace(&current, project) {
                let _ = app.emit(ATLAS_CHANGED, event);
            }
        }
        Err(e) => {
            let _ = app.emit(ATLAS_CHANGED, LiveEvent::Failed { message: e.message });
        }
    }
}

fn live_dto(state: &AppState) -> LiveDto {
    let live = state.live();
    LiveDto {
        enabled: live.enabled,
        watching: live.watch.is_some(),
        error: live.error.clone(),
    }
}

/// Switches live updates on or off. Changes made while they were off are
/// applied as soon as they are switched on again.
#[tauri::command]
pub fn set_live(enabled: bool, app: AppHandle, state: State<'_, AppState>) -> LiveDto {
    let (pending, watched) = {
        let mut live = state.live();
        live.enabled = enabled;
        let watched = live.watch.as_ref().map(|w| w.root.clone());
        let pending = if enabled {
            std::mem::take(&mut live.pending)
        } else {
            Batch::default()
        };
        (pending, watched)
    };
    if let (false, Some(watched)) = (pending.is_empty(), watched) {
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            apply_batch(&handle, &handle.state::<AppState>(), &watched, pending);
        });
    }
    live_dto(&state)
}

#[tauri::command]
pub async fn get_atlas_view(
    request: ViewRequest,
    state: State<'_, AppState>,
) -> Result<AtlasViewDto, AppError> {
    let project = open(&state)?;
    tauri::async_runtime::spawn_blocking(move || project.view(&request))
        .await
        .map_err(join_error)?
}

#[tauri::command]
pub async fn get_node_details(
    key: String,
    state: State<'_, AppState>,
) -> Result<NodeDetailsDto, AppError> {
    let project = open(&state)?;
    tauri::async_runtime::spawn_blocking(move || project.details(&key))
        .await
        .map_err(join_error)?
}

#[tauri::command]
pub async fn get_edge_evidence(
    from: String,
    to: String,
    state: State<'_, AppState>,
) -> Result<Vec<EvidenceDto>, AppError> {
    let project = open(&state)?;
    tauri::async_runtime::spawn_blocking(move || project.evidence(&from, &to))
        .await
        .map_err(join_error)?
}

#[tauri::command]
pub async fn get_tree(state: State<'_, AppState>) -> Result<TreeDto, AppError> {
    let project = open(&state)?;
    tauri::async_runtime::spawn_blocking(move || project.tree())
        .await
        .map_err(join_error)
}

#[tauri::command]
pub async fn search(
    query: String,
    state: State<'_, AppState>,
) -> Result<Vec<SearchHitDto>, AppError> {
    let project = open(&state)?;
    tauri::async_runtime::spawn_blocking(move || project.search(&query))
        .await
        .map_err(join_error)
}

#[tauri::command]
pub fn recent_projects(app: AppHandle) -> Vec<RecentProject> {
    recent::load(&app)
}

#[tauri::command]
pub fn forget_recent_project(path: String, app: AppHandle) -> Vec<RecentProject> {
    recent::forget(&app, &path);
    recent::load(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_info_reports_a_working_grammar() {
        let info = app_info();
        assert!(info.java_grammar_ok);
        assert!(!info.version.is_empty());
    }
}
