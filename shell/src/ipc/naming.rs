//! Port of core/Ipc/PipeName.cs + SocketDiscovery.cs — the Win32 pipe naming scheme and the
//! client-side discovery order. The pipe names (`optimus-{variant}[-slug]`) are the contract
//! external agents connect to; the strings and fallback order must not drift.

/// Env var carrying an explicit full pipe path (`\\.\pipe\...`), highest priority.
pub const SOCKET_PATH_ENV: &str = "OPTIMUS_SOCKET_PATH";
/// Legacy env var, consulted only if [`SOCKET_PATH_ENV`] is unset.
pub const SOCKET_ENV: &str = "OPTIMUS_SOCKET";

const LOCAL_PIPE_PREFIX: &str = r"\\.\pipe\";

/// Client-side variant probe order: prefer stable, then pre-release rings.
const VARIANT_FALLBACK_ORDER: [&str; 4] = ["stable", "nightly", "staging", "dev"];

/// Build the full Win32 pipe path `\\.\pipe\optimus-{variant}[-slug]`.
pub fn build_pipe_name(variant: &str, slug: Option<&str>) -> String {
    let normalized = normalize_variant(variant);
    let suffix = match slug {
        Some(s) if !s.trim().is_empty() => format!("-{s}"),
        _ => String::new(),
    };
    format!(r"\\.\pipe\optimus-{normalized}{suffix}")
}

/// Strip the `\\.\pipe\` prefix so the bare name can be handed to APIs that prepend it themselves.
pub fn to_local_name(pipe_path: &str) -> String {
    if pipe_path.len() >= LOCAL_PIPE_PREFIX.len()
        && pipe_path[..LOCAL_PIPE_PREFIX.len()].eq_ignore_ascii_case(LOCAL_PIPE_PREFIX)
    {
        pipe_path[LOCAL_PIPE_PREFIX.len()..].to_string()
    } else {
        pipe_path.to_string()
    }
}

/// Resolve an explicit pipe path from the environment: `OPTIMUS_SOCKET_PATH` wins over
/// `OPTIMUS_SOCKET`; empty string when neither is set.
pub fn resolve_from_environment(get_env: impl Fn(&str) -> Option<String>) -> String {
    if let Some(path) = get_env(SOCKET_PATH_ENV).filter(|v| !v.trim().is_empty()) {
        return path;
    }
    if let Some(fallback) = get_env(SOCKET_ENV).filter(|v| !v.trim().is_empty()) {
        return fallback;
    }
    String::new()
}

/// Normalize a variant token to one of `stable|nightly|staging|dev`, defaulting to `stable`.
pub fn normalize_variant(variant: &str) -> String {
    if variant.trim().is_empty() {
        return "stable".to_string();
    }
    match variant.trim().to_lowercase().as_str() {
        "nightly" => "nightly",
        "staging" => "staging",
        "dev" => "dev",
        "stable" => "stable",
        _ => "stable",
    }
    .to_string()
}

/// Build the ordered candidate pipe list a client probes: an explicit path first (if any), then
/// the preferred variant, then the remaining variants in fallback order.
pub fn build_candidate_pipes(
    explicit_path: Option<&str>,
    explicit_variant: Option<&str>,
    slug: Option<&str>,
) -> Vec<String> {
    let mut candidates = Vec::new();

    if let Some(path) = explicit_path.filter(|p| !p.trim().is_empty()) {
        candidates.push(path.to_string());
    }

    let preferred = normalize_variant(explicit_variant.unwrap_or(""));
    candidates.push(build_pipe_name(&preferred, slug));

    for variant in VARIANT_FALLBACK_ORDER {
        if variant == preferred {
            continue;
        }
        candidates.push(build_pipe_name(variant, slug));
    }

    candidates
}

/// Return the first candidate that `can_connect`, or the preferred (first) candidate when none do.
pub fn resolve_with_probe(candidates: &[String], can_connect: impl Fn(&str) -> bool) -> String {
    candidates
        .iter()
        .find(|c| can_connect(c))
        .or_else(|| candidates.first())
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_path_prefers_socket_path_over_socket() {
        let resolver = |key: &str| match key {
            SOCKET_PATH_ENV => Some(r"\\.\pipe\from-path".to_string()),
            SOCKET_ENV => Some(r"\\.\pipe\from-legacy".to_string()),
            _ => None,
        };
        assert_eq!(resolve_from_environment(resolver), r"\\.\pipe\from-path");
    }

    #[test]
    fn build_pipe_name_uses_stable_by_default() {
        assert_eq!(build_pipe_name("", None), r"\\.\pipe\optimus-stable");
    }

    #[test]
    fn build_candidate_pipes_orders_preferred_first() {
        let list = build_candidate_pipes(None, Some("nightly"), Some("beta"));
        assert_eq!(list[0], r"\\.\pipe\optimus-nightly-beta");
        assert_eq!(list[1], r"\\.\pipe\optimus-stable-beta");
    }

    #[test]
    fn resolve_with_probe_selects_first_connectable() {
        let candidates = vec![
            r"\\.\pipe\down".to_string(),
            r"\\.\pipe\up".to_string(),
            r"\\.\pipe\later".to_string(),
        ];
        let chosen = resolve_with_probe(&candidates, |pipe| pipe.contains("up"));
        assert_eq!(chosen, r"\\.\pipe\up");
    }

    #[test]
    fn resolve_with_probe_falls_back_to_preferred_when_none_connect() {
        let candidates = vec!["a".to_string(), "b".to_string()];
        assert_eq!(resolve_with_probe(&candidates, |_| false), "a");
    }

    #[test]
    fn to_local_name_strips_the_pipe_prefix() {
        assert_eq!(to_local_name(r"\\.\pipe\optimus-stable"), "optimus-stable");
        assert_eq!(to_local_name("optimus-stable"), "optimus-stable");
    }
}
