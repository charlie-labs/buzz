use std::{
    fs,
    io::Read as _,
    path::{Component, Path, PathBuf},
};

use sha2::{Digest, Sha256};
use tauri::AppHandle;
use uuid::Uuid;

use super::validation::MAX_DAEMON_MD_BYTES;
use super::{parse_daemon_policy, DaemonPackageSummary, LoadedDaemonPackage};
use crate::managed_agents::managed_agents_base_dir;

pub const MAX_PACKAGE_DEPTH: usize = 8;
pub const MAX_PACKAGE_FILES: usize = 256;
pub const MAX_PACKAGE_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_PACKAGE_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn daemon_library_root(app: &AppHandle) -> Result<PathBuf, String> {
    let root = managed_agents_base_dir(app)?
        .join(".agents")
        .join("daemons");
    fs::create_dir_all(&root)
        .map_err(|error| format!("failed to create daemon library: {error}"))?;
    reject_symlink(&root, "daemon library")?;
    Ok(root)
}

pub(crate) fn load_managed_package(
    app: &AppHandle,
    daemon_id: &str,
) -> Result<LoadedDaemonPackage, String> {
    super::validation::validate_daemon_id(daemon_id)?;
    load_package_directory(&daemon_library_root(app)?.join(daemon_id), Some(daemon_id))
}

pub(crate) fn load_package_directory(
    directory: &Path,
    expected_id: Option<&str>,
) -> Result<LoadedDaemonPackage, String> {
    reject_symlink(directory, "daemon package")?;
    if !directory.is_dir() {
        return Err("daemon package directory does not exist".into());
    }
    let files = inspect_package_tree(directory)?;
    let daemon_path = directory.join("DAEMON.md");
    if !files.iter().any(|path| path == Path::new("DAEMON.md")) {
        return Err("daemon package requires a root DAEMON.md".into());
    }
    let daemon_md = fs::read_to_string(&daemon_path)
        .map_err(|error| format!("failed to read DAEMON.md as UTF-8: {error}"))?;
    let policy = parse_daemon_policy(&daemon_md, expected_id)?;
    let package_hash = hash_package_files(directory, &files)?;
    Ok(LoadedDaemonPackage {
        directory: directory.to_path_buf(),
        daemon_md,
        policy,
        package_hash,
        has_scripts: files.iter().any(|path| path.starts_with("scripts")),
        has_references: files.iter().any(|path| path.starts_with("references")),
    })
}

pub(crate) fn package_summary(package: &LoadedDaemonPackage) -> DaemonPackageSummary {
    DaemonPackageSummary {
        id: package.policy.id.clone(),
        purpose: package.policy.purpose.clone(),
        activation_mode: package.policy.activation_mode.clone(),
        package_hash: package.package_hash.clone(),
        watch_count: package.policy.watch.len(),
        routine_count: package.policy.routines.len(),
        has_scripts: package.has_scripts,
        has_references: package.has_references,
    }
}

