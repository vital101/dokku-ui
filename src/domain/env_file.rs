use crate::domain::types::EnvVar;

/// Maximum characters a single env value may carry. Enforced because dokku's
/// SSH wrapper re-splits `$SSH_ORIGINAL_COMMAND` per token; anything wild is
/// rejected before it can mangle a command.
pub const CONFIG_VALUE_MAX_LEN: usize = 4096;

/// Env keys must match dokku's own `config:set` validator (`[A-Z_][A-Z0-9_]*`
/// case-insensitively); enforced so a form value can never inject argv.
pub fn is_valid_config_key(key: &str) -> bool {
    if key.is_empty() || key.len() > 256 {
        return false;
    }
    let Some(first) = key.chars().next() else {
        return false;
    };
    let first = first.to_ascii_uppercase();
    (first.is_ascii_alphabetic() || first == '_')
        && key.chars().all(|c| {
            let c = c.to_ascii_uppercase();
            c.is_ascii_alphanumeric() || c == '_'
        })
}

/// Validates a single `KEY=VALUE` for the SSH command line: the value must be
/// single-line (no `\n`/`\r`) and free of single quotes, because dokku's SSH
/// wrapper re-splits the command with `xargs -n 1` + `readarray`, which does
/// not survive shell-style quoting of those characters.
pub fn is_valid_config_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= CONFIG_VALUE_MAX_LEN
        && !value.contains('\n')
        && !value.contains('\r')
        && !value.contains('\'')
}

/// Parses a `.env`-style paste (one `KEY=VALUE` per line, `#` comments and
/// blank lines skipped). Quoted values are unquoted; trailing whitespace on
/// the value is trimmed only when the value was not quoted. Malformed lines
/// are skipped — the caller reports what could not be parsed.
pub fn parse_env_file(input: &str) -> (Vec<EnvVar>, usize) {
    let mut vars: Vec<EnvVar> = Vec::new();
    let mut skipped: usize = 0;
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            skipped += 1;
            continue;
        };
        let key = raw_key.trim();
        if !is_valid_config_key(key) {
            skipped += 1;
            continue;
        }
        let value = unquote_value(raw_value.trim());
        if value.is_empty() || !is_valid_config_value(&value) {
            skipped += 1;
            continue;
        }
        // Last occurrence wins, like dokku's own import.
        vars.retain(|existing| existing.key != key);
        vars.push(EnvVar {
            key: key.to_owned(),
            value,
        });
    }
    (vars, skipped)
}

/// Strips one level of matching quotes from a value (`"x"`, `'x'`, `"x y"`).
fn unquote_value(value: &str) -> String {
    if value.len() < 2 {
        return value.to_owned();
    }
    let first = value.chars().next().unwrap_or_default();
    let last = value.chars().last();
    if (first == '"' || first == '\'') && Some(first) == last {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

/// The diff between the current config and a desired `.env` set: which keys
/// to set and which to unset. Setting an identical value is a no-op.
pub fn config_diff(current: &[EnvVar], desired: &[EnvVar]) -> (Vec<EnvVar>, Vec<String>) {
    let current_map = current
        .iter()
        .map(|var| (var.key.clone(), var.value.clone()))
        .collect::<std::collections::HashMap<String, String>>();
    let mut to_set: Vec<EnvVar> = Vec::new();
    for var in desired {
        let Some(existing) = current_map.get(&var.key) else {
            to_set.push(var.clone());
            continue;
        };
        if *existing != var.value {
            to_set.push(var.clone());
        }
    }
    let desired_keys = desired
        .iter()
        .map(|var| var.key.clone())
        .collect::<Vec<String>>();
    let to_unset = current
        .iter()
        .map(|var| var.key.clone())
        .filter(|key| !desired_keys.contains(key))
        .collect::<Vec<_>>();
    (to_set, to_unset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(key: &str, value: &str) -> EnvVar {
        EnvVar {
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn keys_follow_dokku_rules() {
        for key in ["DATABASE_URL", "FOO_BAR_2", "_LEADING", "A"] {
            assert!(is_valid_config_key(key), "{key}");
        }
        for key in ["", "FOO-BAR", "FOO BAR", "1BAD", "FOO.BAR", "a b"] {
            assert!(!is_valid_config_key(key), "{key}");
        }
        assert!(is_valid_config_key("lowercase_ok"));
    }

    #[test]
    fn values_must_survive_the_ssh_wrapper() {
        for value in ["hunter2", "a b c", "https://u:p@h/path", "x=1&y=2"] {
            assert!(is_valid_config_value(value), "{value:?}");
        }
        for value in ["", "'quoted", "line\nbreak", "carriage\rreturn"] {
            assert!(!is_valid_config_value(value), "{value:?}");
        }
        assert!(!is_valid_config_value(&"a".repeat(4097)));
    }

    #[test]
    fn env_file_parses_lines_comments_and_quotes() {
        let input = "# comment\n\nDATABASE_URL=postgres://u:p@h/db\nFOO=\"a b c\"\nEMPTY=\nBAD KEY=x\nDUPLICATE=1\nDUPLICATE=2\n";
        let (vars, skipped) = parse_env_file(input);
        assert_eq!(
            vars,
            vec![
                var("DATABASE_URL", "postgres://u:p@h/db"),
                var("FOO", "a b c"),
                var("DUPLICATE", "2"),
            ]
        );
        assert_eq!(skipped, 2, "EMPTY and the invalid key are skipped");
    }

    #[test]
    fn config_diff_sets_changed_and_unsets_absent() {
        let current = vec![var("A", "1"), var("B", "2"), var("GONE", "x")];
        let desired = vec![var("A", "1"), var("B", "9"), var("NEW", "n")];
        let (to_set, to_unset) = config_diff(&current, &desired);
        assert_eq!(to_set, vec![var("B", "9"), var("NEW", "n")]);
        assert_eq!(to_unset, vec!["GONE"]);
    }
}
