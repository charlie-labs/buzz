use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;

use super::{
    package::{
        copy_source_to_staging, load_package_directory, package_summary, promote_package,
        safe_export, stage_package, stage_package_update,
    },
    store::{list_history, reconcile_publishing_success},
    BindingStore, CreateDaemonBindingRequest, CreateDaemonPackageRequest, DaemonBinding,
    DaemonBindingSummary, DaemonHistoryEntry, DaemonPackageDetail, DaemonPackageSummary,
    ImportDaemonPackageRequest, UpdateDaemonBindingRequest, UpdateDaemonPackageRequest,
};
use crate::commands::managed_agent_channel_message_event_id_by_marker;
use crate::{
    app_state::AppState,
    managed_agents::{
        load_managed_agents, validate_daemon_agent_record_capability, ManagedAgentRuntimeKey,
    },
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
pub fn update_daemon_package(
    request: UpdateDaemonPackageRequest,
    app: AppHandle,
) -> Result<DaemonPackageSummary, String> {
    let state = app.state::<AppState>();
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    let daemon_id = request.daemon_id.trim();
    let existing = super::load_managed_package(&app, daemon_id)?;
    let root = super::daemon_library_root(&app)?;
    let (staging, loaded) = stage_package_update(&root, &existing, request.daemon_md.as_bytes())?;
    promote_package(&root, &staging, daemon_id, true)?;
    Ok(package_summary(&loaded))
}

pub(crate) fn import_daemon_package(
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
pub async fn pick_and_import_daemon_md(
    replace: bool,
    app: AppHandle,
) -> Result<Option<DaemonPackageSummary>, String> {
    let Some(source_path) = pick_file(&app).await? else {
        return Ok(None);
    };
    import_daemon_package(
        ImportDaemonPackageRequest {
            source_path,
            replace,
        },
        app,
    )
    .map(Some)
}

#[tauri::command]
pub async fn pick_and_import_daemon_folder(
    replace: bool,
    app: AppHandle,
) -> Result<Option<DaemonPackageSummary>, String> {
    let Some(source_path) = pick_folder(&app).await? else {
        return Ok(None);
    };
    import_daemon_package(
        ImportDaemonPackageRequest {
            source_path,
            replace,
        },
        app,
    )
    .map(Some)
}

#[tauri::command]
pub async fn pick_daemon_context_folder(app: AppHandle) -> Result<Option<String>, String> {
    let Some(selected) = pick_folder(&app).await? else {
        return Ok(None);
    };
    canonical_context(Some(&selected))
}

#[tauri::command]
pub async fn export_daemon_package_with_picker(
    daemon_id: String,
    app: AppHandle,
) -> Result<bool, String> {
    let Some(parent) = pick_folder(&app).await? else {
        return Ok(false);
    };
    let daemon_id = daemon_id.trim();
    let package = super::load_managed_package(&app, daemon_id)?;
    safe_export(&package.directory, &Path::new(&parent).join(daemon_id))?;
    Ok(true)
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
    let binding_ids = BindingStore::load(&app)?
        .bindings
        .into_iter()
        .filter(|binding| binding.daemon_id == daemon_id)
        .map(|binding| binding.id)
        .collect::<Vec<_>>();
    if !binding_ids.is_empty() {
        return Err(format!(
            "remove all {} daemon binding record(s) before deleting the package (binding IDs: {})",
            binding_ids.len(),
            binding_ids.join(", ")
        ));
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
pub fn get_daemon_schedule_status(
    binding_id: String,
    app: AppHandle,
) -> Result<super::DaemonScheduleStatus, String> {
    super::get_schedule_status(&app, binding_id.trim())
}

#[tauri::command]
pub async fn create_daemon_binding(
    request: CreateDaemonBindingRequest,
    app: AppHandle,
) -> Result<DaemonBindingSummary, String> {
    super::load_managed_package(&app, request.daemon_id.trim())?;
    let key = ManagedAgentRuntimeKey::new(request.agent_pubkey, &request.relay_url)?;
    validate_managed_agent_relationship(&app, &key.pubkey, &key.relay_url)?;
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
    crate::managed_agents::validate_daemon_output_channel(&app, &binding).await?;
    let state = app.state::<AppState>();
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    super::load_managed_package(&app, &binding.daemon_id)?;
    validate_managed_agent_relationship(&app, &binding.agent_pubkey, &binding.relay_url)?;
    let mut store = BindingStore::load(&app)?;
    ensure_primary_binding_available(&store.bindings, &binding.daemon_id, None)?;
    ensure_binding_id_available(&store.bindings, &binding.id)?;
    store.bindings.push(binding.clone());
    store.save(&app)?;
    Ok((&binding).into())
}

#[tauri::command]
pub async fn update_daemon_binding(
    request: UpdateDaemonBindingRequest,
    app: AppHandle,
) -> Result<DaemonBindingSummary, String> {
    let state = app.state::<AppState>();
    let (original, binding) = {
        let _guard = state
            .managed_agents_store_lock
            .lock()
            .map_err(|error| error.to_string())?;
        let binding = BindingStore::load(&app)?
            .bindings
            .into_iter()
            .find(|binding| binding.id == request.id)
            .ok_or("daemon binding not found")?;
        (binding.clone(), binding)
    };
    let binding = apply_daemon_binding_update(
        binding,
        request,
        crate::util::now_iso(),
        |daemon_id| super::load_managed_package(&app, daemon_id).map(|_| ()),
        |pubkey, relay_url| validate_managed_agent_relationship(&app, pubkey, relay_url),
    )?;
    crate::managed_agents::validate_daemon_output_channel(&app, &binding).await?;

    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    let mut store = BindingStore::load(&app)?;
    let index = store
        .bindings
        .iter()
        .position(|candidate| candidate.id == binding.id)
        .ok_or("daemon binding not found")?;
    if store.bindings[index] != original {
        return Err("daemon binding changed while validating; reload and retry the update".into());
    }
    ensure_primary_binding_available(&store.bindings, &binding.daemon_id, Some(&binding.id))?;
    store.bindings[index] = binding.clone();
    store.save(&app)?;
    Ok((&binding).into())
}

pub(crate) fn apply_daemon_binding_update(
    mut binding: DaemonBinding,
    request: UpdateDaemonBindingRequest,
    updated_at: String,
    mut validate_package: impl FnMut(&str) -> Result<(), String>,
    validate_relationship: impl FnOnce(&str, &str) -> Result<(), String>,
) -> Result<DaemonBinding, String> {
    if let Some(daemon_id) = request.daemon_id {
        validate_package(daemon_id.trim())?;
        binding.daemon_id = daemon_id.trim().to_string();
    }
    if request.agent_pubkey.is_some() || request.relay_url.is_some() {
        let key = ManagedAgentRuntimeKey::new(
            request
                .agent_pubkey
                .unwrap_or_else(|| binding.agent_pubkey.clone()),
            request.relay_url.as_deref().unwrap_or(&binding.relay_url),
        )?;
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
    binding.updated_at = updated_at;
    validate_relationship(&binding.agent_pubkey, &binding.relay_url)?;
    Ok(binding)
}

#[tauri::command]
pub fn delete_daemon_binding(binding_id: String, app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let _guard = state
        .managed_agents_store_lock
        .lock()
        .map_err(|error| error.to_string())?;
    let mut store = BindingStore::load(&app)?;
    delete_binding_record(&mut store.bindings, binding_id.trim())?;
    store.save(&app)
}

pub(crate) fn ensure_primary_binding_available(
    bindings: &[DaemonBinding],
    daemon_id: &str,
    exclude_binding_id: Option<&str>,
) -> Result<(), String> {
    if let Some(existing) = bindings.iter().find(|binding| {
        binding.daemon_id == daemon_id && exclude_binding_id != Some(binding.id.as_str())
    }) {
        return Err(format!(
            "daemon {daemon_id} already has primary binding {}; update or delete that binding instead of creating another",
            existing.id
        ));
    }
    Ok(())
}

fn ensure_binding_id_available(bindings: &[DaemonBinding], binding_id: &str) -> Result<(), String> {
    if bindings.iter().any(|binding| binding.id == binding_id) {
        return Err("daemon binding ID already exists; retry creation".into());
    }
    Ok(())
}

pub(crate) fn delete_binding_record(
    bindings: &mut Vec<DaemonBinding>,
    binding_id: &str,
) -> Result<(), String> {
    let matches = bindings
        .iter()
        .filter(|binding| binding.id == binding_id)
        .count();
    match matches {
        0 => Err("daemon binding not found".into()),
        1 => {
            bindings.retain(|binding| binding.id != binding_id);
            Ok(())
        }
        count => Err(format!(
            "daemon binding store contains {count} records with ID {binding_id}; no records were deleted. Repair the duplicate IDs, then retry deletion"
        )),
    }
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

fn validate_managed_agent_relationship(
    app: &AppHandle,
    pubkey: &str,
    relay_url: &str,
) -> Result<(), String> {
    let records = load_managed_agents(app)?;
    let record = records
        .iter()
        .find(|record| record.pubkey.eq_ignore_ascii_case(pubkey))
        .ok_or("managed agent not found")?;
    if record.relay_url.trim_end_matches('/') != relay_url.trim_end_matches('/') {
        return Err("daemon binding relay must match the selected managed agent relay".into());
    }
    validate_daemon_agent_record_capability(app, record)
}

async fn pick_file(app: &AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("DAEMON.md", &["md"])
        .pick_file(move |path| {
            let _ = tx.send(path);
        });
    let selected = rx
        .await
        .map_err(|_| "daemon file picker closed unexpectedly")?;
    selected
        .map(|path| {
            path.as_path()
                .map(|path| path.to_string_lossy().into_owned())
                .ok_or_else(|| "daemon file picker returned a non-filesystem path".into())
        })
        .transpose()
}

async fn pick_folder(app: &AppHandle) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path);
    });
    let selected = rx
        .await
        .map_err(|_| "daemon folder picker closed unexpectedly")?;
    selected
        .map(|path| {
            path.as_path()
                .map(|path| path.to_string_lossy().into_owned())
                .ok_or_else(|| "daemon folder picker returned a non-filesystem path".into())
        })
        .transpose()
}
