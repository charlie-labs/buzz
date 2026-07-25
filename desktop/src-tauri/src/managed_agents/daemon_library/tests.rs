use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::{
    commands::{delete_binding_record, ensure_primary_binding_available},
    package::{
        copy_source_to_staging, load_package_directory, promote_package, safe_export,
        stage_package, stage_package_update, MAX_PACKAGE_DEPTH, MAX_PACKAGE_FILES,
    },
    store::{read_run_path, reserve_run_at_path, write_run_path, Reservation},
    DaemonActivationMode, DaemonBinding, DaemonRunLifecycle, DaemonRunStatus, DaemonRunTrigger,
    UpdateDaemonBindingRequest,
};

fn daemon_md(id: &str, activation: &str) -> String {
    format!(
        "---\nid: {id}\npurpose: Keep a bounded test state healthy.\n{activation}routines:\n  - Inspect one bounded state.\ndeny:\n  - Never expose secrets.\n---\n# Policy\n\nAct conservatively.\n"
    )
}

#[test]
fn canonical_policy_accepts_watch_schedule_and_hybrid() {
    let watch = super::parse_daemon_policy(
        &daemon_md("watcher", "watch:\n  - A pull request changes.\n"),
        Some("watcher"),
    )
    .unwrap();
    assert_eq!(watch.activation_mode, DaemonActivationMode::WatchOnly);

    let schedule = super::parse_daemon_policy(
        &daemon_md("scheduled", "schedule: \"0 9 * * 1\"\n"),
        Some("scheduled"),
    )
    .unwrap();
    assert_eq!(schedule.activation_mode, DaemonActivationMode::ScheduleOnly);

    let hybrid = super::parse_daemon_policy(
        &daemon_md(
            "hybrid",
            "watch:\n  - A Linear issue changes.\nschedule: \"*/15 * * * *\"\n",
        ),
        Some("hybrid"),
    )
    .unwrap();
    assert_eq!(hybrid.activation_mode, DaemonActivationMode::Hybrid);

    let frequent = super::parse_daemon_policy(
        &daemon_md("frequent", "schedule: \"*/2 * * * *\"\n"),
        Some("frequent"),
    )
    .unwrap();
    assert_eq!(frequent.schedule.as_deref(), Some("*/2 * * * *"));
}

#[test]
fn canonical_policy_rejects_unknown_missing_empty_cron_and_id_mismatch() {
    let unknown = daemon_md(
        "bad",
        "watch:\n  - A pull request changes.\nname: forbidden\n",
    );
    assert!(super::parse_daemon_policy(&unknown, Some("bad"))
        .unwrap_err()
        .contains("unknown field"));
    assert!(
        super::parse_daemon_policy(&daemon_md("bad", ""), Some("bad"))
            .unwrap_err()
            .contains("wake source")
    );
    assert!(
        super::parse_daemon_policy(&daemon_md("bad", "watch:\n  - \"\"\n"), Some("bad")).is_err()
    );
    assert!(
        super::parse_daemon_policy(&daemon_md("bad", "schedule: \"0 1 * *\"\n"), Some("bad"))
            .unwrap_err()
            .contains("exactly five")
    );
    assert!(super::parse_daemon_policy(
        &daemon_md("bad", "schedule: \"0 1 * * * extra\"\n"),
        Some("bad")
    )
    .unwrap_err()
    .contains("exactly five"));
    assert!(super::parse_daemon_policy(
        &daemon_md("actual", "watch:\n  - A change occurs.\n"),
        Some("expected")
    )
    .unwrap_err()
    .contains("match its directory"));

    let yaml_error =
        super::parse_daemon_policy(&daemon_md("bad", "schedule: */2 * * * *\n"), Some("bad"))
            .unwrap_err();
    assert!(yaml_error.contains("invalid DAEMON.md frontmatter"));
    assert!(yaml_error.contains("YAML-reserved syntax"));
    assert!(yaml_error.contains("schedule: \"*/2 * * * *\""));
}

