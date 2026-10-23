# 01 - Storage entry domain types

Read `00-overview.md` first. This file only adds pure validation. Do not touch commands, jobs, web handlers, or templates yet.

## Files

- Create `src/domain/storage_entry.rs`.
- Update `src/domain/mod.rs` to add `pub mod storage_entry;` and re-export `StorageEntryName`, `StorageEntryNameError`, `is_valid_volume_container_path`.
- Do not modify `src/domain/mount_spec.rs`.

## `StorageEntryName`

Dokku 0.38.4 `ValidateEntryName` rules (source-verified in `plugins/storage/entry.go`):

- non-empty
- at most 45 characters (`MaxEntryNameLength`)
- DNS-1123 label: `^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`

Dokku also rejects names starting with `legacy-` when **creating** a new entry, but existing `legacy-*` entries are valid attach targets. Keep this distinction in the type API.

Implement a newtype:

```rust
pub const MAX_STORAGE_ENTRY_NAME_LEN: usize = 45;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorageEntryName(String);

impl StorageEntryName {
    pub fn as_str(&self) -> &str { &self.0 }

    /// Validate a name for `storage:create`; rejects the reserved `legacy-` prefix.
    pub fn try_from_create(value: &str) -> Result<Self, StorageEntryNameError> { /* ... */ }

    /// True when the name is an existing reserved/legacy entry.
    pub fn is_reserved_legacy_name(&self) -> bool {
        self.0.starts_with("legacy-")
    }
}

impl TryFrom<&str> for StorageEntryName { /* shape only; allows legacy-* */ }
impl TryFrom<String> for StorageEntryName { /* ... */ }
impl std::str::FromStr for StorageEntryName { /* ... */ }
impl std::fmt::Display for StorageEntryName { /* ... */ }
```

Use a `thiserror` enum:

```rust
pub enum StorageEntryNameError {
    Empty,
    TooLong,
    InvalidCharacter,
    ReservedPrefix,
}
```

Keep the invalid character in the error where useful, matching the style of `AppName` and `ServiceName`.

## Container path validator

Add:

```rust
pub fn is_valid_volume_container_path(path: &str) -> bool
```

Accept only:

- non-empty
- at most 1024 characters
- starts with `/`
- no whitespace, `:`, `'`, `"`, newline, carriage return, or control characters

This mirrors the `MountSpec` constraints that matter for the named-mount flag path.

## Tests

Add `#[cfg(test)] mod tests` in the new module:

- accepts `uploads`, `a`, `my-volume`, `a1-b2`, exactly 45 chars
- rejects empty, `Uploads`, `my_volume`, `my.volume`, `-leading`, `trailing-`, 46 chars
- `StorageEntryName::try_from("legacy-151e4f1a23")` is Ok (existing entry)
- `StorageEntryName::try_from_create("legacy-151e4f1a23")` is ReservedPrefix
- accepts `/data`, `/app/uploads`, `/var/lib/app`
- rejects empty, `relative`, `/bad path`, `/bad:path`, `/it's`, `/line\nbreak`, `a`.repeat(1025)

Focused command:

```text
docker compose run --rm web cargo test storage_entry
```

## Definition of done

- New module exported.
- Only pure code and tests added.
- No `unwrap`/`expect` outside tests.