pub(crate) fn stage_package(
    library: &Path,
    daemon_md: &[u8],
    files: impl IntoIterator<Item = (PathBuf, Vec<u8>, bool)>,
) -> Result<(PathBuf, LoadedDaemonPackage), String> {
    if daemon_md.len() > MAX_DAEMON_MD_BYTES {
        return Err("DAEMON.md exceeds 256 KiB".into());
    }
    let staging_root = library.join(".staging");
    fs::create_dir_all(&staging_root).map_err(|error| error.to_string())?;
    let staging = staging_root.join(Uuid::new_v4().to_string());
    fs::create_dir(&staging).map_err(|error| error.to_string())?;
    let result = (|| {
        write_restricted(&staging.join("DAEMON.md"), daemon_md, false)?;
        for (relative, bytes, executable) in files {
            validate_relative_support_path(&relative)?;
            if bytes.len() as u64 > MAX_PACKAGE_FILE_BYTES {
                return Err(format!("package file {} exceeds 1 MiB", relative.display()));
            }
            let target = staging.join(&relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            write_restricted(
                &target,
                &bytes,
                executable && relative.starts_with("scripts"),
            )?;
        }
        let loaded = load_package_directory(&staging, None)?;
        Ok((staging.clone(), loaded))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

pub(crate) fn copy_source_to_staging(
    library: &Path,
    source: &Path,
) -> Result<(PathBuf, LoadedDaemonPackage), String> {
    reject_symlink(source, "import source")?;
    if source.is_file() {
        if source.file_name().and_then(|value| value.to_str()) != Some("DAEMON.md") {
            return Err("single-file import source must be named DAEMON.md".into());
        }
        let bytes = fs::read(source).map_err(|error| error.to_string())?;
        return stage_package(library, &bytes, []);
    }
    if !source.is_dir() {
        return Err("import source must be a DAEMON.md file or package directory".into());
    }
    let source_id = source
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("daemon package directory name must be valid UTF-8")?;
    load_package_directory(source, Some(source_id))?;
    let paths = inspect_package_tree(source)?;
    let daemon_md = fs::read(source.join("DAEMON.md")).map_err(|error| error.to_string())?;
    let mut support = Vec::new();
    for relative in paths
        .into_iter()
        .filter(|path| path != Path::new("DAEMON.md"))
    {
        let source_path = source.join(&relative);
        let bytes = fs::read(&source_path).map_err(|error| error.to_string())?;
        support.push((relative, bytes, is_executable(&source_path)));
    }
    stage_package(library, &daemon_md, support)
}

pub(crate) fn promote_package(
    library: &Path,
    staging: &Path,
    daemon_id: &str,
    replace: bool,
) -> Result<(), String> {
    let target = library.join(daemon_id);
    if !target.exists() {
        return fs::rename(staging, target)
            .map_err(|error| format!("failed to promote daemon package: {error}"));
    }
    if !replace {
        return Err(format!("daemon package {daemon_id} already exists"));
    }
    reject_symlink(&target, "existing daemon package")?;
    let backup = library.join(format!(".{daemon_id}.backup-{}", Uuid::new_v4()));
    fs::rename(&target, &backup)
        .map_err(|error| format!("failed to back up existing package: {error}"))?;
    match fs::rename(staging, &target) {
        Ok(()) => {
            if let Err(error) = fs::remove_dir_all(&backup) {
                return Err(format!(
                    "replacement succeeded but old package backup cleanup failed: {error}"
                ));
            }
            Ok(())
        }
        Err(error) => {
            let rollback = fs::rename(&backup, &target);
            match rollback {
                Ok(()) => Err(format!(
                    "failed to promote replacement; old package restored: {error}"
                )),
                Err(rollback_error) => Err(format!(
                    "failed to promote replacement ({error}) and restore backup ({rollback_error})"
                )),
            }
        }
    }
}

pub(crate) fn safe_export(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        return Err("export destination must not already exist".into());
    }
    let paths = inspect_package_tree(source)?;
    fs::create_dir(destination).map_err(|error| error.to_string())?;
    let result = (|| {
        for relative in paths {
            let from = source.join(&relative);
            let to = destination.join(&relative);
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let bytes = fs::read(&from).map_err(|error| error.to_string())?;
            write_restricted(
                &to,
                &bytes,
                is_executable(&from) && relative.starts_with("scripts"),
            )?;
        }
        load_package_directory(
            destination,
            source.file_name().and_then(|value| value.to_str()),
        )?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result
}

fn inspect_package_tree(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut aggregate = 0_u64;
    inspect_directory(root, root, 0, &mut files, &mut aggregate)?;
    files.sort_by_key(|path| slash_path(path));
    Ok(files)
}

fn inspect_directory(
    root: &Path,
    directory: &Path,
    depth: usize,
    files: &mut Vec<PathBuf>,
    aggregate: &mut u64,
) -> Result<(), String> {
    if depth > MAX_PACKAGE_DEPTH {
        return Err(format!(
            "daemon package exceeds maximum depth {MAX_PACKAGE_DEPTH}"
        ));
    }
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "daemon package paths must be unambiguous UTF-8")?;
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            return Err("daemon package contains an unsafe path component".into());
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!("symlinks are not allowed: {}", path.display()));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "package path escaped source root")?
            .to_path_buf();
        validate_root_entry(&relative)?;
        if metadata.is_dir() {
            inspect_directory(root, &path, depth + 1, files, aggregate)?;
        } else if metadata.is_file() {
            if metadata.len() > MAX_PACKAGE_FILE_BYTES && relative != Path::new("DAEMON.md") {
                return Err(format!("package file {} exceeds 1 MiB", relative.display()));
            }
            if relative == Path::new("DAEMON.md") && metadata.len() > MAX_DAEMON_MD_BYTES as u64 {
                return Err("DAEMON.md exceeds 256 KiB".into());
            }
            *aggregate = aggregate.saturating_add(metadata.len());
            if *aggregate > MAX_PACKAGE_BYTES {
                return Err("daemon package exceeds 8 MiB aggregate size".into());
            }
            files.push(relative);
            if files.len() > MAX_PACKAGE_FILES {
                return Err(format!("daemon package exceeds {MAX_PACKAGE_FILES} files"));
            }
        } else {
            return Err(format!(
                "special filesystem entries are not allowed: {}",
                path.display()
            ));
        }
    }
    Ok(())
}

fn validate_root_entry(relative: &Path) -> Result<(), String> {
    match relative.components().next() {
        Some(Component::Normal(name))
            if name == "DAEMON.md" || name == "scripts" || name == "references" =>
        {
            Ok(())
        }
        _ => Err(format!(
            "unexpected daemon package entry: {}",
            relative.display()
        )),
    }
}

fn validate_relative_support_path(relative: &Path) -> Result<(), String> {
    if relative.components().count() > MAX_PACKAGE_DEPTH + 1 {
        return Err(format!(
            "daemon package exceeds maximum depth {MAX_PACKAGE_DEPTH}"
        ));
    }
    if relative == Path::new("DAEMON.md") {
        return Err("support files cannot replace DAEMON.md".into());
    }
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("support file path must be a safe relative path".into());
    }
    let portable = relative
        .to_str()
        .ok_or("support file path must be valid UTF-8")?;
    if portable.contains('\\') {
        return Err("support file paths must use portable slash separators".into());
    }
    validate_root_entry(relative)
}

fn hash_package_files(root: &Path, files: &[PathBuf]) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for relative in files {
        let slash = slash_path(relative);
        let mut file = fs::File::open(root.join(relative)).map_err(|error| error.to_string())?;
        let length = file.metadata().map_err(|error| error.to_string())?.len();
        hasher.update((slash.len() as u64).to_be_bytes());
        hasher.update(slash.as_bytes());
        hasher.update(length.to_be_bytes());
        let mut buffer = [0_u8; 32 * 1024];
        loop {
            let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
    }
    Ok(hex::encode(hasher.finalize()))
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn reject_symlink(path: &Path, label: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {label}: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{label} must not be a symlink"));
    }
    Ok(())
}

fn write_restricted(path: &Path, bytes: &[u8], executable: bool) -> Result<(), String> {
    use std::io::Write as _;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if executable { 0o700 } else { 0o600 });
    }
    let mut file = options.open(path).map_err(|error| error.to_string())?;
    file.write_all(bytes).map_err(|error| error.to_string())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}