#[test]
fn package_import_copies_single_file_and_folder_and_hashes_content() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    std::fs::create_dir(&library).unwrap();
    let single_dir = temp.path().join("single");
    std::fs::create_dir(&single_dir).unwrap();
    let single = single_dir.join("DAEMON.md");
    std::fs::write(
        &single,
        daemon_md("single", "watch:\n  - A change occurs.\n"),
    )
    .unwrap();
    let (staging, _loaded) = copy_source_to_staging(&library, &single).unwrap();
    promote_package(&library, &staging, "single", false).unwrap();
    std::fs::write(&single, "mutated source").unwrap();
    assert!(load_package_directory(&library.join("single"), Some("single")).is_ok());

    let folder = temp.path().join("folder");
    std::fs::create_dir_all(folder.join("scripts")).unwrap();
    std::fs::create_dir_all(folder.join("references")).unwrap();
    std::fs::write(
        folder.join("DAEMON.md"),
        daemon_md("folder", "schedule: \"0 * * * *\"\n"),
    )
    .unwrap();
    std::fs::write(folder.join("scripts/check.sh"), b"#!/bin/sh\nexit 0\n").unwrap();
    std::fs::write(folder.join("references/policy.md"), b"policy\n").unwrap();
    let (_, first) = copy_source_to_staging(&library, &folder).unwrap();
    let first_hash = first.package_hash.clone();
    let (_, second) = copy_source_to_staging(&library, &folder).unwrap();
    assert_eq!(first_hash, second.package_hash);
    std::fs::write(folder.join("references/policy.md"), b"changed\n").unwrap();
    let (_, changed) = copy_source_to_staging(&library, &folder).unwrap();
    assert_ne!(first_hash, changed.package_hash);
}

#[cfg(unix)]
#[test]
fn package_rejects_symlinks_and_unexpected_entries() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("package");
    std::fs::create_dir_all(package.join("scripts")).unwrap();
    std::fs::write(
        package.join("DAEMON.md"),
        daemon_md("package", "watch:\n  - A change occurs.\n"),
    )
    .unwrap();
    symlink("DAEMON.md", package.join("scripts/link")).unwrap();
    assert!(load_package_directory(&package, Some("package"))
        .unwrap_err()
        .contains("symlinks"));
    std::fs::remove_file(package.join("scripts/link")).unwrap();
    std::fs::write(package.join("unexpected.txt"), b"no").unwrap();
    assert!(load_package_directory(&package, Some("package"))
        .unwrap_err()
        .contains("unexpected"));
}

#[test]
fn package_rejects_oversize_depth_and_count() {
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("package");
    std::fs::create_dir_all(package.join("scripts")).unwrap();
    std::fs::write(
        package.join("DAEMON.md"),
        daemon_md("package", "watch:\n  - A change occurs.\n"),
    )
    .unwrap();
    std::fs::write(package.join("scripts/huge"), vec![0_u8; 1024 * 1024 + 1]).unwrap();
    assert!(load_package_directory(&package, Some("package")).is_err());
    std::fs::remove_file(package.join("scripts/huge")).unwrap();

    let deep = (0..=MAX_PACKAGE_DEPTH).fold(package.join("scripts"), |path, index| {
        path.join(index.to_string())
    });
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("file"), b"x").unwrap();
    assert!(load_package_directory(&package, Some("package")).is_err());
    std::fs::remove_dir_all(package.join("scripts")).unwrap();
    std::fs::create_dir(package.join("scripts")).unwrap();
    for index in 0..MAX_PACKAGE_FILES {
        std::fs::write(package.join("scripts").join(index.to_string()), b"x").unwrap();
    }
    assert!(load_package_directory(&package, Some("package"))
        .unwrap_err()
        .contains("files"));
}

#[test]
fn duplicate_replace_and_export_are_non_destructive() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    std::fs::create_dir(&library).unwrap();
    let original = daemon_md("safe", "watch:\n  - A change occurs.\n");
    let (staging, _) = stage_package(&library, original.as_bytes(), []).unwrap();
    promote_package(&library, &staging, "safe", false).unwrap();
    let (duplicate, _) = stage_package(&library, original.as_bytes(), []).unwrap();
    assert!(promote_package(&library, &duplicate, "safe", false).is_err());
    assert_eq!(
        std::fs::read_to_string(library.join("safe/DAEMON.md")).unwrap(),
        original
    );
    assert!(stage_package(&library, b"invalid", []).is_err());
    assert_eq!(
        std::fs::read_to_string(library.join("safe/DAEMON.md")).unwrap(),
        original
    );

    let export = temp.path().join("exported");
    safe_export(&library.join("safe"), &export).unwrap();
    assert!(safe_export(&library.join("safe"), &export).is_err());
}

