use std::collections::HashMap;

pub const PUBLIC_URL_KEY: &str = "public_url";
pub const LOGIN_BANNER_KEY: &str = "login_banner";
pub const APP_FILTER_KEY: &str = "app_filter";
pub const ACTIVITY_TTL_KEY: &str = "activity_ttl_secs";
pub const RUN_LOG_TTL_KEY: &str = "run_log_ttl_secs";

pub const TTL_MIN_SECS: u64 = 60;
pub const TTL_MAX_SECS: u64 = 31_536_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstanceSettings {
    pub public_url: Option<String>,
    pub login_banner: Option<String>,
    /// Glob patterns (one per line) for apps hidden across the UI.
    pub app_filter: Vec<String>,
    pub activity_ttl_secs: Option<u64>,
    pub run_log_ttl_secs: Option<u64>,
}

impl InstanceSettings {
    /// Validates admin form input. Blank strings become unset values.
    pub fn parse(
        public_url: Option<&str>,
        login_banner: Option<&str>,
        app_filter: Option<&str>,
        activity_ttl_secs: Option<&str>,
        run_log_ttl_secs: Option<&str>,
    ) -> Result<Self, InstanceSettingsError> {
        let public_url = normalize(public_url);
        if let Some(url) = &public_url {
            let valid = url.len() <= 2048
                && (url.starts_with("http://") || url.starts_with("https://"))
                && !url
                    .chars()
                    .any(|c| c.is_whitespace() || c == '\'' || c == '"');
            if !valid {
                return Err(InstanceSettingsError::InvalidPublicUrl);
            }
        }

        let login_banner = normalize(login_banner);
        if let Some(banner) = &login_banner {
            if banner.chars().count() > 280 || banner.contains('\0') {
                return Err(InstanceSettingsError::InvalidLoginBanner);
            }
        }

        let mut patterns = Vec::new();
        if let Some(raw) = app_filter {
            for pattern in raw
                .split(['\n', ','])
                .map(str::trim)
                .filter(|p| !p.is_empty())
            {
                if !is_valid_app_pattern(pattern) {
                    return Err(InstanceSettingsError::InvalidAppFilter(pattern.to_owned()));
                }
                patterns.push(pattern.to_owned());
            }
            if patterns.len() > 50 {
                return Err(InstanceSettingsError::TooManyAppPatterns);
            }
        }

        Ok(Self {
            public_url,
            login_banner,
            app_filter: patterns,
            activity_ttl_secs: parse_ttl(activity_ttl_secs, "activity TTL")?,
            run_log_ttl_secs: parse_ttl(run_log_ttl_secs, "run-log TTL")?,
        })
    }

    pub fn app_is_hidden(&self, name: &str) -> bool {
        self.app_filter
            .iter()
            .any(|pattern| glob_matches(pattern, name))
    }

    /// Key/value rows to persist; unset fields are omitted (and cleared by the
    /// repo's delete-all-then-insert save).
    pub fn to_map(&self) -> Vec<(&'static str, String)> {
        let mut pairs = Vec::new();
        if let Some(url) = &self.public_url {
            pairs.push((PUBLIC_URL_KEY, url.clone()));
        }
        if let Some(banner) = &self.login_banner {
            pairs.push((LOGIN_BANNER_KEY, banner.clone()));
        }
        if !self.app_filter.is_empty() {
            pairs.push((APP_FILTER_KEY, self.app_filter.join("\n")));
        }
        if let Some(ttl) = self.activity_ttl_secs {
            pairs.push((ACTIVITY_TTL_KEY, ttl.to_string()));
        }
        if let Some(ttl) = self.run_log_ttl_secs {
            pairs.push((RUN_LOG_TTL_KEY, ttl.to_string()));
        }
        pairs
    }

