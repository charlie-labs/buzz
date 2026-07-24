use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
    sync::atomic::Ordering,
    time::Duration as StdDuration,
};

use chrono::{DateTime, Duration, Timelike as _, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    binding_has_active_run, get_binding_internal, load_managed_package, record_scheduler_receipt,
    BindingStore, DaemonBinding, DaemonRunStatus, DaemonRunTrigger, DaemonScheduleReadiness,
    DaemonScheduleStatus, DaemonSchedulerDecision, DaemonSchedulerDecisionKind,
    RunManagedDaemonRequest,
};
use crate::{app_state::AppState, managed_agents::run_managed_daemon};

const SCHEDULER_NAMESPACE: Uuid = Uuid::from_u128(0x7f4f_9cc2_c315_4a99_994b_15fb_c48e_6081);
const MAX_CLAIMS: usize = 2_000;

pub(crate) struct DaemonSchedulerRuntime {
    cancellation: CancellationToken,
    task: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchedulerStore {
    #[serde(default)]
    cursors: BTreeMap<String, SchedulerCursor>,
    #[serde(default)]
    claims: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchedulerCursor {
    schedule_hash: String,
    observed_through_utc: String,
    last_decision: Option<DaemonSchedulerDecision>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DueOccurrences {
    latest_missed: Option<DateTime<Utc>>,
    current: Option<DateTime<Utc>>,
}

pub(crate) fn start_daemon_scheduler(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut slot = state
        .daemon_scheduler
        .lock()
        .map_err(|error| error.to_string())?;
    if slot.is_some() {
        return Ok(());
    }
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let scheduler_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        if let Err(error) = process_scheduler_tick(&scheduler_app, Utc::now()).await {
            eprintln!("buzz-desktop: daemon scheduler reconciliation failed: {error}");
        }
        loop {
            let now = Utc::now();
            let next_minute = minute_floor(now) + Duration::minutes(1);
            let wait = (next_minute - now)
                .to_std()
                .unwrap_or_else(|_| StdDuration::from_secs(1));
            tokio::select! {
                _ = task_cancellation.cancelled() => break,
                _ = tokio::time::sleep(wait) => {}
            }
            if task_cancellation.is_cancelled()
                || scheduler_app
                    .state::<AppState>()
                    .shutdown_started
                    .load(Ordering::Acquire)
            {
                break;
            }
            if let Err(error) = process_scheduler_tick(&scheduler_app, Utc::now()).await {
                eprintln!("buzz-desktop: daemon scheduler tick failed: {error}");
            }
        }
    });
    *slot = Some(DaemonSchedulerRuntime { cancellation, task });
    Ok(())
}

pub(crate) fn stop_daemon_scheduler(app: &AppHandle) {
    let runtime = app
        .state::<AppState>()
        .daemon_scheduler
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    if let Some(runtime) = runtime {
        runtime.cancellation.cancel();
        runtime.task.abort();
    }
}

pub(crate) fn get_schedule_status(
    app: &AppHandle,
    binding_id: &str,
) -> Result<DaemonScheduleStatus, String> {
    let binding = get_binding_internal(app, binding_id)?;
    let store = SchedulerStore::load(app)?;
    schedule_status_for_binding(app, &binding, &store, Utc::now())
}

fn schedule_status_for_binding(
    app: &AppHandle,
    binding: &DaemonBinding,
    store: &SchedulerStore,
    now: DateTime<Utc>,
) -> Result<DaemonScheduleStatus, String> {
    let package = match load_managed_package(app, &binding.daemon_id) {
        Ok(package) => package,
        Err(error) => {
            return Ok(status(
                binding,
                None,
                None,
                DaemonScheduleReadiness::MissingPackage,
                Some(error),
                None,
                store,
            ));
        }
    };
    let Some(expression) = package.policy.schedule.as_deref() else {
        return Ok(status(
            binding,
            None,
            None,
            DaemonScheduleReadiness::WatchOnly,
            Some("DAEMON.md has no schedule; manual Run now remains available".into()),
            None,
            store,
        ));
    };
    let hash = schedule_hash(expression);
    let schedule = match parse_schedule(expression) {
        Ok(schedule) => schedule,
        Err(error) => {
            return Ok(status(
                binding,
                Some(expression.into()),
                Some(hash),
                DaemonScheduleReadiness::InvalidSchedule,
                Some(error),
                None,
                store,
            ));
        }
    };
    let next = schedule.after(&minute_floor(now)).next().map(utc_string);
    if !binding.schedule_enabled {
        return Ok(status(
            binding,
            Some(expression.into()),
            Some(hash),
            DaemonScheduleReadiness::Disabled,
            Some("scheduled execution is disabled; manual Run now remains available".into()),
            next,
            store,
        ));
    }
    if let Err(error) = super::revalidate_binding_context(binding) {
        return Ok(status(
            binding,
            Some(expression.into()),
            Some(hash),
            DaemonScheduleReadiness::InvalidContext,
            Some(error),
            next,
            store,
        ));
    }
    let (readiness, reason) = match crate::managed_agents::daemon_binding_readiness(app, binding) {
        Ok(()) => (DaemonScheduleReadiness::Ready, None),
        Err(value) => value,
    };
    Ok(status(
        binding,
        Some(expression.into()),
        Some(hash),
        readiness,
        reason,
        next,
        store,
    ))
}

async fn process_scheduler_tick(app: &AppHandle, now: DateTime<Utc>) -> Result<(), String> {
    let now = minute_floor(now);
    let bindings = BindingStore::load(app)?.bindings;
    for binding in bindings {
        process_binding_tick(app, &binding, now).await?;
    }
    Ok(())
}

async fn process_binding_tick(
    app: &AppHandle,
    binding: &DaemonBinding,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.shutdown_started.load(Ordering::Acquire) {
        return Ok(());
    }
    let package = match load_managed_package(app, &binding.daemon_id) {
        Ok(package) => package,
        Err(error) => {
            update_cursor_decision(
                app,
                binding,
                "missing-package",
                now,
                decision(
                    DaemonSchedulerDecisionKind::SkippedUnready,
                    now,
                    None,
                    Some(error),
                ),
            )?;
            return Ok(());
        }
    };
    let Some(expression) = package.policy.schedule.as_deref() else {
        update_cursor_decision(
            app,
            binding,
            "watch-only",
            now,
            decision(DaemonSchedulerDecisionKind::WatchOnly, now, None, None),
        )?;
        return Ok(());
    };
    let hash = schedule_hash(expression);
    let schedule = match parse_schedule(expression) {
        Ok(schedule) => schedule,
        Err(error) => {
            update_cursor_decision(
                app,
                binding,
                &hash,
                now,
                decision(
                    DaemonSchedulerDecisionKind::SkippedUnready,
                    now,
                    None,
                    Some(error),
                ),
            )?;
            return Ok(());
        }
    };
    let mut scheduler_store = SchedulerStore::load(app)?;
    let previous = scheduler_store
        .cursors
        .get(&binding.id)
        .filter(|cursor| cursor.schedule_hash == hash)
        .and_then(|cursor| parse_utc(&cursor.observed_through_utc).ok())
        .unwrap_or(now - Duration::minutes(1));
    let due = due_occurrences(&schedule, previous, now);

    if !binding.schedule_enabled {
        scheduler_store.cursors.insert(
            binding.id.clone(),
            SchedulerCursor {
                schedule_hash: hash,
                observed_through_utc: utc_string(now),
                last_decision: Some(decision(
                    DaemonSchedulerDecisionKind::Disabled,
                    now,
                    None,
                    Some("scheduled execution is disabled".into()),
                )),
            },
        );
        return scheduler_store.save(app);
    }

    if let Some(missed) = due.latest_missed {
        let claim = occurrence_claim(&binding.id, &hash, missed);
        if scheduler_store.claims.insert(claim.clone()) {
            trim_claims(&mut scheduler_store.claims);
            scheduler_store.save(app)?;
            let run_id = occurrence_run_id(&claim).to_string();
            record_scheduler_receipt(
                app,
                &run_id,
                binding,
                Some(&package.package_hash),
                &utc_string(missed),
                DaemonRunStatus::Missed,
                "scheduled occurrence elapsed while Buzz was not evaluating the current UTC minute; backlog was not replayed",
            )?;
        }
    }

    let mut last_decision = due.latest_missed.map(|missed| {
        decision(
            DaemonSchedulerDecisionKind::Missed,
            now,
            Some(missed),
            Some("recorded only the latest missed occurrence; backlog was not replayed".into()),
        )
    });
    if let Some(current) = due.current {
        let claim = occurrence_claim(&binding.id, &hash, current);
        if scheduler_store.claims.insert(claim.clone()) {
            trim_claims(&mut scheduler_store.claims);
            scheduler_store.save(app)?;
            let run_id = occurrence_run_id(&claim).to_string();
            let scheduled_for = utc_string(current);
            let readiness = super::revalidate_binding_context(binding)
                .map(|_| ())
                .map_err(|error| (DaemonScheduleReadiness::InvalidContext, Some(error)))
                .and_then(|_| crate::managed_agents::daemon_binding_readiness(app, binding));
            match readiness {
                Err((_, reason)) => {
                    record_scheduler_receipt(
                        app,
                        &run_id,
                        binding,
                        Some(&package.package_hash),
                        &scheduled_for,
                        DaemonRunStatus::SkippedUnready,
                        reason.as_deref().unwrap_or("daemon binding is not ready"),
                    )?;
                    last_decision = Some(decision(
                        DaemonSchedulerDecisionKind::SkippedUnready,
                        now,
                        Some(current),
                        reason,
                    ));
                }
                Ok(()) if binding_has_active_run(app, &binding.id)? => {
                    record_scheduler_receipt(
                        app,
                        &run_id,
                        binding,
                        Some(&package.package_hash),
                        &scheduled_for,
                        DaemonRunStatus::SkippedOverlap,
                        "scheduled occurrence skipped because the binding already had an active run",
                    )?;
                    last_decision = Some(decision(
                        DaemonSchedulerDecisionKind::SkippedOverlap,
                        now,
                        Some(current),
                        Some("binding already had an active run".into()),
                    ));
                }
                Ok(()) => {
                    last_decision = Some(decision(
                        DaemonSchedulerDecisionKind::Executed,
                        now,
                        Some(current),
                        None,
                    ));
                    let run_app = app.clone();
                    let binding_id = binding.id.clone();
                    let receipt_run_id = run_id.clone();
                    let package_hash = package.package_hash.clone();
                    let receipt_binding = binding.clone();
                    tauri::async_runtime::spawn(async move {
                        let result = run_managed_daemon(
                            RunManagedDaemonRequest {
                                run_id,
                                binding_id: binding_id.clone(),
                                wake_instruction: format!(
                                    "Run the scheduled daemon occurrence for {scheduled_for}. This timestamp is UTC."
                                ),
                                trigger: DaemonRunTrigger::Schedule,
                                scheduled_for_utc: Some(scheduled_for.clone()),
                            },
                            run_app.clone(),
                        )
                        .await;
                        if let Err(error) = result {
                            let status = if error.contains("active run") {
                                DaemonRunStatus::SkippedOverlap
                            } else if error.contains("current UTC minute") {
                                DaemonRunStatus::Missed
                            } else {
                                DaemonRunStatus::SkippedUnready
                            };
                            let _ = record_scheduler_receipt(
                                &run_app,
                                &receipt_run_id,
                                &receipt_binding,
                                Some(&package_hash),
                                &scheduled_for,
                                status,
                                &error,
                            );
                        }
                    });
                }
            }
        }
    }
    scheduler_store.cursors.insert(
        binding.id.clone(),
        SchedulerCursor {
            schedule_hash: hash,
            observed_through_utc: utc_string(now),
            last_decision: last_decision.or_else(|| {
                Some(decision(
                    DaemonSchedulerDecisionKind::Ready,
                    now,
                    None,
                    None,
                ))
            }),
        },
    );
    trim_claims(&mut scheduler_store.claims);
    scheduler_store.save(app)
}

fn status(
    binding: &DaemonBinding,
    schedule: Option<String>,
    schedule_hash: Option<String>,
    readiness: DaemonScheduleReadiness,
    readiness_reason: Option<String>,
    next_occurrence_utc: Option<String>,
    store: &SchedulerStore,
) -> DaemonScheduleStatus {
    DaemonScheduleStatus {
        binding_id: binding.id.clone(),
        schedule,
        schedule_hash,
        readiness,
        readiness_reason,
        next_occurrence_utc,
        last_decision: store
            .cursors
            .get(&binding.id)
            .and_then(|cursor| cursor.last_decision.clone()),
    }
}

fn update_cursor_decision(
    app: &AppHandle,
    binding: &DaemonBinding,
    hash: &str,
    now: DateTime<Utc>,
    last_decision: DaemonSchedulerDecision,
) -> Result<(), String> {
    let mut store = SchedulerStore::load(app)?;
    store.cursors.insert(
        binding.id.clone(),
        SchedulerCursor {
            schedule_hash: hash.into(),
            observed_through_utc: utc_string(now),
            last_decision: Some(last_decision),
        },
    );
    store.save(app)
}

impl SchedulerStore {
    fn load(app: &AppHandle) -> Result<Self, String> {
        let path = scheduler_store_path(app)?;
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("failed to parse daemon scheduler store: {error}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn save(&self, app: &AppHandle) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        crate::managed_agents::atomic_write_json_restricted(&scheduler_store_path(app)?, &bytes)
    }
}

fn scheduler_store_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(crate::managed_agents::managed_agents_base_dir(app)?.join("daemon-scheduler.json"))
}

fn parse_schedule(expression: &str) -> Result<Schedule, String> {
    Schedule::from_str(&format!("0 {expression}"))
        .map_err(|error| format!("invalid five-field UTC cron schedule: {error}"))
}

fn due_occurrences(
    schedule: &Schedule,
    observed_through: DateTime<Utc>,
    now: DateTime<Utc>,
) -> DueOccurrences {
    let mut due = DueOccurrences::default();
    for occurrence in schedule.after(&observed_through) {
        if occurrence > now {
            break;
        }
        if occurrence == now {
            due.current = Some(occurrence);
        } else {
            due.latest_missed = Some(occurrence);
        }
    }
    due
}

fn minute_floor(value: DateTime<Utc>) -> DateTime<Utc> {
    value
        .with_second(0)
        .and_then(|value| value.with_nanosecond(0))
        .unwrap_or(value)
}

fn occurrence_claim(binding_id: &str, schedule_hash: &str, occurrence: DateTime<Utc>) -> String {
    format!("{binding_id}:{schedule_hash}:{}", utc_string(occurrence))
}

fn occurrence_run_id(claim: &str) -> Uuid {
    Uuid::new_v5(&SCHEDULER_NAMESPACE, claim.as_bytes())
}

fn schedule_hash(expression: &str) -> String {
    hex::encode(Sha256::digest(expression.as_bytes()))
}

fn decision(
    kind: DaemonSchedulerDecisionKind,
    decided_at: DateTime<Utc>,
    scheduled_for: Option<DateTime<Utc>>,
    diagnostic: Option<String>,
) -> DaemonSchedulerDecision {
    DaemonSchedulerDecision {
        kind,
        decided_at_utc: utc_string(decided_at),
        scheduled_for_utc: scheduled_for.map(utc_string),
        diagnostic,
    }
}

fn utc_string(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn parse_utc(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| error.to_string())
}

fn trim_claims(claims: &mut BTreeSet<String>) {
    while claims.len() > MAX_CLAIMS {
        let Some(first) = claims.first().cloned() else {
            break;
        };
        claims.remove(&first);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(value: &str) -> DateTime<Utc> {
        parse_utc(value).unwrap()
    }

    #[test]
    fn five_field_schedule_reports_next_utc_occurrence() {
        let schedule = parse_schedule("*/15 * * * *").unwrap();
        assert_eq!(
            schedule.after(&at("2026-07-24T21:01:00Z")).next(),
            Some(at("2026-07-24T21:15:00Z"))
        );
    }

    #[test]
    fn current_minute_executes_without_replaying_backlog() {
        let schedule = parse_schedule("* * * * *").unwrap();
        let due = due_occurrences(
            &schedule,
            at("2026-07-24T20:57:00Z"),
            at("2026-07-24T21:00:00Z"),
        );
        assert_eq!(due.latest_missed, Some(at("2026-07-24T20:59:00Z")));
        assert_eq!(due.current, Some(at("2026-07-24T21:00:00Z")));
    }

    #[test]
    fn downtime_records_only_latest_missed_occurrence() {
        let schedule = parse_schedule("* * * * *").unwrap();
        let due = due_occurrences(
            &schedule,
            at("2026-07-24T20:00:00Z"),
            at("2026-07-24T20:10:30Z"),
        );
        assert_eq!(due.latest_missed, Some(at("2026-07-24T20:10:00Z")));
        assert_eq!(due.current, None);
    }

    #[test]
    fn occurrence_uuid_is_stable_for_restart_dedupe() {
        let occurrence = at("2026-07-24T21:00:00Z");
        let claim = occurrence_claim("binding", "hash", occurrence);
        assert_eq!(occurrence_run_id(&claim), occurrence_run_id(&claim));
    }
}
