use sqlx::SqlitePool;

use crate::auth::rbac::Role;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub password_hash: String,
    pub role: Role,
    pub created_at: i64,
}

#[async_trait::async_trait]
pub trait UsersRepo: Send + Sync {
    async fn count(&self) -> Result<i64, sqlx::Error>;
    async fn count_admins(&self) -> Result<i64, sqlx::Error>;
    async fn list(&self) -> Result<Vec<User>, sqlx::Error>;
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error>;
    async fn find_by_id(&self, id: i64) -> Result<Option<User>, sqlx::Error>;
    async fn insert(
        &self,
        email: &str,
        password_hash: &str,
        role: Role,
    ) -> Result<User, sqlx::Error>;
    async fn update_role(&self, id: i64, role: Role) -> Result<(), sqlx::Error>;
    /// Role change with the last-admin predicate enforced atomically inside
    /// the statement; returns `false` when the guard fired.
    async fn update_role_guarded(&self, id: i64, role: Role) -> Result<bool, sqlx::Error>;
    async fn update_password_hash(&self, id: i64, password_hash: &str) -> Result<(), sqlx::Error>;
    async fn delete(&self, id: i64) -> Result<(), sqlx::Error>;
    /// Deletion with the last-admin predicate enforced atomically inside the
    /// statement; returns `false` when the guard fired.
    async fn delete_guarded(&self, id: i64) -> Result<bool, sqlx::Error>;
}

#[derive(Debug, Clone)]
pub struct SqliteUsersRepo {
    pool: SqlitePool,
}

impl SqliteUsersRepo {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl UsersRepo for SqliteUsersRepo {
    async fn count(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await
    }

    async fn count_admins(&self) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role = 'admin'")
            .fetch_one(&self.pool)
            .await
    }

