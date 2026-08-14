use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use sha2::{Digest, Sha256};

use super::model::{RelayConfig, RelayProviderConfig, RelayWorkspaceConfig};
use super::RelayError;
use crate::child_environment::MODEL_API_CREDENTIALS;
use crate::orchestration::store::OrchestrationStore;

const RELAY_URL_ENV: &str = "OPTIMUS_RELAY_URL";
const DESKTOP_TOKEN_ENV: &str = "OPTIMUS_DESKTOP_TOKEN";
const DEVICE_ID_ENV: &str = "OPTIMUS_DEVICE_ID";
const WORKSPACES_ENV: &str = "OPTIMUS_RELAY_WORKSPACES";
const PROVIDERS_ENV: &str = "OPTIMUS_PROVIDER_PROFILES";

pub fn load_relay_config() -> Result<Option<RelayConfig>, RelayError> {
    let Some(base_url) = env_nonempty(RELAY_URL_ENV) else {
        return Ok(None);
    };
    let desktop_token = env_nonempty(DESKTOP_TOKEN_ENV)
        .ok_or_else(|| RelayError::Config(format!("{DESKTOP_TOKEN_ENV} is required")))?;
    if desktop_token.len() < 24 {
        return Err(RelayError::Config(format!(
            "{DESKTOP_TOKEN_ENV} must contain at least 24 characters"
        )));
    }
    let base_url = validate_url(&base_url)?;
    let device_id = env_nonempty(DEVICE_ID_ENV).unwrap_or_else(|| "primary".to_string());
    validate_id(&device_id, DEVICE_ID_ENV)?;
    let workspaces: Vec<RelayWorkspaceConfig> = parse_json_array(WORKSPACES_ENV)?;
    let providers: Vec<RelayProviderConfig> = parse_json_array(PROVIDERS_ENV)?;
    if workspaces.is_empty() || providers.is_empty() {
        return Err(RelayError::Config(format!(
            "{WORKSPACES_ENV} and {PROVIDERS_ENV} must each register at least one item"
        )));
    }
    validate_workspaces(&workspaces)?;
    validate_providers(&providers)?;
    Ok(Some(RelayConfig {
        base_url,
        desktop_token,
        device_id,
        instance_id: generated_uuid("instance"),
        consumer_id: generated_uuid("consumer"),
        workspaces,
        providers,
    }))
}

pub fn configure_registry(
    config: &RelayConfig,
    store: &Arc<OrchestrationStore>,
    timestamp: i64,
) -> Result<(), RelayError> {
    store.disable_relay_registry()?;
    for entry in &config.workspaces {
        let root = Path::new(&entry.repo_root)
            .canonicalize()
            .map_err(|error| RelayError::Config(format!("workspace {}: {error}", entry.id)))?;
        let name = root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("workspace");
        let workspace = store.get_or_create_workspace(name, &root, timestamp)?;
        store.register_relay_workspace(&entry.id, workspace.id, &entry.label)?;
    }
    for entry in &config.providers {
        store.register_relay_provider(
            &entry.id,
            &entry.label,
            &entry.command,
            "prompt_arg",
            &entry.auth_mode,
            entry.credential_env.as_deref(),
        )?;
    }
    store.recover_relay_commands(timestamp)?;
    Ok(())
}

