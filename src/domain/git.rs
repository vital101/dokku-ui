/// The only `git:set` property the UI exposes; blank clears it and falls back
/// to the `--global` deploy branch.
pub const DEPLOY_BRANCH_PROPERTY: &str = "deploy-branch";

/// How `git:sync` should treat the fetched code (`--build`,
/// `--build-if-changes`, or neither = fetch only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum GitBuildMode {
    Build,
    BuildIfChanges,
    NoBuild,
}

impl GitBuildMode {
    /// The argv flag, if any. `git:sync` takes it before the app name.
    pub fn flag(&self) -> Option<&'static str> {
        match self {
            Self::Build => Some("--build"),
            Self::BuildIfChanges => Some("--build-if-changes"),
            Self::NoBuild => None,
        }
    }

    /// Stable form-encoded/storage spelling (`build_mode` form field and the
    /// `app_webhooks.build_mode` column).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::BuildIfChanges => "build-if-changes",
            Self::NoBuild => "no-build",
        }
    }

    /// Parses [`Self::as_str`]; a blank value means `NoBuild` (the form's
    /// "never" option is sometimes submitted empty).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "build" => Some(Self::Build),
            "build-if-changes" => Some(Self::BuildIfChanges),
            "" | "no-build" => Some(Self::NoBuild),
            _ => None,
        }
    }
}

/// A git branch or ref, used for `git:set <app> deploy-branch <value>` and the
/// optional `<git-ref>` of `git:sync`. A conservative subset of
/// `git check-ref-format`: the charset also guarantees SSH-wrapper safety
/// (single-line, quote-free).
pub fn is_valid_git_ref(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('-')
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("..")
        && !value.contains("//")
        && !value.contains("@{")
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
}

/// A git remote for `git:sync`: `https://`, `ssh://`, or scp-style
/// `git@host:path`. Must stay single-line and quote-free (the dokku SSH
/// wrapper re-splits argv with `xargs -n 1`).
pub fn is_valid_git_remote(value: &str) -> bool {
    if value.is_empty() || value.len() > 2048 {
        return false;
    }
    if value
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '\'' | '"' | '`'))
    {
        return false;
    }
    if value.starts_with("https://") || value.starts_with("ssh://") {
        return true;
    }
    // scp-style: user@host:path, e.g. git@github.com:org/repo.git
    value
        .split_once('@')
        .is_some_and(|(user, rest)| !user.is_empty() && rest.contains(':'))
}

/// A docker image reference for `git:from-image`. Registry-safe charset only;
/// no shell metacharacters, no whitespace.
pub fn is_valid_image_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('-')
        && !value.starts_with('/')
        && !value.starts_with('.')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '@'))
}

/// An archive URL for `git:from-archive`: http(s), single-line, quote-free.
pub fn is_valid_archive_url(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 2048
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '\'' | '"' | '`'))
        && (value.starts_with("https://") || value.starts_with("http://"))
}

/// Credentials embedded in a remote URL that must never surface in run logs.
/// For `https://user:token@host/repo` this returns the full URL, the
/// `user:token` userinfo, and the bare token — error output can echo any of
/// them, and literal redaction covers every occurrence.
pub fn remote_secret_fragments(url: &str) -> Vec<String> {
    let mut fragments = vec![url.to_owned()];
    if let Some((_, rest)) = url.split_once("://") {
        if let Some((userinfo, _)) = rest.split_once('@') {
            if userinfo.contains(':') {
                fragments.push(userinfo.to_owned());
                if let Some((_, password)) = userinfo.split_once(':') {
                    if !password.is_empty() {
                        fragments.push(password.to_owned());
                    }
                }
            }
        }
    }
    fragments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_refs_accept_branches_and_reject_junk() {
        for value in ["main", "master", "feature/thing", "release-1.2", "v1.0.0"] {
            assert!(is_valid_git_ref(value), "{value}");
        }
        for value in [
            "",
            "-main",
            "/main",
            "main/",
            "feat..ure",
            "a//b",
            "a@{b",
            "it's",
            "two words",
            "main\nnext",
        ] {
            assert!(!is_valid_git_ref(value), "{value:?}");
        }
        assert!(!is_valid_git_ref(&"x".repeat(256)));
    }

    #[test]
    fn git_remotes_accept_https_ssh_and_scp_forms() {
        for value in [
            "https://github.com/org/repo.git",
            "https://user:token@github.com/org/repo.git",
            "ssh://git@github.com/org/repo.git",
            "git@github.com:org/repo.git",
            "git@github.com:org/repo",
        ] {
            assert!(is_valid_git_remote(value), "{value}");
        }
        for value in [
            "",
            "github.com/org/repo",
            "git://github.com/org/repo.git",
            "https://github.com/it's/repo",
            "https://github.com/a repo",
            "git@github.com:org/repo\nHEAD",
        ] {
            assert!(!is_valid_git_remote(value), "{value:?}");
        }
    }

    #[test]
    fn image_refs_accept_registry_safe_charsets() {
        for value in [
            "alpine",
            "alpine:3.19",
            "ghcr.io/org/app:v1.2.3",
            "registry.example.com:5000/team/app",
            "app@sha256:abc123",
        ] {
            assert!(is_valid_image_ref(value), "{value}");
        }
        for value in ["", "-app", "/app", "it's", "app name", "app;rm"] {
            assert!(!is_valid_image_ref(value), "{value:?}");
        }
    }

    #[test]
    fn archive_urls_accept_http_and_https_only() {
        for value in [
            "https://example.com/app.tar.gz",
            "http://example.com/app.zip",
        ] {
            assert!(is_valid_archive_url(value), "{value}");
        }
        for value in [
            "",
            "ftp://example.com/app.tar.gz",
            "https://example.com/it's.tar.gz",
            "https://example.com/a b.tar.gz",
        ] {
            assert!(!is_valid_archive_url(value), "{value:?}");
        }
    }

    #[test]
    fn build_modes_map_to_sync_flags() {
        assert_eq!(GitBuildMode::Build.flag(), Some("--build"));
        assert_eq!(
            GitBuildMode::BuildIfChanges.flag(),
            Some("--build-if-changes")
        );
        assert_eq!(GitBuildMode::NoBuild.flag(), None);
    }

    #[test]
    fn build_modes_roundtrip_through_strings() {
        for mode in [
            GitBuildMode::Build,
            GitBuildMode::BuildIfChanges,
            GitBuildMode::NoBuild,
        ] {
            assert_eq!(GitBuildMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(GitBuildMode::parse(""), Some(GitBuildMode::NoBuild));
        assert_eq!(GitBuildMode::parse("always"), None);
    }

    #[test]
    fn remote_secret_fragments_cover_userinfo_and_token() {
        assert_eq!(
            remote_secret_fragments("https://alice:tok3n@github.com/org/repo.git"),
            vec![
                "https://alice:tok3n@github.com/org/repo.git",
                "alice:tok3n",
                "tok3n",
            ]
        );
        assert_eq!(
            remote_secret_fragments("https://github.com/org/repo.git"),
            vec!["https://github.com/org/repo.git"]
        );
        assert_eq!(
            remote_secret_fragments("git@github.com:org/repo.git"),
            vec!["git@github.com:org/repo.git"],
            "scp-style user is not a secret"
        );
        assert_eq!(
            remote_secret_fragments("https://github.com/a@b/repo.git"),
            vec!["https://github.com/a@b/repo.git"],
            "an @ in the path without a scheme userinfo is not a credential"
        );
    }
}