#[test]
fn atomic_package_update_preserves_support_tree_and_rejects_id_changes() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("library");
    std::fs::create_dir(&library).unwrap();
    let original = daemon_md("editable", "schedule: \"0 * * * *\"\n");
    let (staging, _) = stage_package(
        &library,
        original.as_bytes(),
        [(
            PathBuf::from("scripts/check.sh"),
            b"#!/bin/sh\necho ok\n".to_vec(),
            true,
        )],
    )
    .unwrap();
    promote_package(&library, &staging, "editable", false).unwrap();
    let existing = load_package_directory(&library.join("editable"), Some("editable")).unwrap();
    let updated = daemon_md("editable", "schedule: \"15 * * * *\"\n");
    let (replacement, loaded) =
        stage_package_update(&library, &existing, updated.as_bytes()).unwrap();
    promote_package(&library, &replacement, "editable", true).unwrap();
    assert_eq!(
        std::fs::read(library.join("editable/scripts/check.sh")).unwrap(),
        b"#!/bin/sh\necho ok\n"
    );
    assert_ne!(existing.package_hash, loaded.package_hash);

    let installed = load_package_directory(&library.join("editable"), Some("editable")).unwrap();
    let wrong_id = daemon_md("renamed", "schedule: \"15 * * * *\"\n");
    assert!(stage_package_update(&library, &installed, wrong_id.as_bytes()).is_err());
    assert_eq!(
        std::fs::read_to_string(library.join("editable/DAEMON.md")).unwrap(),
        updated
    );
}

fn binding() -> DaemonBinding {
    let now = "2026-07-24T00:00:00Z".to_string();
    DaemonBinding {
        id: Uuid::new_v4().to_string(),
        daemon_id: "safe".into(),
        agent_pubkey: "aa".repeat(32),
        relay_url: "wss://relay.example".into(),
        channel_id: Uuid::new_v4().to_string(),
        context_directory: None,
        context_configured: false,
        schedule_enabled: false,
        created_at: now.clone(),
        updated_at: now,
    }
}

#[test]
fn update_context_distinguishes_omission_and_explicit_null() {
    let omitted: UpdateDaemonBindingRequest = serde_json::from_value(serde_json::json!({
        "id": Uuid::new_v4().to_string()
    }))
    .unwrap();
    assert_eq!(omitted.context_directory, None);
    let cleared: UpdateDaemonBindingRequest = serde_json::from_value(serde_json::json!({
        "id": Uuid::new_v4().to_string(),
        "contextDirectory": null
    }))
    .unwrap();
    assert_eq!(cleared.context_directory, Some(None));
}

#[test]
fn primary_binding_rejects_duplicate_daemon_but_allows_self_update() {
    let existing = binding();
    let error = ensure_primary_binding_available(
        std::slice::from_ref(&existing),
        &existing.daemon_id,
        None,
    )
    .unwrap_err();
    assert!(error.contains("already has primary binding"));
    assert!(error.contains("update or delete"));
    assert!(ensure_primary_binding_available(
        std::slice::from_ref(&existing),
        &existing.daemon_id,
        Some(&existing.id),
    )
    .is_ok());
}

#[test]
fn binding_deletion_refuses_legacy_duplicate_ids_without_deleting_anything() {
    let first = binding();
    let mut duplicate = first.clone();
    duplicate.daemon_id = "legacy-duplicate".into();
    let mut bindings = vec![first.clone(), duplicate];
    let error = delete_binding_record(&mut bindings, &first.id).unwrap_err();
    assert!(error.contains("2 records"));
    assert!(error.contains("no records were deleted"));
    assert_eq!(bindings.len(), 2);

    bindings[1].id = Uuid::new_v4().to_string();
    delete_binding_record(&mut bindings, &first.id).unwrap();
    assert_eq!(bindings.len(), 1);
}

