use serde::{Deserialize, Serialize};

use crate::domain::env_file::{is_valid_config_key, is_valid_config_value};
use crate::domain::git::is_valid_image_ref;

const IMAGE_VERSION_MAX_LEN: usize = 128;
const CONFIG_OPTIONS_MAX_LEN: usize = 512;

/// Advanced `<plugin>:create` options, source-verified against the installed
/// service-plugin generation (dokku-postgres 1.36.4 `subcommands/create`):
/// `--image`, `--image-version`, `--custom-env "A=B;C=D"`, and
/// `--config-options "--flag value"`. Every field is normalized to `None` when
/// blank, and validated to survive the SSH argv re-split (single-line,
/// quote-free).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServiceCreateOptions {
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub image_version: Option<String>,
    #[serde(default)]
    pub custom_env: Option<String>,
    #[serde(default)]
    pub config_options: Option<String>,
}

impl ServiceCreateOptions {
    /// Validates raw form input into a normalized plan. Blank strings become
    /// `None`.
    pub fn parse(
        image: Option<&str>,
        image_version: Option<&str>,
        custom_env: Option<&str>,
        config_options: Option<&str>,
    ) -> Result<Self, ServiceCreateOptionsError> {
        let options = Self {
            image: normalize(image),
            image_version: normalize(image_version),
            custom_env: match normalize(custom_env) {
                Some(raw) => Some(normalize_custom_env(&raw)?),
                None => None,
            },
            config_options: normalize(config_options),
        };
        options.validate()?;
        Ok(options)
    }

    /// Re-checks values rehydrated from persisted job payloads.
    pub fn validate(&self) -> Result<(), ServiceCreateOptionsError> {
        if let Some(image) = &self.image {
            if !is_valid_image_ref(image) {
                return Err(ServiceCreateOptionsError::InvalidImage);
            }
        }
        if let Some(version) = &self.image_version {
            let valid = !version.is_empty()
                && version.len() <= IMAGE_VERSION_MAX_LEN
                && version
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
            if !valid {
                return Err(ServiceCreateOptionsError::InvalidImageVersion);
            }
        }
        if let Some(custom_env) = &self.custom_env {
            normalize_custom_env(custom_env)?;
        }
        if let Some(options) = &self.config_options {
            let valid = !options.is_empty()
                && options.len() <= CONFIG_OPTIONS_MAX_LEN
                && !options.contains('\'')
                && !options.contains('\n')
                && !options.contains('\r');
            if !valid {
                return Err(ServiceCreateOptionsError::InvalidConfigOptions);
            }
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// Literal secret fragments (custom-env values) for run-line redaction.
    pub fn redaction_fragments(&self) -> Vec<String> {
        let Some(custom_env) = &self.custom_env else {
            return Vec::new();
        };
        custom_env
            .split(';')
            .filter_map(|pair| pair.split_once('='))
            .map(|(_, value)| value.to_owned())
            .filter(|value| !value.is_empty())
            .collect()
    }
}

fn normalize(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// Accepts `KEY=VALUE` pairs joined by `;` (the plugin generation's delimiter)
/// and returns the canonical form. Keys use the config-key rules; values must
/// be non-empty, single-line, and quote-free so they survive dokku's SSH
/// wrapper re-split.
fn normalize_custom_env(raw: &str) -> Result<String, ServiceCreateOptionsError> {
    let mut pairs = Vec::new();
    for segment in raw.split(';') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let Some((key, value)) = segment.split_once('=') else {
            return Err(ServiceCreateOptionsError::InvalidCustomEnv);
        };
        let key = key.trim();
        let value = value.trim();
        if !is_valid_config_key(key) || !is_valid_config_value(value) {
            return Err(ServiceCreateOptionsError::InvalidCustomEnv);
        }
        pairs.push(format!("{key}={value}"));
    }
    if pairs.is_empty() {
        return Err(ServiceCreateOptionsError::InvalidCustomEnv);
    }
    Ok(pairs.join(";"))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceCreateOptionsError {
    #[error("image must be a valid container image reference")]
    InvalidImage,
    #[error("image version must be letters, numbers, dots, underscores, or hyphens")]
    InvalidImageVersion,
    #[error("environment variables must be KEY=VALUE pairs separated by semicolons")]
    InvalidCustomEnv,
    #[error("config options must be a single quote-free line")]
    InvalidConfigOptions,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_inputs_normalize_to_none() {
        let options = ServiceCreateOptions::parse(Some(""), Some("  "), None, Some(""))
            .expect("blank options");
        assert!(options.is_empty());
    }

    #[test]
    fn accepts_valid_image_and_version() {
        let options =
            ServiceCreateOptions::parse(Some("postgres"), Some("16.2"), None, None).expect("valid");
        assert_eq!(options.image.as_deref(), Some("postgres"));
        assert_eq!(options.image_version.as_deref(), Some("16.2"));
    }

    #[test]
    fn rejects_invalid_image() {
        for image in ["-flag", "/root", ".hidden", "bad image", "img'quote"] {
            assert!(
                ServiceCreateOptions::parse(Some(image), None, None, None).is_err(),
                "{image}"
            );
        }
    }

    #[test]
    fn rejects_invalid_image_version() {
        for version in ["bad version", "v1'2", "tag\nline", ""] {
            let result = ServiceCreateOptions::parse(None, Some(version), None, None);
            if version.is_empty() {
                assert!(result.is_ok(), "empty normalizes away");
            } else {
                assert!(result.is_err(), "{version}");
            }
        }
    }

    #[test]
    fn parses_custom_env_pairs_and_normalizes_whitespace() {
        let options =
            ServiceCreateOptions::parse(None, None, Some(" USER = alpha ; HOST = beta "), None)
                .expect("valid env");
        assert_eq!(options.custom_env.as_deref(), Some("USER=alpha;HOST=beta"));
        assert_eq!(options.redaction_fragments(), vec!["alpha", "beta"]);
    }

    #[test]
    fn rejects_malformed_custom_env() {
        for raw in [
            "USER",
            "=nokey",
            "1BAD=value",
            "USER=has'quote",
            "USER=",
            ";",
        ] {
            assert!(
                ServiceCreateOptions::parse(None, None, Some(raw), None).is_err(),
                "{raw}"
            );
        }
        assert!(ServiceCreateOptions::parse(None, None, Some(""), None).is_ok());
        assert!(ServiceCreateOptions::parse(None, None, Some("   "), None).is_ok());
    }

    #[test]
    fn rejects_config_options_with_quotes_or_newlines() {
        assert!(ServiceCreateOptions::parse(None, None, None, Some("--log-level=debug")).is_ok());
        for raw in ["it's bad", "line\nbreak", "cr\rlf"] {
            assert!(
                ServiceCreateOptions::parse(None, None, None, Some(raw)).is_err(),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn rehydration_validate_rejects_tampered_values() {
        let tampered = ServiceCreateOptions {
            image: Some("-rm".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            tampered.validate(),
            Err(ServiceCreateOptionsError::InvalidImage)
        );

        let tampered = ServiceCreateOptions {
            custom_env: Some("USER=has'quote".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            tampered.validate(),
            Err(ServiceCreateOptionsError::InvalidCustomEnv)
        );
    }
}
