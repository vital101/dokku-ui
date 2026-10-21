//! Advanced `<plugin>:upgrade` options, source-verified against the installed
//! datastore-plugin generation (`redis:upgrade` 1.42.1, `postgres:upgrade`
//! 1.36.4, `mysql:upgrade` 1.44.3, `mongo:upgrade` 1.40.4). Reuses the
//! create-option validators for `--image`/`--image-version`/`--custom-env`/
//! `--config-options` and adds `--restart-apps`, `--shm-size`, and the three
//! network flags. Every value is normalized to `None` when blank and validated
//! to survive the SSH argv re-split.

use serde::{Deserialize, Serialize};

use crate::domain::service_create::{ServiceCreateOptions, ServiceCreateOptionsError};

const SHM_SIZE_MAX_LEN: usize = 32;
const NETWORK_MAX_LEN: usize = 128;

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServiceUpgradeOptions {
    #[serde(flatten)]
    pub base: ServiceCreateOptions,
    #[serde(default)]
    pub restart_apps: bool,
    #[serde(default)]
    pub shm_size: Option<String>,
    #[serde(default)]
    pub initial_network: Option<String>,
    #[serde(default)]
    pub post_create_network: Option<String>,
    #[serde(default)]
    pub post_start_network: Option<String>,
}

impl ServiceUpgradeOptions {
    #[allow(clippy::too_many_arguments)]
    pub fn parse(
        image: Option<&str>,
        image_version: Option<&str>,
        custom_env: Option<&str>,
        config_options: Option<&str>,
        restart_apps: bool,
        shm_size: Option<&str>,
        initial_network: Option<&str>,
        post_create_network: Option<&str>,
        post_start_network: Option<&str>,
    ) -> Result<Self, ServiceUpgradeOptionsError> {
        let options = Self {
            base: ServiceCreateOptions::parse(image, image_version, custom_env, config_options)?,
            restart_apps,
            shm_size: normalize(shm_size),
            initial_network: normalize(initial_network),
            post_create_network: normalize(post_create_network),
            post_start_network: normalize(post_start_network),
        };
        options.validate()?;
        Ok(options)
    }

    /// Re-checks values rehydrated from persisted job payloads.
    pub fn validate(&self) -> Result<(), ServiceUpgradeOptionsError> {
        self.base.validate()?;
        if let Some(shm) = &self.shm_size {
            if !is_valid_shm_size(shm) {
                return Err(ServiceUpgradeOptionsError::InvalidShmSize);
            }
        }
        for network in [
            &self.initial_network,
            &self.post_create_network,
            &self.post_start_network,
        ]
        .into_iter()
        .flatten()
        {
            if !is_valid_network_list(network) {
                return Err(ServiceUpgradeOptionsError::InvalidNetwork);
            }
        }
        Ok(())
    }

    /// The `--flag value` argv tail appended after `service`.
    pub fn flags(&self) -> Vec<String> {
        let mut flags = Vec::new();
        let mut push = |flag: &str, value: &str| {
            flags.push(flag.to_owned());
            flags.push(value.to_owned());
        };
        if let Some(image) = &self.base.image {
            push("--image", image);
        }
        if let Some(version) = &self.base.image_version {
            push("--image-version", version);
        }
        if let Some(custom_env) = &self.base.custom_env {
            push("--custom-env", custom_env);
        }
        if let Some(config_options) = &self.base.config_options {
            push("--config-options", config_options);
        }
        if self.restart_apps {
            push("--restart-apps", "true");
        }
        if let Some(shm) = &self.shm_size {
            push("--shm-size", shm);
        }
        if let Some(network) = &self.initial_network {
            push("--initial-network", network);
        }
        if let Some(network) = &self.post_create_network {
            push("--post-create-network", network);
        }
        if let Some(network) = &self.post_start_network {
            push("--post-start-network", network);
        }
        flags
    }

    pub fn redaction_fragments(&self) -> Vec<String> {
        self.base.redaction_fragments()
    }
}

