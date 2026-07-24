use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;

use super::{
    package::{
        copy_source_to_staging, load_package_directory, package_summary, promote_package,
        safe_export, stage_package,
    },
    store::{list_history, reconcile_publishing_success},
    BindingStore, CreateDaemonBindingRequest, CreateDaemonPackageRequest, DaemonBinding,
    DaemonBindingSummary, DaemonHistoryEntry, DaemonPackageDetail, DaemonPackageSummary,
    ExportDaemonPackageRequest, ImportDaemonPackageRequest, UpdateDaemonBindingRequest,
};
use crate::commands::managed_agent_channel_message_event_id_by_marker;
use crate::{
    app_state::AppState,
    managed_agents::{load_managed_agents, ManagedAgentRuntimeKey},
};

#[tauri::command]
pub fn list_daemon_packages(app: AppHandle) -> Result<Vec<DaemonPackageSummary>, String> {
    let root = super::daemon_library_root(&app)?;
    let mut packages = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = match entry.file_name().into_string() {
            Ok(name) if !name.starts_with('.') => name,
            _ => continue,
        };
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        packages.push(package_summary(&load_package_directory(
            &entry.path(),
            Some(&name),
        )?));
    }
    packages.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(packages)
}

#[tauri::command]
pub fn get_daemon_package(
    daemon_id: String,
    app: AppHandle,
) -> Result<DaemonPackageDetail, String> {
    let package = super::load_managed_package(&app, daemon_id.trim())?;
    Ok(DaemonPackageDetail {
        summary: package_summary(&package),
        daemon_md: package.daemon_md,
    })
}

#[tauri::command]
pub fn create_daemon_package(
    request: CreateDaemonPackageRequest,
    app: AppHandle,
) -> Result<DaemonPackageSummary, String> {
    let state = app.state::<AppState>();
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    let root = super::daemon_library_root(&app)?;
    let support = request
        .files
        .into_iter()
        .map(|file| (PathBuf::from(file.path), file.bytes, file.executable));
    let (staging, loaded) = stage_package(&root, request.daemon_md.as_bytes(), support)?;
    promote_package(&root, &staging, &loaded.policy.id, false)?;
    Ok(package_summary(&loaded))
}

#[tauri::command]
pub fn import_daemon_package(
    request: ImportDaemonPackageRequest,
    app: AppHandle,
) -> Result<DaemonPackageSummary, String> {
    let state = app.state::<AppState>();
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    let source = canonical_source(&request.source_path)?;
    let root = super::daemon_library_root(&app)?;
    let (staging, loaded) = copy_source_to_staging(&root, &source)?;
    promote_package(&root, &staging, &loaded.policy.id, request.replace)?;
    Ok(package_summary(&loaded))
}

#[tauri::command]
pub fn export_daemon_package(
    request: ExportDaemonPackageRequest,
    app: AppHandle,
) -> Result<(), String> {
    let package = super::load_managed_package(&app, request.daemon_id.trim())?;
    safe_export(
        &package.directory,
        &absolute_destination(&request.destination_path)?,
    )
}

