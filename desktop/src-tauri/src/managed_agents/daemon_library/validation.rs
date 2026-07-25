use std::str::FromStr;

use cron::Schedule;
use serde::Deserialize;

use super::{DaemonActivationMode, DaemonPolicy};

pub const MAX_DAEMON_MD_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DaemonFrontmatter {
    id: String,
    purpose: String,
    #[serde(default)]
    watch: Vec<String>,
    routines: Vec<String>,
    #[serde(default)]
    deny: Vec<String>,
    schedule: Option<String>,
}

pub(crate) fn validate_daemon_id(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || value.starts_with('-')
        || value.ends_with('-')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err("daemon ID must be 1-64 lowercase letters, digits, or interior hyphens".into());
    }
    Ok(())
}

pub(crate) fn parse_daemon_policy(
    contents: &str,
    expected_id: Option<&str>,
) -> Result<DaemonPolicy, String> {
    if contents.len() > MAX_DAEMON_MD_BYTES {
        return Err("DAEMON.md exceeds 256 KiB".into());
    }
    let normalized = contents.replace("\r\n", "\n").replace('\r', "\n");
    let rest = normalized
        .strip_prefix("---\n")
        .ok_or("DAEMON.md must start with YAML frontmatter")?;
    let (frontmatter, body) = rest
        .split_once("\n---\n")
        .ok_or("DAEMON.md frontmatter is missing its closing delimiter")?;
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(frontmatter).map_err(|error| yaml_frontmatter_error(&error))?;
    if !matches!(yaml, serde_yaml::Value::Mapping(_)) {
        return Err("DAEMON.md frontmatter must be a YAML object/map".into());
    }
    let metadata: DaemonFrontmatter = serde_yaml::from_value(yaml)
        .map_err(|error| format!("invalid DAEMON.md frontmatter: {error}"))?;
    validate_daemon_id(&metadata.id)?;
    if expected_id.is_some_and(|expected| expected != metadata.id) {
        return Err("DAEMON.md ID must match its directory".into());
    }
    if metadata.purpose.trim().is_empty() {
        return Err("DAEMON.md purpose must be non-empty".into());
    }
    validate_nonempty_list("routines", &metadata.routines, true)?;
    validate_nonempty_list("watch", &metadata.watch, false)?;
    validate_nonempty_list("deny", &metadata.deny, false)?;
    if body.trim().is_empty() {
        return Err("DAEMON.md Markdown body must be non-empty".into());
    }

    let schedule = metadata
        .schedule
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(expression) = &schedule {
        validate_cron(expression)?;
    }
    if metadata.watch.is_empty() && schedule.is_none() {
        return Err("DAEMON.md requires at least one wake source: watch or schedule".into());
    }
    let activation_mode = match (metadata.watch.is_empty(), schedule.is_none()) {
        (false, true) => DaemonActivationMode::WatchOnly,
        (true, false) => DaemonActivationMode::ScheduleOnly,
        (false, false) => DaemonActivationMode::Hybrid,
        (true, true) => unreachable!("wake source checked above"),
    };
    Ok(DaemonPolicy {
        id: metadata.id,
        purpose: metadata.purpose.trim().to_string(),
        watch: trim_list(metadata.watch),
        routines: trim_list(metadata.routines),
        deny: trim_list(metadata.deny),
        schedule,
        body: body.trim().to_string(),
        activation_mode,
    })
}

fn yaml_frontmatter_error(error: &serde_yaml::Error) -> String {
    format!(
        "invalid DAEMON.md frontmatter: {error}. Quote values that begin with YAML-reserved syntax such as `*`; for example: `schedule: \"*/2 * * * *\"`"
    )
}

fn validate_nonempty_list(name: &str, values: &[String], required: bool) -> Result<(), String> {
    if required && values.is_empty() {
        return Err(format!("DAEMON.md {name} must contain at least one entry"));
    }
    if values.iter().any(|value| value.trim().is_empty()) {
        return Err(format!("DAEMON.md {name} entries must be non-empty"));
    }
    Ok(())
}

fn trim_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .collect()
}

fn validate_cron(expression: &str) -> Result<(), String> {
    if expression.split_whitespace().count() != 5 {
        return Err("daemon schedule must contain exactly five cron fields (UTC)".into());
    }
    let seconds_prefixed = format!("0 {expression}");
    Schedule::from_str(&seconds_prefixed)
        .map_err(|error| format!("invalid five-field UTC cron schedule: {error}"))?;
    Ok(())
}
