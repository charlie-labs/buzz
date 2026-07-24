use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDaemonPackageRequest {
    pub daemon_id: String,
    pub daemon_md: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonActivationMode {
    WatchOnly,
    ScheduleOnly,
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonPolicy {
    pub id: String,
    pub purpose: String,
    pub watch: Vec<String>,
    pub routines: Vec<String>,
    pub deny: Vec<String>,
    pub schedule: Option<String>,
    pub body: String,
    pub activation_mode: DaemonActivationMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonPackageSummary {
    pub id: String,
    pub purpose: String,
    pub activation_mode: DaemonActivationMode,
    pub package_hash: String,
    pub watch_count: usize,
    pub routine_count: usize,
    pub has_scripts: bool,
    pub has_references: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonPackageDetail {
    #[serde(flatten)]
    pub summary: DaemonPackageSummary,
    pub daemon_md: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDaemonPackageRequest {
    pub daemon_md: String,
    #[serde(default)]
    pub files: Vec<DaemonPackageFileInput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonPackageFileInput {
    pub path: String,
    pub bytes: Vec<u8>,
    #[serde(default)]
    pub executable: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportDaemonPackageRequest {
    pub source_path: String,
    #[serde(default)]
    pub replace: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonBinding {
    pub id: String,
    pub daemon_id: String,
    pub agent_pubkey: String,
    pub relay_url: String,
    pub channel_id: String,
    pub context_directory: Option<String>,
    pub context_configured: bool,
    pub schedule_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonBindingSummary {
    pub id: String,
    pub daemon_id: String,
    pub agent_pubkey: String,
    pub relay_url: String,
    pub channel_id: String,
    pub context_configured: bool,
    pub schedule_enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<&DaemonBinding> for DaemonBindingSummary {
    fn from(value: &DaemonBinding) -> Self {
        Self {
            id: value.id.clone(),
            daemon_id: value.daemon_id.clone(),
            agent_pubkey: value.agent_pubkey.clone(),
            relay_url: value.relay_url.clone(),
            channel_id: value.channel_id.clone(),
            context_configured: value.context_configured,
            schedule_enabled: value.schedule_enabled,
            created_at: value.created_at.clone(),
            updated_at: value.updated_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDaemonBindingRequest {
    pub daemon_id: String,
    pub agent_pubkey: String,
    pub relay_url: String,
    pub channel_id: String,
    pub context_directory: Option<String>,
    #[serde(default = "default_true")]
    pub schedule_enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDaemonBindingRequest {
    pub id: String,
    pub daemon_id: Option<String>,
    pub agent_pubkey: Option<String>,
    pub relay_url: Option<String>,
    pub channel_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_nullable")]
    pub context_directory: Option<Option<String>>,
    pub schedule_enabled: Option<bool>,
}

fn deserialize_optional_nullable<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonRunTrigger {
    #[default]
    Manual,
    Watch,
    Schedule,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunManagedDaemonRequest {
    pub run_id: String,
    pub binding_id: String,
    pub wake_instruction: String,
    #[serde(default)]
    pub trigger: DaemonRunTrigger,
    #[serde(default)]
    pub scheduled_for_utc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonRunLifecycle {
    Reserved,
    Running,
    Publishing,
    Terminal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonRunStatus {
    Succeeded,
    NoOp,
    Failed,
    Cancelled,
    Interrupted,
    Missed,
    SkippedOverlap,
    SkippedUnready,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonScheduleReadiness {
    Ready,
    Disabled,
    WatchOnly,
    MissingPackage,
    InvalidSchedule,
    MissingAgent,
    RelayMismatch,
    ChannelUnavailable,
    UnsupportedRuntime,
    AgentNotReady,
    InvalidContext,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonSchedulerDecisionKind {
    Ready,
    Executed,
    Missed,
    SkippedOverlap,
    SkippedUnready,
    Disabled,
    WatchOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonSchedulerDecision {
    pub kind: DaemonSchedulerDecisionKind,
    pub decided_at_utc: String,
    pub scheduled_for_utc: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonScheduleStatus {
    pub binding_id: String,
    pub schedule: Option<String>,
    pub schedule_hash: Option<String>,
    pub readiness: DaemonScheduleReadiness,
    pub readiness_reason: Option<String>,
    pub next_occurrence_utc: Option<String>,
    pub last_decision: Option<DaemonSchedulerDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonBindingSnapshot {
    pub binding_id: String,
    pub daemon_id: String,
    pub agent_pubkey: String,
    pub relay_url: String,
    pub channel_id: String,
    pub context_configured: bool,
    pub schedule_enabled: bool,
}

impl From<&DaemonBinding> for DaemonBindingSnapshot {
    fn from(value: &DaemonBinding) -> Self {
        Self {
            binding_id: value.id.clone(),
            daemon_id: value.daemon_id.clone(),
            agent_pubkey: value.agent_pubkey.clone(),
            relay_url: value.relay_url.clone(),
            channel_id: value.channel_id.clone(),
            context_configured: value.context_configured,
            schedule_enabled: value.schedule_enabled,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DaemonRunRecord {
    pub run_id: String,
    pub binding_id: String,
    pub daemon_id: String,
    pub package_hash: Option<String>,
    pub policy_hash: Option<String>,
    pub fingerprint: String,
    pub wake_hash: String,
    pub binding: DaemonBindingSnapshot,
    pub trigger: DaemonRunTrigger,
    #[serde(default)]
    pub scheduled_for_utc: Option<String>,
    pub lifecycle: DaemonRunLifecycle,
    pub status: Option<DaemonRunStatus>,
    pub reserved_at: String,
    pub started_at: Option<String>,
    pub publishing_at: Option<String>,
    pub completed_at: Option<String>,
    pub acp_session_id: Option<String>,
    pub output_event_id: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LegacyDaemonRunRecord {
    pub run_id: String,
    pub daemon_id: String,
    pub status: DaemonRunStatus,
    pub started_at: String,
    pub completed_at: String,
    pub acp_session_id: Option<String>,
    pub output_event_id: Option<String>,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "recordType", rename_all = "snake_case")]
pub enum DaemonHistoryEntry {
    Managed(Box<DaemonRunRecord>),
    Legacy(LegacyDaemonRunRecord),
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedDaemonPackage {
    pub directory: std::path::PathBuf,
    pub daemon_md: String,
    pub policy: DaemonPolicy,
    pub package_hash: String,
    pub has_scripts: bool,
    pub has_references: bool,
}