    /// Tolerant load: unknown keys and unparseable values are ignored.
    pub fn from_map(map: &HashMap<String, String>) -> Self {
        Self {
            public_url: map.get(PUBLIC_URL_KEY).cloned().filter(|v| !v.is_empty()),
            login_banner: map.get(LOGIN_BANNER_KEY).cloned().filter(|v| !v.is_empty()),
            app_filter: map
                .get(APP_FILTER_KEY)
                .map(|raw| {
                    raw.split('\n')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .filter(|p| is_valid_app_pattern(p))
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            activity_ttl_secs: map
                .get(ACTIVITY_TTL_KEY)
                .and_then(|raw| raw.parse::<u64>().ok())
                .filter(|ttl| (TTL_MIN_SECS..=TTL_MAX_SECS).contains(ttl)),
            run_log_ttl_secs: map
                .get(RUN_LOG_TTL_KEY)
                .and_then(|raw| raw.parse::<u64>().ok())
                .filter(|ttl| (TTL_MIN_SECS..=TTL_MAX_SECS).contains(ttl)),
        }
    }
}

fn normalize(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn parse_ttl(raw: Option<&str>, field: &'static str) -> Result<Option<u64>, InstanceSettingsError> {
    match normalize(raw) {
        None => Ok(None),
        Some(value) => match value.parse::<u64>() {
            Ok(secs) if (TTL_MIN_SECS..=TTL_MAX_SECS).contains(&secs) => Ok(Some(secs)),
            _ => Err(InstanceSettingsError::InvalidTtl { field }),
        },
    }
}

fn is_valid_app_pattern(pattern: &str) -> bool {
    !pattern.is_empty()
        && pattern.len() <= 64
        && pattern.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.' | '*')
        })
}

/// Minimal glob: `*` matches any run of characters.
pub fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut pi, mut vi) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while vi < value.len() {
        if pi < pattern.len() && pattern[pi] == value[vi] {
            pi += 1;
            vi += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star = Some(pi);
            pi += 1;
            mark = vi;
        } else if let Some(star_index) = star {
            pi = star_index + 1;
            mark += 1;
            vi = mark;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstanceSettingsError {
    #[error("public URL must start with http:// or https:// and contain no spaces")]
    InvalidPublicUrl,
    #[error("login banner must be at most 280 characters")]
    InvalidLoginBanner,
    #[error("app filter pattern `{0}` is invalid (lowercase letters, digits, . _ - and * only)")]
    InvalidAppFilter(String),
    #[error("at most 50 app filter patterns are supported")]
    TooManyAppPatterns,
    #[error("{field} must be a whole number between 60 and 31536000 seconds")]
    InvalidTtl { field: &'static str },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_star_patterns() {
        assert!(glob_matches("dokku-ui", "dokku-ui"));
        assert!(glob_matches("dokku-*", "dokku-ui"));
        assert!(glob_matches("*ui", "dokku-ui"));
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("*-internal-*", "app-internal-1"));
        assert!(!glob_matches("dokku-*", "dokku"));
        assert!(!glob_matches("ui", "dokku-ui"));
        assert!(!glob_matches("a*b", "axc"));
    }

    #[test]
    fn parse_validates_url_banner_filter_and_ttls() {
        let settings = InstanceSettings::parse(
            Some("https://ui.example.com"),
            Some("Scheduled maintenance at 22:00 UTC"),
            Some("dokku-ui\n*-internal-*"),
            Some("3600"),
            Some("600"),
        )
        .expect("valid");
        assert_eq!(
            settings.public_url.as_deref(),
            Some("https://ui.example.com")
        );
        assert_eq!(settings.app_filter, vec!["dokku-ui", "*-internal-*"]);
        assert_eq!(settings.activity_ttl_secs, Some(3600));
        assert_eq!(settings.run_log_ttl_secs, Some(600));
        assert!(settings.app_is_hidden("dokku-ui"));
        assert!(settings.app_is_hidden("my-internal-1"));
        assert!(!settings.app_is_hidden("myapp"));
    }

    #[test]
    fn blank_input_clears_everything() {
        let settings = InstanceSettings::parse(Some(""), Some("  "), Some("\n\n"), Some(""), None)
            .expect("blank");
        assert_eq!(settings, InstanceSettings::default());
    }

    #[test]
    fn rejects_bad_values() {
        assert!(matches!(
            InstanceSettings::parse(Some("ftp://x"), None, None, None, None),
            Err(InstanceSettingsError::InvalidPublicUrl)
        ));
        assert!(matches!(
            InstanceSettings::parse(None, Some(&"x".repeat(281)), None, None, None),
            Err(InstanceSettingsError::InvalidLoginBanner)
        ));
        assert!(matches!(
            InstanceSettings::parse(None, None, Some("Bad Pattern"), None, None),
            Err(InstanceSettingsError::InvalidAppFilter(_))
        ));
        assert!(matches!(
            InstanceSettings::parse(None, None, None, Some("10"), None),
            Err(InstanceSettingsError::InvalidTtl { .. })
        ));
        assert!(matches!(
            InstanceSettings::parse(None, None, None, None, Some("nope")),
            Err(InstanceSettingsError::InvalidTtl { .. })
        ));
    }

    #[test]
    fn map_roundtrips_and_ignores_tampering() {
        let settings = InstanceSettings::parse(
            Some("https://ui.example.com/"),
            Some("hello"),
            Some("dokku-*"),
            Some("86400"),
            Some("3600"),
        )
        .expect("valid");
        let map: HashMap<String, String> = settings
            .to_map()
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        assert_eq!(InstanceSettings::from_map(&map), settings);

        let mut tampered = map.clone();
        tampered.insert(ACTIVITY_TTL_KEY.into(), "not-a-number".into());
        tampered.insert(APP_FILTER_KEY.into(), "UPPER\nalso bad!".into());
        let loaded = InstanceSettings::from_map(&tampered);
        assert_eq!(loaded.activity_ttl_secs, None);
        assert!(loaded.app_filter.is_empty());
    }
}
