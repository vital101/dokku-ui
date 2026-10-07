//! Scheduler selection validation.
//!
//! On dokku 0.38.4 `scheduler:set` accepts only the `selected` and `shell`
//! properties (`plugins/scheduler/scheduler.go::DefaultProperties`); the
//! per-scheduler property namespaces (`docker-local.*`, `k3s.*`) arrive in
//! later dokku releases. The UI therefore exposes scheduler selection only.

/// Scheduler names a `scheduler:set … selected` call may carry. `docker-local`
/// is built into dokku; `k3s` and `null` are separate core plugins the UI
/// only offers when the capability probe lists them.
pub const SCHEDULERS: [&str; 3] = ["docker-local", "k3s", "null"];

pub fn is_valid_scheduler(value: &str) -> bool {
    SCHEDULERS.contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_known_schedulers_only() {
        for value in ["docker-local", "k3s", "null"] {
            assert!(is_valid_scheduler(value), "{value}");
        }
        for value in ["", "Docker-Local", "kubernetes", "docker_local"] {
            assert!(!is_valid_scheduler(value), "{value}");
        }
    }
}
