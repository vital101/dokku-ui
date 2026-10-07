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
    pub fn all() -> [Role; 3] {
        [Role::Admin, Role::Operator, Role::Viewer]
    }

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

    /// Template helper: may this role mutate apps, services, and volumes?
    pub fn can_manage_apps(&self) -> bool {
        authorize(*self, Permission::ManageApps)
    }

    /// Template helper: may this role manage users?
    pub fn is_admin(&self) -> bool {
        authorize(*self, Permission::ManageUsers)
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

/// Maps a request method + path onto the permission it needs. The auth
/// middleware consults this for every authenticated request, so the policy
/// lives here in one place instead of being sprinkled over handlers.
///
/// `users/` is admin-only for reads too (identity data); every GET is a view;
/// a small self-service set (logout, own password, re-auth, toast acks) is
/// open to any authenticated role; everything else that mutates needs
/// `ManageApps`.
pub fn permission_for(method: &str, path: &str) -> Permission {
    if path == "/users"
        || path.starts_with("/users/")
        || path == "/settings"
        || path.starts_with("/settings/")
        || path == "/keys"
        || path.starts_with("/keys/")
    {
        return Permission::ManageUsers;
    }
    if method == "GET" || method == "HEAD" || method == "OPTIONS" {
        return Permission::View;
    }
    if is_self_service(path) {
        return Permission::View;
    }
    Permission::ManageApps
}

fn is_self_service(path: &str) -> bool {
    path == "/logout"
        || path == "/password"
        || path == "/reauth"
        || (path.starts_with("/actions/runs/") && path.ends_with("/ack"))
}

/// Last-admin guard for user deletion: an admin account can never be removed
/// while it is the only one, regardless of who asks.
pub fn can_delete_user(target_role: Role, admin_count: i64) -> bool {
    !(target_role == Role::Admin && admin_count <= 1)
}

/// Last-admin guard for role changes: the final admin can never be demoted.
pub fn can_set_role(target_role: Role, new_role: Role, admin_count: i64) -> bool {
    !(target_role == Role::Admin && new_role != Role::Admin && admin_count <= 1)
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

    #[test]
    fn gets_are_views_and_mutations_manage_apps() {
        assert_eq!(permission_for("GET", "/"), Permission::View);
        assert_eq!(permission_for("GET", "/apps/myapp"), Permission::View);
        assert_eq!(permission_for("HEAD", "/volumes"), Permission::View);
        assert_eq!(permission_for("POST", "/apps"), Permission::ManageApps);
        assert_eq!(
            permission_for("POST", "/apps/myapp/restart"),
            Permission::ManageApps
        );
        assert_eq!(
            permission_for("POST", "/volumes/mount"),
            Permission::ManageApps
        );
        assert_eq!(permission_for("POST", "/refresh"), Permission::ManageApps);
    }

    #[test]
    fn users_paths_are_admin_only_for_every_method() {
        assert_eq!(permission_for("GET", "/users"), Permission::ManageUsers);
        assert_eq!(permission_for("POST", "/users"), Permission::ManageUsers);
        assert_eq!(
            permission_for("POST", "/users/3/role"),
            Permission::ManageUsers
        );
        assert_eq!(
            permission_for("POST", "/users/3/delete"),
            Permission::ManageUsers
        );
        assert_eq!(permission_for("GET", "/keys"), Permission::ManageUsers);
        assert_eq!(permission_for("POST", "/keys"), Permission::ManageUsers);
        assert_eq!(
            permission_for("POST", "/keys/remove"),
            Permission::ManageUsers
        );
        // `/username` is not under the users namespace.
        assert_eq!(permission_for("GET", "/username"), Permission::View);
        assert_eq!(permission_for("POST", "/username"), Permission::ManageApps);
    }

    #[test]
    fn self_service_posts_are_open_to_every_role() {
        for path in [
            "/logout",
            "/password",
            "/reauth",
            "/actions/runs/abc123/ack",
        ] {
            assert_eq!(permission_for("POST", path), Permission::View, "{path}");
        }
        assert_eq!(
            permission_for("POST", "/reauth/other"),
            Permission::ManageApps
        );
    }

    #[test]
    fn last_admin_cannot_be_deleted_or_demoted() {
        assert!(!can_delete_user(Role::Admin, 1));
        assert!(can_delete_user(Role::Admin, 2));
        assert!(can_delete_user(Role::Operator, 1));
        assert!(can_delete_user(Role::Viewer, 1));

        assert!(!can_set_role(Role::Admin, Role::Operator, 1));
        assert!(!can_set_role(Role::Admin, Role::Viewer, 1));
        assert!(can_set_role(Role::Admin, Role::Admin, 1));
        assert!(can_set_role(Role::Admin, Role::Operator, 2));
        assert!(can_set_role(Role::Viewer, Role::Operator, 1));
    }

    #[test]
    fn role_helpers_match_the_matrix() {
        assert!(Role::Admin.can_manage_apps());
        assert!(Role::Admin.is_admin());
        assert!(Role::Operator.can_manage_apps());
        assert!(!Role::Operator.is_admin());
        assert!(!Role::Viewer.can_manage_apps());
        assert!(!Role::Viewer.is_admin());
    }
}
