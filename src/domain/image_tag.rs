//! Pure helpers for the service image-upgrade selector.
//!
//! Splits an `image:tag` reference and reduces a raw Docker Hub tag list to a
//! sorted, deduplicated set of concrete version tags suitable for a dropdown.

/// Splits `repo[:tag]`, respecting a registry host with a port
/// (`localhost:5000/foo:1` -> `localhost:5000/foo`, `1`).
pub fn split_image_ref(image: &str) -> (String, Option<String>) {
    let slash = image.rfind('/');
    let colon = image.rfind(':');
    match colon {
        Some(colon) if slash.is_none_or(|slash| colon > slash) => (
            image[..colon].to_owned(),
            Some(image[colon + 1..].to_owned()),
        ),
        _ => (image.to_owned(), None),
    }
}

/// Docker Hub repository path for a bare official-image name: `redis` lives
/// at `library/redis`; anything with a slash is used as-is.
pub fn docker_hub_repo(image_repo: &str) -> String {
    if image_repo.contains('/') {
        image_repo.to_owned()
    } else {
        format!("library/{image_repo}")
    }
}

/// A concrete version tag with two or three numeric components and an optional
/// variant suffix (`8.10.2`, `7.4.11-alpine3.22`, `16.2`). Postgres publishes
/// two-component tags (`16.2`, `17.11`), so both widths are accepted; the
/// caller narrows to a single width so moving aliases (`8.8`, `16`) stay out.
pub fn is_version_tag(tag: &str, width: usize) -> bool {
    split_tag(tag).is_some_and(|(_, _, tag_width)| tag_width == width)
}

/// Number of numeric components (2 or 3) in a concrete version tag.
pub fn version_width(tag: &str) -> Option<usize> {
    split_tag(tag).map(|(_, _, width)| width)
}

/// Splits a version tag into `(base, variant, component count)` and validates
/// the variant suffix. Returns `None` for moving tags (`latest`, `8.8`) and
/// malformed input.
fn split_tag(tag: &str) -> Option<(&str, Option<&str>, usize)> {
    let (base, suffix) = match tag.split_once('-') {
        Some((base, suffix)) => (base, Some(suffix)),
        None => (tag, None),
    };
    if let Some(suffix) = suffix {
        if suffix.is_empty()
            || !suffix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        {
            return None;
        }
    }
    let parts: Vec<&str> = base.split('.').collect();
    if !(2..=3).contains(&parts.len()) || !parts.iter().all(|part| part.parse::<u64>().is_ok()) {
        return None;
    }
    Some((base, suffix, parts.len()))
}

/// Parsed `(major, minor, patch, variant)` sort key for a version tag.
type VersionKey<'a> = (u64, u64, u64, Option<&'a str>);

fn version_key(tag: &str, width: usize) -> Option<VersionKey<'_>> {
    let (base, suffix, tag_width) = split_tag(tag)?;
    if tag_width != width {
        return None;
    }
    let mut parts = base.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = match width {
        2 => 0,
        _ => parts.next()?.parse().ok()?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch, suffix))
}

/// The component width to offer: the current tag's shape when it is concrete,
/// otherwise three components when the repository publishes three-component
/// versions (redis/mysql/mongo), else two (postgres).
fn select_width(raw: &[String], current: Option<&str>) -> usize {
    if let Some(width) = current.and_then(version_width) {
        return width;
    }
    if raw
        .iter()
        .any(|tag| split_tag(tag).is_some_and(|(_, _, width)| width == 3))
    {
        3
    } else {
        2
    }
}