#[tauri::command]
pub fn delete_daemon_package(daemon_id: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    let daemon_id = daemon_id.trim();
    let package = super::load_managed_package(&app, daemon_id)?;
    if BindingStore::load(&app)?
        .bindings
        .iter()
        .any(|binding| binding.daemon_id == daemon_id)
    {
        return Err("remove daemon bindings before deleting the package".into());
    }
    std::fs::remove_dir_all(package.directory).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_daemon_library_folder(app: AppHandle) -> Result<(), String> {
    let root = super::daemon_library_root(&app)?;
    app.opener()
        .open_path(root.to_string_lossy(), None::<&str>)
        .map_err(|error| format!("failed to open daemon library folder: {error}"))
}

#[tauri::command]
pub fn list_daemon_bindings(app: AppHandle) -> Result<Vec<DaemonBindingSummary>, String> {
    let store = BindingStore::load(&app)?;
    let mut bindings = store
        .bindings
        .iter()
        .map(DaemonBindingSummary::from)
        .collect::<Vec<_>>();
    bindings.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    Ok(bindings)
}

#[tauri::command]
pub fn get_daemon_binding(
    binding_id: String,
    app: AppHandle,
) -> Result<DaemonBindingSummary, String> {
    super::get_binding_internal(&app, binding_id.trim()).map(|binding| (&binding).into())
}

#[tauri::command]
pub fn create_daemon_binding(
    request: CreateDaemonBindingRequest,
    app: AppHandle,
) -> Result<DaemonBindingSummary, String> {
    let state = app.state::<AppState>();
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    super::load_managed_package(&app, request.daemon_id.trim())?;
    let key = ManagedAgentRuntimeKey::new(request.agent_pubkey, &request.relay_url)?;
    validate_managed_agent_exists(&app, &key.pubkey)?;
    let channel_id = Uuid::parse_str(request.channel_id.trim())
        .map_err(|_| "invalid channel UUID")?
        .to_string();
    let context_directory = canonical_context(request.context_directory.as_deref())?;
    let now = crate::util::now_iso();
    let binding = DaemonBinding {
        id: Uuid::new_v4().to_string(),
        daemon_id: request.daemon_id.trim().to_string(),
        agent_pubkey: key.pubkey,
        relay_url: key.relay_url,
        channel_id,
        context_configured: context_directory.is_some(),
        context_directory,
        schedule_enabled: request.schedule_enabled,
        created_at: now.clone(),
        updated_at: now,
    };
    let mut store = BindingStore::load(&app)?;
    store.bindings.push(binding.clone());
    store.save(&app)?;
    Ok((&binding).into())
}

#[tauri::command]
pub fn update_daemon_binding(
    request: UpdateDaemonBindingRequest,
    app: AppHandle,
) -> Result<DaemonBindingSummary, String> {
    let state = app.state::<AppState>();
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    let mut store = BindingStore::load(&app)?;
    let binding = store
        .bindings
        .iter_mut()
        .find(|binding| binding.id == request.id)
        .ok_or("daemon binding not found")?;
    if let Some(daemon_id) = request.daemon_id {
        super::load_managed_package(&app, daemon_id.trim())?;
        binding.daemon_id = daemon_id.trim().to_string();
    }
    if request.agent_pubkey.is_some() || request.relay_url.is_some() {
        let key = ManagedAgentRuntimeKey::new(
            request
                .agent_pubkey
                .unwrap_or_else(|| binding.agent_pubkey.clone()),
            request.relay_url.as_deref().unwrap_or(&binding.relay_url),
        )?;
        validate_managed_agent_exists(&app, &key.pubkey)?;
        binding.agent_pubkey = key.pubkey;
        binding.relay_url = key.relay_url;
    }
    if let Some(channel_id) = request.channel_id {
        binding.channel_id = Uuid::parse_str(channel_id.trim())
            .map_err(|_| "invalid channel UUID")?
            .to_string();
    }
    if let Some(context_patch) = request.context_directory {
        binding.context_directory = canonical_context(context_patch.as_deref())?;
        binding.context_configured = binding.context_directory.is_some();
    }
    if let Some(enabled) = request.schedule_enabled {
        binding.schedule_enabled = enabled;
    }
    binding.updated_at = crate::util::now_iso();
    let result = binding.clone();
    store.save(&app)?;
    Ok((&result).into())
}

#[tauri::command]
pub fn delete_daemon_binding(binding_id: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    let mut store = BindingStore::load(&app)?;
    let previous = store.bindings.len();
    store
        .bindings
        .retain(|binding| binding.id != binding_id.trim());
    if store.bindings.len() == previous {
        return Err("daemon binding not found".into());
    }
    store.save(&app)
}

#[tauri::command]
pub async fn list_daemon_run_history(app: AppHandle) -> Result<Vec<DaemonHistoryEntry>, String> {
    let state = app.state::<AppState>();
    let mut entries = {
        let _guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        list_history(&app)?
    };
    for entry in &mut entries {
        let DaemonHistoryEntry::Managed(record) = entry else {
            continue;
        };
        if record.lifecycle != super::DaemonRunLifecycle::Publishing {
            continue;
        }
        let marker = format!("daemon-run:{}", record.run_id);
        let Some(event_id) = managed_agent_channel_message_event_id_by_marker(
            &state,
            &record.binding.agent_pubkey,
            &record.binding.channel_id,
            &marker,
        )
        .await?
        else {
            continue;
        };
        let recovered = {
            let _guard = state
                .managed_agents_store_lock
                .lock()
                .map_err(|error| error.to_string())?;
            reconcile_publishing_success(&app, &record.run_id, event_id)?
        };
        **record = recovered;
    }
    Ok(entries)
}

pub(crate) fn finish_publishing_recovery(
    app: &AppHandle,
    run_id: &str,
    event_id: String,
) -> Result<super::DaemonRunRecord, String> {
    reconcile_publishing_success(app, run_id, event_id)
}

pub(crate) fn revalidate_binding_context(
    binding: &DaemonBinding,
) -> Result<Option<PathBuf>, String> {
    canonical_context(binding.context_directory.as_deref()).map(|value| value.map(PathBuf::from))
}

fn canonical_context(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = std::fs::canonicalize(value)
        .map_err(|error| format!("invalid daemon context directory: {error}"))?;
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("daemon context must be an existing non-symlink directory".into());
    }
    path.to_str()
        .map(|value| Some(value.to_string()))
        .ok_or_else(|| "daemon context directory must be valid UTF-8".into())
}

fn canonical_source(value: &str) -> Result<PathBuf, String> {
    if value.trim().is_empty() {
        return Err("import source path is required".into());
    }
    std::fs::canonicalize(value).map_err(|error| format!("invalid import source: {error}"))
}

fn absolute_destination(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value.trim());
    if !path.is_absolute() || path.file_name().is_none() {
        return Err("export destination must be a non-empty absolute package path".into());
    }
    let parent = path
        .parent()
        .ok_or("export destination requires a parent directory")?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|error| format!("invalid export destination parent: {error}"))?;
    Ok(parent.join(path.file_name().ok_or("invalid export destination")?))
}

fn validate_managed_agent_exists(app: &AppHandle, pubkey: &str) -> Result<(), String> {
    if load_managed_agents(app)?
        .iter()
        .any(|record| record.pubkey.eq_ignore_ascii_case(pubkey))
    {
        Ok(())
    } else {
        Err("managed agent not found".into())
    }
}
