use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub email: String,
    pub password_hash: String,
    pub created_at: i64,
}

#[async_trait::async_trait]
pub trait UsersRepo: Send + Sync {
    async fn count(&self) -> Result<i64, sqlx::Error>;
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error>;
    async fn insert(&self, email: &str, password_hash: &str) -> Result<User, sqlx::Error>;
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

    async fn find_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, (i64, String, String, i64)>(
            "SELECT id, email, password_hash, created_at FROM users WHERE email = ?",
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map(|row| {
            row.map(|(id, email, password_hash, created_at)| User {
                id,
                email,
                password_hash,
                created_at,
            })
        })
    }

    async fn insert(&self, email: &str, password_hash: &str) -> Result<User, sqlx::Error> {
        let created_at = time::OffsetDateTime::now_utc().unix_timestamp();
        sqlx::query_as::<_, (i64, i64)>(
            "INSERT INTO users (email, password_hash, created_at) VALUES (?, ?, ?) RETURNING id, created_at",
        )
        .bind(email)
        .bind(password_hash)
        .bind(created_at)
        .fetch_one(&self.pool)
        .await
        .map(|(id, created_at)| User {
            id,
            email: email.to_owned(),
            password_hash: password_hash.to_owned(),
            created_at,
        })
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
            .insert("admin@example.com", "hash")
            .await
            .expect("insert");
        assert_eq!(user.email, "admin@example.com");
        assert_eq!(user.password_hash, "hash");
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
        repo.insert("Admin@Example.com", "hash")
            .await
            .expect("insert");
        let found = repo
            .find_by_email("admin@example.com")
            .await
            .expect("find")
            .expect("some");
        assert_eq!(found.email, "Admin@Example.com");
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
    async fn duplicate_email_is_rejected() {
        let (repo, _dir) = repo().await;
        repo.insert("a@b.com", "h1").await.expect("first insert");
        assert!(repo.insert("a@b.com", "h2").await.is_err());
    }
}