fn validate_url(value: &str) -> Result<String, RelayError> {
    let mut url = reqwest::Url::parse(value)
        .map_err(|error| RelayError::Config(format!("{RELAY_URL_ENV} is invalid: {error}")))?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
    if url.scheme() != "https" && !(local && url.scheme() == "http") {
        return Err(RelayError::Config(
            "relay URL must use HTTPS (HTTP is allowed only for localhost)".to_string(),
        ));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RelayError::Config(
            "relay URL cannot contain credentials, a query, or a fragment".to_string(),
        ));
    }
    let path = url.path().trim_end_matches('/').to_string();
    url.set_path(&path);
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn validate_workspaces(entries: &[RelayWorkspaceConfig]) -> Result<(), RelayError> {
    let mut ids = HashSet::new();
    for entry in entries {
        validate_id(&entry.id, "workspace id")?;
        validate_label(&entry.label, "workspace label")?;
        if !ids.insert(&entry.id) {
            return Err(RelayError::Config("duplicate workspace id".to_string()));
        }
        if entry.repo_root.trim().is_empty() {
            return Err(RelayError::Config(
                "workspace path cannot be empty".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_providers(entries: &[RelayProviderConfig]) -> Result<(), RelayError> {
    let mut ids = HashSet::new();
    for entry in entries {
        validate_id(&entry.id, "provider id")?;
        validate_label(&entry.label, "provider label")?;
        if !ids.insert(&entry.id) {
            return Err(RelayError::Config("duplicate provider id".to_string()));
        }
        match entry.auth_mode.as_str() {
            "subscription" if entry.credential_env.is_some() => {
                return Err(RelayError::Config(
                    "subscription profiles cannot declare credentialEnv".to_string(),
                ))
            }
            "subscription" => {}
            "api"
                if entry
                    .credential_env
                    .as_deref()
                    .is_some_and(|name| MODEL_API_CREDENTIALS.contains(&name)) => {}
            "api" => {
                return Err(RelayError::Config(format!(
                    "API profiles require credentialEnv from: {}",
                    MODEL_API_CREDENTIALS.join(", ")
                )))
            }
            _ => {
                return Err(RelayError::Config(
                    "provider authMode must be subscription or api".to_string(),
                ))
            }
        }
        validate_command_prefix(&entry.command)?;
    }
    Ok(())
}

fn validate_command_prefix(command: &str) -> Result<(), RelayError> {
    let command = command.trim();
    if command.is_empty() || command.len() > 512 || command.contains(['\r', '\n', '\0']) {
        return Err(RelayError::Config(
            "provider command must be a single-line command prefix".to_string(),
        ));
    }
    let executable = command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches('"');
    let executable = Path::new(executable)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        executable.as_str(),
        "cmd" | "powershell" | "pwsh" | "wsl" | "bash" | "sh"
    ) {
        return Err(RelayError::Config(
            "provider command must launch the model CLI directly, not a shell".to_string(),
        ));
    }
    Ok(())
}

fn validate_id(value: &str, label: &str) -> Result<(), RelayError> {
    let pattern = Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,63}$").expect("static regex");
    if !pattern.is_match(value) {
        return Err(RelayError::Config(format!("{label} has an invalid format")));
    }
    Ok(())
}

fn validate_label(value: &str, label: &str) -> Result<(), RelayError> {
    if value.trim().is_empty() || value.len() > 80 || value.contains(['\r', '\n', '\0']) {
        return Err(RelayError::Config(format!("{label} is invalid")));
    }
    Ok(())
}

fn parse_json_array<T: serde::de::DeserializeOwned>(name: &str) -> Result<Vec<T>, RelayError> {
    let raw =
        env_nonempty(name).ok_or_else(|| RelayError::Config(format!("{name} is required")))?;
    serde_json::from_str(&raw)
        .map_err(|error| RelayError::Config(format!("{name} is invalid JSON: {error}")))
}

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn generated_uuid(domain: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(timestamp.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(format!("{:?}", std::thread::current().id()).as_bytes());
    let mut bytes: [u8; 16] = hasher.finalize()[..16].try_into().expect("16-byte slice");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes(bytes[0..4].try_into().expect("uuid word")),
        u16::from_be_bytes(bytes[4..6].try_into().expect("uuid word")),
        u16::from_be_bytes(bytes[6..8].try_into().expect("uuid word")),
        u16::from_be_bytes(bytes[8..10].try_into().expect("uuid word")),
        u64::from_be_bytes([
            0, 0, bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_shell_wrapped_provider_commands() {
        assert!(validate_command_prefix("cmd /c codex").is_err());
        assert!(validate_command_prefix("powershell codex").is_err());
        assert!(validate_command_prefix("codex exec").is_ok());
    }

    #[test]
    fn generated_ids_match_the_relay_uuid_contract() {
        let id = generated_uuid("test");
        assert_eq!(36, id.len());
        assert_eq!('-', id.as_bytes()[8] as char);
        assert_eq!('-', id.as_bytes()[13] as char);
    }

    #[test]
    fn provider_auth_modes_have_distinct_credential_contracts() {
        let subscription = RelayProviderConfig {
            id: "subscription".to_string(),
            label: "Subscription".to_string(),
            command: "codex".to_string(),
            auth_mode: "subscription".to_string(),
            credential_env: None,
        };
        let api = RelayProviderConfig {
            id: "api".to_string(),
            label: "API".to_string(),
            command: "codex".to_string(),
            auth_mode: "api".to_string(),
            credential_env: Some("OPENAI_API_KEY".to_string()),
        };
        assert!(validate_providers(&[subscription]).is_ok());
        assert!(validate_providers(&[api]).is_ok());

        let invalid = RelayProviderConfig {
            id: "invalid".to_string(),
            label: "Invalid".to_string(),
            command: "codex".to_string(),
            auth_mode: "api".to_string(),
            credential_env: Some("UNREVIEWED_SECRET".to_string()),
        };
        assert!(validate_providers(&[invalid]).is_err());
    }
}