    async fn list(&self) -> Result<Vec<User>, sqlx::Error> {
        sqlx::query_as::<_, (i64, String, String, String, i64)>(
            "SELECT id, email, password_hash, role, created_at FROM users ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map(|rows| rows.into_iter().map(user_from_row).collect())
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, (i64, String, String, String, i64)>(
            "SELECT id, email, password_hash, role, created_at FROM users WHERE email = ?",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(user_from_row))
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, (i64, String, String, String, i64)>(
            "SELECT id, email, password_hash, role, created_at FROM users WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map(|row| row.map(user_from_row))
    }

    async fn insert(
        &self,
        email: &str,
        password_hash: &str,
        role: Role,
    ) -> Result<User, sqlx::Error> {
        let created_at = time::OffsetDateTime::now_utc().unix_timestamp();
        sqlx::query_as::<_, (i64, i64)>(
            "INSERT INTO users (email, password_hash, role, created_at) VALUES (?, ?, ?, ?) RETURNING id, created_at",
        )
        .bind(email)
        .bind(password_hash)
        .bind(role.as_str())
        .bind(created_at)
        .fetch_one(&self.pool)
        .await
        .map(|(id, created_at)| User {
            id,
            email: email.to_owned(),
            password_hash: password_hash.to_owned(),
            role,
            created_at,
        })
    }

    async fn update_role(&self, id: i64, role: Role) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET role = ? WHERE id = ?")
            .bind(role.as_str())
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn update_role_guarded(&self, id: i64, role: Role) -> Result<bool, sqlx::Error> {
        let new_role = role.as_str();
        // Same predicate as `rbac::can_set_role`, evaluated inside the
        // statement so a concurrent demote/delete can never leave zero admins.
        let updated = sqlx::query(
            "UPDATE users SET role = ? WHERE id = ? AND \
             (role <> 'admin' OR ? = 'admin' OR \
             (SELECT COUNT(*) FROM users WHERE role = 'admin') > 1)",
        )
        .bind(new_role)
        .bind(id)
        .bind(new_role)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(updated == 1)
    }

    async fn update_password_hash(&self, id: i64, password_hash: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(password_hash)
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn delete(&self, id: i64) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn delete_guarded(&self, id: i64) -> Result<bool, sqlx::Error> {
        let deleted = sqlx::query(
            "DELETE FROM users WHERE id = ? AND \
             (role <> 'admin' OR \
             (SELECT COUNT(*) FROM users WHERE role = 'admin') > 1)",
        )
        .bind(id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(deleted == 1)
    }
}

fn user_from_row(
    (id, email, password_hash, role, created_at): (i64, String, String, String, i64),
) -> User {
    User {
        id,
        email,
        password_hash,
        // Unknown roles fail closed: a tampered row degrades to the least
        // privileged role rather than silently granting admin.
        role: Role::try_from(&role).unwrap_or(Role::Viewer),
        created_at,
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::storage;

    async fn repo() -> (SqliteUsersRepo, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let url = format!("sqlite://{}/users.db", dir.path().display());
        let pool = storage::connect(&url).await.expect("connect");
        (SqliteUsersRepo::new(pool), dir)
    }

    #[tokio::test]
    async fn insert_counts_and_finds_by_email() {
        let (repo, _dir) = repo().await;
        assert_eq!(repo.count().await.expect("count"), 0);

        let user = repo
            .insert("admin@example.com", "hash", Role::Admin)
            .await
            .expect("insert");
        assert_eq!(user.email, "admin@example.com");
        assert_eq!(user.password_hash, "hash");
        assert_eq!(user.role, Role::Admin);
        assert!(user.id > 0);

        assert_eq!(repo.count().await.expect("count"), 1);
        let found = repo
            .find_by_email("admin@example.com")
            .await
            .expect("find")
            .expect("some");
        assert_eq!(found, user);
    }

    #[tokio::test]
    async fn find_by_email_is_case_insensitive() {
        let (repo, _dir) = repo().await;
        repo.insert("Admin@Example.com", "hash", Role::Operator)
            .await
            .expect("insert");
        let found = repo
            .find_by_email("admin@example.com")
            .await
            .expect("find")
            .expect("some");
        assert_eq!(found.email, "Admin@Example.com");
        assert_eq!(found.role, Role::Operator);
    }

    #[tokio::test]
    async fn find_by_unknown_email_returns_none() {
        let (repo, _dir) = repo().await;
        assert!(
            repo.find_by_email("nobody@example.com")
                .await
                .expect("find")
                .is_none()
        );
    }

    #[tokio::test]
    async fn find_by_id_returns_user() {
        let (repo, _dir) = repo().await;
        let user = repo
            .insert("admin@example.com", "hash", Role::Admin)
            .await
            .expect("insert");
        let found = repo.find_by_id(user.id).await.expect("find").expect("some");
        assert_eq!(found, user);
    }

    #[tokio::test]
    async fn find_by_unknown_id_returns_none() {
        let (repo, _dir) = repo().await;
        assert!(repo.find_by_id(999).await.expect("find").is_none());
    }

    #[tokio::test]
    async fn duplicate_email_is_rejected() {
        let (repo, _dir) = repo().await;
        repo.insert("a@b.com", "h1", Role::Admin)
            .await
            .expect("first insert");
        assert!(repo.insert("a@b.com", "h2", Role::Viewer).await.is_err());
    }

    #[tokio::test]
    async fn list_returns_users_in_insertion_order_and_counts_admins() {
        let (repo, _dir) = repo().await;
        repo.insert("first@example.com", "h", Role::Admin)
            .await
            .expect("insert");
        repo.insert("second@example.com", "h", Role::Operator)
            .await
            .expect("insert");
        repo.insert("third@example.com", "h", Role::Viewer)
            .await
            .expect("insert");

        let emails: Vec<String> = repo
            .list()
            .await
            .expect("list")
            .into_iter()
            .map(|user| user.email)
            .collect();
        assert_eq!(
            emails,
            vec![
                "first@example.com",
                "second@example.com",
                "third@example.com"
            ]
        );
        assert_eq!(repo.count_admins().await.expect("admins"), 1);
    }

    #[tokio::test]
    async fn update_role_and_password_and_delete() {
        let (repo, _dir) = repo().await;
        let user = repo
            .insert("user@example.com", "old-hash", Role::Viewer)
            .await
            .expect("insert");

        repo.update_role(user.id, Role::Operator)
            .await
            .expect("update role");
        let found = repo.find_by_id(user.id).await.expect("find").expect("some");
        assert_eq!(found.role, Role::Operator);

        repo.update_password_hash(user.id, "new-hash")
            .await
            .expect("update password");
        let found = repo.find_by_id(user.id).await.expect("find").expect("some");
        assert_eq!(found.password_hash, "new-hash");

        assert!(
            repo.delete_guarded(user.id).await.expect("delete"),
            "delete removed the row"
        );
        assert!(repo.find_by_id(user.id).await.expect("find").is_none());
        assert_eq!(repo.count().await.expect("count"), 0);
    }

    #[tokio::test]
    async fn guarded_role_change_and_delete_protect_the_last_admin() {
        let (repo, _dir) = repo().await;
        let admin = repo
            .insert("admin@example.com", "h", Role::Admin)
            .await
            .expect("insert");
        let viewer = repo
            .insert("viewer@example.com", "h", Role::Viewer)
            .await
            .expect("insert");

        assert!(
            !repo
                .update_role_guarded(admin.id, Role::Viewer)
                .await
                .expect("guarded update"),
            "the last admin cannot be demoted"
        );
        assert_eq!(repo.count_admins().await.expect("admins"), 1);
        assert!(
            !repo.delete_guarded(admin.id).await.expect("guarded delete"),
            "the last admin cannot be deleted"
        );
        assert_eq!(repo.count_admins().await.expect("admins"), 1);

        assert!(
            repo.update_role_guarded(viewer.id, Role::Admin)
                .await
                .expect("guarded update"),
            "promoting a non-admin is unaffected"
        );
        assert_eq!(repo.count_admins().await.expect("admins"), 2);
        assert!(
            repo.update_role_guarded(admin.id, Role::Viewer)
                .await
                .expect("guarded update"),
            "demoting is fine with two admins"
        );
        assert!(
            repo.delete_guarded(admin.id).await.expect("guarded delete"),
            "deleting the demoted user is fine"
        );
        assert_eq!(repo.count_admins().await.expect("admins"), 1);
    }

    #[tokio::test]
    async fn unknown_role_rows_degrade_to_viewer() {
        let (repo, _dir) = repo().await;
        let user = repo
            .insert("a@b.com", "h", Role::Admin)
            .await
            .expect("insert");
        sqlx::query("UPDATE users SET role = 'root' WHERE id = ?")
            .bind(user.id)
            .execute(&repo.pool)
            .await
            .expect("tamper role");
        let found = repo.find_by_id(user.id).await.expect("find").expect("some");
        assert_eq!(found.role, Role::Viewer, "corrupt role fails closed");
    }
}
