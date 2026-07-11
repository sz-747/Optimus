//! Child-only variables that bind a terminal process tree to its Optimus surface and control pipe.

use crate::cli::parser::SURFACE_ID_ENV;
use crate::domain::ids::SurfaceId;
use crate::ipc::naming::SOCKET_PATH_ENV;

/// Environment overrides for one pane's shell. Descendant agent hooks inherit these values, so a
/// caller-scoped request names the pane that launched it and returns to the same app pipe.
pub fn for_surface(surface: SurfaceId, pipe_name: &str) -> Vec<(String, String)> {
    vec![
        (SURFACE_ID_ENV.to_string(), surface.to_string()),
        (SOCKET_PATH_ENV.to_string(), pipe_name.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_surfaces_receive_distinct_caller_identity() {
        let first = for_surface(SurfaceId(3), r"\\.\pipe\optimus-test");
        let second = for_surface(SurfaceId(4), r"\\.\pipe\optimus-test");

        assert_eq!(first[0], (SURFACE_ID_ENV.to_string(), "S3".to_string()));
        assert_eq!(second[0], (SURFACE_ID_ENV.to_string(), "S4".to_string()));
        assert_eq!(first[1], second[1]);
    }
}