fn normalize(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Docker size strings (`1g`, `512m`, `1073741824`); quote/whitespace-free by
/// construction.
fn is_valid_shm_size(value: &str) -> bool {
    let (digits, unit) = match value.chars().last() {
        Some(last) if last.is_ascii_alphabetic() => {
            (&value[..value.len() - 1], Some(last.to_ascii_lowercase()))
        }
        _ => (value, None),
    };
    !digits.is_empty()
        && value.len() <= SHM_SIZE_MAX_LEN
        && digits.chars().all(|c| c.is_ascii_digit())
        && unit.is_none_or(|unit| matches!(unit, 'b' | 'k' | 'm' | 'g'))
}

/// One or more comma-separated docker network names.
fn is_valid_network_list(value: &str) -> bool {
    value.split(',').all(|token| {
        let token = token.trim();
        !token.is_empty()
            && token.len() <= NETWORK_MAX_LEN
            && token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    })
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceUpgradeOptionsError {
    #[error(transparent)]
    Base(#[from] ServiceCreateOptionsError),
    #[error("shared memory size must look like `512m` or `1g`")]
    InvalidShmSize,
    #[error("network names must be comma-separated and free of spaces or quotes")]
    InvalidNetwork,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_flags_for_every_option() {
        let options = ServiceUpgradeOptions::parse(
            Some("redis"),
            Some("7.4.11"),
            Some("USER=alpha;HOST=beta"),
            Some("--log-level=debug"),
            true,
            Some("1g"),
            Some("initial"),
            Some("post-create,other"),
            Some("post-start"),
        )
        .expect("valid options");
        assert_eq!(
            options.flags(),
            vec![
                "--image",
                "redis",
                "--image-version",
                "7.4.11",
                "--custom-env",
                "USER=alpha;HOST=beta",
                "--config-options",
                "--log-level=debug",
                "--restart-apps",
                "true",
                "--shm-size",
                "1g",
                "--initial-network",
                "initial",
                "--post-create-network",
                "post-create,other",
                "--post-start-network",
                "post-start",
            ]
        );
        assert_eq!(options.redaction_fragments(), vec!["alpha", "beta"]);
    }

    #[test]
    fn blank_optionals_are_omitted() {
        let options = ServiceUpgradeOptions::parse(
            Some("redis"),
            Some("7.4.11"),
            None,
            Some("  "),
            false,
            Some(""),
            None,
            None,
            None,
        )
        .expect("valid");
        assert_eq!(
            options.flags(),
            vec!["--image", "redis", "--image-version", "7.4.11"]
        );
    }

    #[test]
    fn rejects_unsafe_values() {
        for shm in ["1 g", "1h", "big"] {
            assert!(
                ServiceUpgradeOptions::parse(
                    None,
                    None,
                    None,
                    None,
                    false,
                    Some(shm),
                    None,
                    None,
                    None
                )
                .is_err(),
                "{shm}"
            );
        }
        for network in ["has space", "a'b", "a,,b"] {
            assert!(
                ServiceUpgradeOptions::parse(
                    None,
                    None,
                    None,
                    None,
                    false,
                    None,
                    Some(network),
                    None,
                    None
                )
                .is_err(),
                "{network}"
            );
        }
    }

    #[test]
    fn rehydration_validate_rejects_tampered_shm_and_networks() {
        let tampered = ServiceUpgradeOptions {
            shm_size: Some("1 g".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            tampered.validate(),
            Err(ServiceUpgradeOptionsError::InvalidShmSize)
        );
        let tampered = ServiceUpgradeOptions {
            post_start_network: Some("bad'net".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            tampered.validate(),
            Err(ServiceUpgradeOptionsError::InvalidNetwork)
        );
    }

    #[test]
    fn defers_base_flag_validation() {
        let err = ServiceUpgradeOptions::parse(
            Some("-rm"),
            None,
            None,
            None,
            false,
            None,
            None,
            None,
            None,
        )
        .expect_err("bad image");
        assert!(matches!(err, ServiceUpgradeOptionsError::Base(_)));
    }
}
