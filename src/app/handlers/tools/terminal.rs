use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    app::AppState,
    domain::ProcessInfo,
    error::{AppError, AppResult},
};

#[cfg(windows)]
use {
    raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, Win32WindowHandle,
        WindowHandle,
    },
    std::num::NonZeroIsize,
    windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow,
};

#[derive(Debug, Deserialize)]
pub struct PickDirectoryRequest {
    #[serde(default)]
    pub initial_directory: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PickDirectoryResponse {
    pub path: Option<String>,
}

#[cfg(windows)]
struct DialogParentWindow {
    hwnd: NonZeroIsize,
}

#[cfg(windows)]
impl DialogParentWindow {
    fn foreground() -> Option<Self> {
        let hwnd = unsafe { GetForegroundWindow() };
        NonZeroIsize::new(hwnd as isize).map(|hwnd| Self { hwnd })
    }
}

#[cfg(windows)]
impl HasWindowHandle for DialogParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = Win32WindowHandle::new(self.hwnd);
        // The HWND is borrowed only while rfd copies it into the dialog options.
        Ok(unsafe { WindowHandle::borrow_raw(handle.into()) })
    }
}

#[cfg(windows)]
impl HasDisplayHandle for DialogParentWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}
pub async fn pick_directory(
    Json(request): Json<PickDirectoryRequest>,
) -> AppResult<Json<PickDirectoryResponse>> {
    let mut dialog = rfd::AsyncFileDialog::new();
    #[cfg(windows)]
    let parent_window = DialogParentWindow::foreground();
    #[cfg(windows)]
    if let Some(parent_window) = parent_window.as_ref() {
        dialog = dialog.set_parent(parent_window);
    }
    if let Some(initial_directory) = request
        .initial_directory
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let initial_directory = std::path::Path::new(initial_directory);
        if initial_directory.is_dir() {
            dialog = dialog.set_directory(initial_directory);
        }
    }

    let path = dialog
        .pick_folder()
        .await
        .map(|folder| folder.path().to_string_lossy().into_owned());
    Ok(Json(PickDirectoryResponse { path }))
}
#[derive(Debug, Deserialize)]
pub struct ProcessListQuery {
    pub agent_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct ProcessOwnerQuery {
    pub agent_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct TerminalInputRequest {
    pub data: String,
    #[serde(default)]
    pub close_stdin: bool,
}

#[derive(Debug, Deserialize)]
pub struct TerminalResizeRequest {
    pub rows: u16,
    pub cols: u16,
}

pub async fn list_tool_processes(
    State(state): State<AppState>,
    Query(query): Query<ProcessListQuery>,
) -> AppResult<Json<Vec<ProcessInfo>>> {
    Ok(Json(state.terminal_manager.list(query.agent_id).await))
}

pub async fn get_tool_process(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    Ok(Json(info))
}

pub async fn stop_tool_process(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    state
        .terminal_manager
        .terminate(&process_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound)
}

pub async fn list_terminal_sessions(
    State(state): State<AppState>,
    Query(query): Query<ProcessListQuery>,
) -> AppResult<Json<Vec<ProcessInfo>>> {
    list_tool_processes(State(state), Query(query)).await
}

pub async fn get_terminal_session(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    get_tool_process(State(state), Path(process_id), Query(query)).await
}

pub async fn write_terminal_session(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
    Json(request): Json<TerminalInputRequest>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    Ok(Json(
        state
            .terminal_manager
            .write(&process_id, request.data.as_bytes(), request.close_stdin)
            .await?,
    ))
}

pub async fn resize_terminal_session(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
    Json(request): Json<TerminalResizeRequest>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    Ok(Json(
        state
            .terminal_manager
            .resize(&process_id, request.rows, request.cols)
            .await?,
    ))
}

pub async fn interrupt_terminal_session(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    let info = state
        .terminal_manager
        .get(&process_id)
        .await
        .ok_or(AppError::NotFound)?;
    if info.agent_id != query.agent_id {
        return Err(AppError::NotFound);
    }
    Ok(Json(state.terminal_manager.interrupt(&process_id).await?))
}

pub async fn terminate_terminal_session(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(query): Query<ProcessOwnerQuery>,
) -> AppResult<Json<ProcessInfo>> {
    stop_tool_process(State(state), Path(process_id), Query(query)).await
}
