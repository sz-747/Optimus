//! Pure-ish runtime helpers shared by the CLI binary and its tests.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::cli::parser::CliInvocation;
use crate::ipc::naming;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct CliReport {
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
    pub exit_code: i32,
}

pub fn resolve_pipe(
    invocation: &CliInvocation,
    get_env: &dyn Fn(&str) -> Option<String>,
    can_connect: &dyn Fn(&str) -> bool,
) -> String {
    if let Some(explicit) = invocation
        .explicit_socket
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return explicit.to_string();
    }

    let from_env = naming::resolve_from_environment(get_env);
    let candidates = naming::build_candidate_pipes(
        (!from_env.is_empty()).then_some(from_env.as_str()),
        invocation.variant.as_deref(),
        None,
    );
    naming::resolve_with_probe(&candidates, can_connect)
}

pub fn write_local_files(
    invocation: &CliInvocation,
    get_env: &dyn Fn(&str) -> Option<String>,
) -> io::Result<Vec<PathBuf>> {
    let Some(writes) = invocation
        .file_writes
        .as_ref()
        .filter(|writes| !writes.is_empty())
    else {
        return Ok(Vec::new());
    };

    let directory = invocation
        .install_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| default_hook_directory(get_env));
    fs::create_dir_all(&directory)?;

    let mut paths = Vec::with_capacity(writes.len());
    for write in writes {
        let relative = Path::new(&write.relative_path);
        if relative.components().count() != 1
            || !matches!(relative.components().next(), Some(Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "hook output must be a file name",
            ));
        }

        let path = directory.join(relative);
        fs::write(&path, &write.content)?;
        paths.push(path);
    }
    Ok(paths)
}

pub fn report_responses(responses: &[Option<String>]) -> CliReport {
    let mut report = CliReport::default();

    for response in responses {
        let Some(response) = response else {
            report
                .stderr
                .push("optimus: connection closed before a response arrived".to_string());
            report.exit_code = 1;
            continue;
        };

        if report_v2(response, &mut report) {
            continue;
        }

        report.stdout.push(response.clone());
        if response.starts_with("ERROR") {
            report.exit_code = 1;
        }
    }

    report
}

fn default_hook_directory(get_env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let home = get_env("USERPROFILE")
        .or_else(|| get_env("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".optimusterm").join("hooks")
}

fn report_v2(response: &str, report: &mut CliReport) -> bool {
    if !response.starts_with('{') {
        return false;
    }
    let Ok(root) = serde_json::from_str::<Value>(response) else {
        return false;
    };
    let Some(root) = root.as_object() else {
        return false;
    };

    if root.get("ok").and_then(Value::as_bool) == Some(true) {
        match root.get("result").filter(|result| !result.is_null()) {
            Some(result) => report.stdout.push(result.to_string()),
            None => report.stdout.push("ok".to_string()),
        }
    } else {
        let message = root
            .get("error")
            .and_then(Value::as_object)
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        report.stderr.push(format!("optimus: {message}"));
        report.exit_code = 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::parser::{CliInvocation, LocalFileWrite};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn explicit_socket_wins_without_probing() {
        let invocation = CliInvocation {
            explicit_socket: Some(r"\\.\pipe\chosen".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_pipe(&invocation, &no_env, &|_| panic!("must not probe")),
            r"\\.\pipe\chosen"
        );
    }

    #[test]
    fn environment_socket_is_probed_before_variant_fallbacks() {
        let invocation = CliInvocation {
            variant: Some("nightly".into()),
            ..Default::default()
        };
        let get_env =
            |key: &str| (key == naming::SOCKET_PATH_ENV).then(|| r"\\.\pipe\from-env".to_string());
        assert_eq!(
            resolve_pipe(&invocation, &get_env, &|pipe| pipe.ends_with("from-env")),
            r"\\.\pipe\from-env"
        );
    }

    #[test]
    fn responses_preserve_v2_and_v1_exit_semantics() {
        let report = report_responses(&[
            Some(r#"{"id":"1","ok":true,"result":{"ready":true},"error":null}"#.into()),
            Some("ERROR: denied".into()),
            None,
        ]);
        assert_eq!(report.stdout, vec![r#"{"ready":true}"#, "ERROR: denied"]);
        assert_eq!(
            report.stderr,
            vec!["optimus: connection closed before a response arrived"]
        );
        assert_eq!(report.exit_code, 1);
    }

    #[test]
    fn v2_error_reports_the_server_message() {
        let report = report_responses(&[Some(
            r#"{"id":"1","ok":false,"result":null,"error":{"code":"denied","message":"not allowed"}}"#
                .into(),
        )]);
        assert!(report.stdout.is_empty());
        assert_eq!(report.stderr, vec!["optimus: not allowed"]);
        assert_eq!(report.exit_code, 1);
    }

    #[test]
    fn hook_files_are_written_to_the_requested_directory() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("optimus-cli-runtime-{nonce}"));
        let invocation = CliInvocation {
            install_dir: Some(directory.to_string_lossy().into_owned()),
            file_writes: Some(vec![LocalFileWrite {
                relative_path: "hook.ps1".into(),
                content: "Write-Host ok".into(),
            }]),
            ..Default::default()
        };

        let paths = write_local_files(&invocation, &no_env).expect("write hook file");
        assert_eq!(paths, vec![directory.join("hook.ps1")]);
        assert_eq!(fs::read_to_string(&paths[0]).unwrap(), "Write-Host ok");
        fs::remove_dir_all(directory).ok();
    }
}
