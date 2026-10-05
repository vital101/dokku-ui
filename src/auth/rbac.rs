use serde::{Deserialize, Serialize};

/// Built-in roles. The P0 RBAC decision (recorded in `GAPS.md`) locks in
/// **own SQLite RBAC**: roles live in our `users` table behind a pure
/// `authorize` check, with dokku's `teams`/`users` namespaces documented as a
/// future reconciliation path via the host companion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Admin,
    Operator,
    Viewer,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Operator => "operator",
            Self::Viewer => "viewer",
        }
    }

    pub fn try_from(value: &str) -> Option<Self> {
        match value {
            "admin" => Some(Self::Admin),
            "operator" => Some(Self::Operator),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Admin => "Admin",
            Self::Operator => "Operator",
            Self::Viewer => "Viewer",
        }
    }
}

/// The coarse permission set every UI action maps onto. The `Authorizer` seam
/// keeps the door open for per-app/service grants (and eventually dokku's
/// teams) without churning every handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    View,
    ManageApps,
    ManageUsers,
}

/// Pure authorization check. Extend the matrix as grants arrive; never put
/// policy in handlers.
pub fn authorize(role: Role, permission: Permission) -> bool {
    match (role, permission) {
        (Role::Admin, _) => true,
        (Role::Operator, Permission::ManageApps | Permission::View) => true,
        (Role::Operator, Permission::ManageUsers) => false,
        (Role::Viewer, Permission::View) => true,
        (Role::Viewer, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_roundtrips_through_its_storage_form() {
        for role in [Role::Admin, Role::Operator, Role::Viewer] {
            assert_eq!(Role::try_from(role.as_str()), Some(role));
        }
        assert_eq!(Role::try_from("root"), None);
        assert_eq!(Role::try_from(""), None);
    }

    #[test]
    fn admins_can_do_everything() {
        for permission in [
            Permission::View,
            Permission::ManageApps,
            Permission::ManageUsers,
        ] {
            assert!(authorize(Role::Admin, permission));
        }
    }

    #[test]
    fn operators_manage_apps_but_not_users() {
        assert!(authorize(Role::Operator, Permission::View));
        assert!(authorize(Role::Operator, Permission::ManageApps));
        assert!(!authorize(Role::Operator, Permission::ManageUsers));
    }

    #[test]
    fn viewers_only_view() {
        assert!(authorize(Role::Viewer, Permission::View));
        assert!(!authorize(Role::Viewer, Permission::ManageApps));
        assert!(!authorize(Role::Viewer, Permission::ManageUsers));
    }
}