/// Reduces a raw tag list to concrete version tags of the current tag's width,
/// newest first (bare `x.y.z` before its variants), capped at `cap`. The
/// `current` tag is always included and placed first, even when it is a moving
/// tag or beyond the cap.
pub fn select_tags(raw: &[String], current: Option<&str>, cap: usize) -> Vec<String> {
    let width = select_width(raw, current);
    let mut keyed: Vec<(&String, VersionKey<'_>)> = raw
        .iter()
        .filter(|tag| is_version_tag(tag, width))
        .filter_map(|tag| version_key(tag, width).map(|key| (tag, key)))
        .collect();
    keyed.sort_by(|(tag_a, key_a), (tag_b, key_b)| {
        let base = (key_b.0, key_b.1, key_b.2).cmp(&(key_a.0, key_a.1, key_a.2));
        if base != std::cmp::Ordering::Equal {
            return base;
        }
        match (key_a.3, key_b.3) {
            (None, None) => tag_a.cmp(tag_b),
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => a.cmp(b),
        }
    });

    let mut seen = std::collections::HashSet::new();
    let mut tags: Vec<String> = keyed
        .into_iter()
        .map(|(tag, _)| tag.clone())
        .filter(|tag| seen.insert(tag.clone()))
        .take(cap)
        .collect();

    if let Some(current) = current {
        if !current.is_empty() {
            if let Some(position) = tags.iter().position(|tag| tag == current) {
                tags.remove(position);
            } else {
                tags.truncate(cap.saturating_sub(1));
            }
            tags.insert(0, current.to_owned());
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_tags_without_mistaking_registry_ports() {
        assert_eq!(
            split_image_ref("redis:7.2.4"),
            ("redis".to_owned(), Some("7.2.4".to_owned()))
        );
        assert_eq!(split_image_ref("redis"), ("redis".to_owned(), None));
        assert_eq!(
            split_image_ref("localhost:5000/team/service"),
            ("localhost:5000/team/service".to_owned(), None)
        );
        assert_eq!(
            split_image_ref("localhost:5000/team/service:1.2"),
            (
                "localhost:5000/team/service".to_owned(),
                Some("1.2".to_owned())
            )
        );
    }

    #[test]
    fn detects_only_concrete_version_tags_of_the_requested_width() {
        for tag in ["8.10.2", "7.4.11-alpine3.22", "8.0.0-bookworm"] {
            assert!(is_version_tag(tag, 3), "{tag}");
            assert!(!is_version_tag(tag, 2), "{tag}");
        }
        for tag in ["16.2", "17.11-bookworm"] {
            assert!(is_version_tag(tag, 2), "{tag}");
            assert!(!is_version_tag(tag, 3), "{tag}");
        }
        for tag in ["latest", "8", "bookworm", "8.10.2-", "8.x.1", "v1"] {
            assert!(!is_version_tag(tag, 2) && !is_version_tag(tag, 3), "{tag}");
        }
        assert_eq!(version_width("16.2"), Some(2));
        assert_eq!(version_width("7.2.4"), Some(3));
        assert_eq!(version_width("latest"), None);
    }

    #[test]
    fn docker_hub_repo_normalizes_official_images() {
        assert_eq!(docker_hub_repo("redis"), "library/redis");
        assert_eq!(
            docker_hub_repo("redis/redis-stack-server"),
            "redis/redis-stack-server"
        );
    }

    #[test]
    fn selects_newest_first_and_keeps_the_current_tag() {
        let raw = vec![
            "latest".to_owned(),
            "8.8".to_owned(),
            "8.8.3".to_owned(),
            "8.10.2".to_owned(),
            "7.2.4".to_owned(),
            "8.8.3-alpine".to_owned(),
            "8.10.2".to_owned(),
            "bogus".to_owned(),
            "7.2.4-alpine".to_owned(),
        ];
        let tags = select_tags(&raw, Some("7.2.4"), 10);
        assert_eq!(tags[0], "7.2.4", "current tag is first");
        assert!(tags.contains(&"8.10.2".to_owned()));
        assert!(tags.contains(&"8.8.3-alpine".to_owned()));
        assert!(!tags.contains(&"latest".to_owned()));
        assert!(!tags.contains(&"8.8".to_owned()));
        // Newest version (8.10.2) precedes older (8.8.3), bare before variant.
        let pos = |needle: &str| tags.iter().position(|tag| tag == needle).unwrap();
        assert!(pos("8.10.2") < pos("8.8.3"));
        assert!(pos("8.8.3") < pos("8.8.3-alpine"));
    }

    #[test]
    fn includes_a_current_tag_that_is_not_a_version_tag() {
        let raw = vec!["8.8.3".to_owned()];
        let tags = select_tags(&raw, Some("latest"), 10);
        assert_eq!(tags[0], "latest");
        assert!(tags.contains(&"8.8.3".to_owned()));
    }

    #[test]
    fn caps_the_result() {
        let raw: Vec<String> = (0..50).map(|n| format!("1.0.{n}")).collect();
        assert_eq!(select_tags(&raw, None, 10).len(), 10);
    }

    #[test]
    fn caps_the_result_when_the_current_tag_is_pinned() {
        let raw: Vec<String> = (0..50).map(|n| format!("1.0.{n}")).collect();
        let tags = select_tags(&raw, Some("9.9.9"), 10);
        assert_eq!(tags.len(), 10, "{tags:?}");
        assert_eq!(tags[0], "9.9.9");
    }

    #[test]
    fn selects_two_component_postgres_tags() {
        let raw = vec![
            "latest".to_owned(),
            "16".to_owned(),
            "16.2".to_owned(),
            "16.2-bookworm".to_owned(),
            "17.11".to_owned(),
            "18.6".to_owned(),
        ];
        let tags = select_tags(&raw, Some("16.2"), 10);
        assert_eq!(tags[0], "16.2", "current tag is first");
        assert!(tags.contains(&"17.11".to_owned()));
        assert!(tags.contains(&"18.6".to_owned()));
        assert!(tags.contains(&"16.2-bookworm".to_owned()));
        assert!(!tags.contains(&"16".to_owned()));
        assert!(!tags.contains(&"latest".to_owned()));
    }

    #[test]
    fn infers_two_component_width_for_two_component_repositories() {
        let raw = vec!["16".to_owned(), "16.2".to_owned(), "17.11".to_owned()];
        let tags = select_tags(&raw, Some("latest"), 10);
        assert_eq!(tags[0], "latest");
        assert!(tags.contains(&"17.11".to_owned()));
        assert!(!tags.contains(&"16".to_owned()));
    }

    #[test]
    fn keeps_three_component_width_when_the_current_tag_is_moving() {
        let raw = vec![
            "8.8".to_owned(),
            "8.8.3".to_owned(),
            "8.10".to_owned(),
            "8.10.2".to_owned(),
        ];
        let tags = select_tags(&raw, Some("latest"), 10);
        assert!(tags.contains(&"8.10.2".to_owned()));
        assert!(!tags.contains(&"8.8".to_owned()));
        assert!(!tags.contains(&"8.10".to_owned()));
    }
}