#[test]
fn durable_run_reservation_is_idempotent_and_conflict_safe() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("run.json");
    let binding = binding();
    let run_id = Uuid::new_v4().to_string();
    let Reservation::New(mut record) = reserve_run_at_path(
        &path,
        &run_id,
        &binding,
        "package-hash",
        "safe wake",
        DaemonRunTrigger::Manual,
        None,
    )
    .unwrap() else {
        panic!("expected new reservation")
    };
    record.lifecycle = DaemonRunLifecycle::Terminal;
    record.status = Some(DaemonRunStatus::Succeeded);
    record.completed_at = Some("2026-07-24T00:01:00Z".into());
    write_run_path(&path, &record).unwrap();
    assert!(matches!(
        reserve_run_at_path(
            &path,
            &run_id,
            &binding,
            "package-hash",
            "safe wake",
            DaemonRunTrigger::Manual,
            None,
        )
        .unwrap(),
        Reservation::ExistingTerminal(_)
    ));
    assert!(reserve_run_at_path(
        &path,
        &run_id,
        &binding,
        "different-hash",
        "safe wake",
        DaemonRunTrigger::Manual,
        None,
    )
    .unwrap_err()
    .contains("conflicts"));
}

#[test]
fn no_op_terminal_receipt_serializes_without_publication_state() {
    assert_eq!(
        serde_json::to_value(DaemonRunStatus::NoOp).unwrap(),
        serde_json::json!("no_op")
    );
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("run.json");
    let binding = binding();
    let run_id = Uuid::new_v4().to_string();
    let Reservation::New(mut record) = reserve_run_at_path(
        &path,
        &run_id,
        &binding,
        "package-hash",
        "wake",
        DaemonRunTrigger::Manual,
        None,
    )
    .unwrap() else {
        panic!("expected new reservation")
    };
    record.lifecycle = DaemonRunLifecycle::Terminal;
    record.status = Some(DaemonRunStatus::NoOp);
    record.acp_session_id = Some("session-1".into());
    record.completed_at = Some("2026-07-24T00:01:00Z".into());
    write_run_path(&path, &record).unwrap();

    let stored = read_run_path(&path).unwrap().unwrap();
    assert_eq!(stored.status, Some(DaemonRunStatus::NoOp));
    assert_eq!(stored.acp_session_id.as_deref(), Some("session-1"));
    assert!(stored.output_event_id.is_none());
    assert!(stored.publishing_at.is_none());
}

#[test]
fn terminal_receipts_are_immutable_and_publishing_is_recoverable() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("run.json");
    let binding = binding();
    let run_id = Uuid::new_v4().to_string();
    let Reservation::New(mut record) = reserve_run_at_path(
        &path,
        &run_id,
        &binding,
        "package-hash",
        "wake",
        DaemonRunTrigger::Manual,
        None,
    )
    .unwrap() else {
        panic!("expected new reservation")
    };
    record.lifecycle = DaemonRunLifecycle::Publishing;
    record.publishing_at = Some("2026-07-24T00:01:00Z".into());
    write_run_path(&path, &record).unwrap();
    assert_eq!(
        read_run_path(&path).unwrap().unwrap().lifecycle,
        DaemonRunLifecycle::Publishing
    );
    record.lifecycle = DaemonRunLifecycle::Terminal;
    record.status = Some(DaemonRunStatus::Succeeded);
    record.output_event_id = Some("event-1".into());
    record.completed_at = Some("2026-07-24T00:02:00Z".into());
    write_run_path(&path, &record).unwrap();
    let mut overwritten = record.clone();
    overwritten.output_event_id = Some("event-2".into());
    assert!(write_run_path(&path, &overwritten)
        .unwrap_err()
        .contains("immutable"));
}

#[test]
fn legacy_receipts_do_not_gain_current_package_hashes() {
    let legacy = serde_json::json!({
        "runId": Uuid::new_v4().to_string(),
        "daemonId": "safe",
        "status": "succeeded",
        "startedAt": "2026-07-24T00:00:00Z",
        "completedAt": "2026-07-24T00:01:00Z",
        "agentPubkey": "aa",
        "channelId": Uuid::new_v4().to_string(),
        "relayUrl": "wss://relay.example",
        "cwd": "/not/exposed",
        "contextSnapshot": {"not": "migrated"},
        "outputEventId": "event"
    });
    assert!(legacy.get("packageHash").is_none());
}

#[test]
fn schedule_disabled_does_not_disable_manual_trigger_shape() {
    let binding = binding();
    assert!(!binding.schedule_enabled);
    assert_eq!(DaemonRunTrigger::Manual, DaemonRunTrigger::default());
}

#[test]
fn package_paths_are_portable_slash_relative() {
    let path = PathBuf::from("scripts").join("check.sh");
    assert!(!path.is_absolute());
    assert_eq!(path.components().count(), 2);
    assert_ne!(path, Path::new("DAEMON.md"));
}
