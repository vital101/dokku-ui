mod common;

use actix_web::cookie::Cookie;
use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    complete_setup, extract_csrf, form_request, get_body, location, login, seed_user,
    seed_user_with_role, test_state,
};

use dokku_ui::auth::rbac::Role;
use dokku_ui::storage::users::{SqliteUsersRepo, UsersRepo};
use dokku_ui::web::build_app;

async fn csrf_for<S, B, E>(app: &S, path: &str, cookie: &Cookie<'static>) -> String
where
    S: actix_web::dev::Service<
            actix_http::Request,
            Response = actix_web::dev::ServiceResponse<B>,
            Error = E,
        >,
    B: actix_web::body::MessageBody,
    E: std::fmt::Debug,
{
    let resp = test::call_service(
        app,
        test::TestRequest::get()
            .uri(path)
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    extract_csrf(&get_body(resp).await)
}

#[tokio::test]
async fn admin_sees_users_page_and_can_create_users() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state.clone())).await;
    let admin = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/users")
            .cookie(admin.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("admin@example.com"));
    assert!(body.contains("Add user"));

    let csrf = extract_csrf(&body);
    let resp = test::call_service(
        &app,
        form_request(
            "/users",
            format!(
                "csrf_token={csrf}&email=operator%40example.com&role=operator&password=correct-horse-battery&confirm=correct-horse-battery"
            ),
        )
        .cookie(admin.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/users");

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/users")
            .cookie(admin)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("operator@example.com"));
    assert!(body.contains("Operator"));

    let repo = SqliteUsersRepo::new(state.db.clone());
    let user = repo
        .find_by_email("operator@example.com")
        .await
        .expect("find")
        .expect("some");
    assert_eq!(user.role, Role::Operator);
}

#[tokio::test]
async fn admin_create_rejects_duplicates_and_bad_input() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;
    let admin = login(&app, "admin@example.com", "correct-horse-battery").await;

    let csrf = csrf_for(&app, "/users", &admin).await;
    let resp = test::call_service(
        &app,
        form_request(
            "/users",
            format!(
                "csrf_token={csrf}&email=admin%40example.com&role=viewer&password=correct-horse-battery&confirm=correct-horse-battery"
            ),
        )
        .cookie(admin.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("already exists"));

    let resp = test::call_service(
        &app,
        form_request(
            "/users",
            format!(
                "csrf_token={csrf}&email=not-an-email&role=viewer&password=correct-horse-battery&confirm=correct-horse-battery"
            ),
        )
        .cookie(admin.clone())
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("valid email"));

    let resp = test::call_service(
        &app,
        form_request(
            "/users",
            format!(
                "csrf_token={csrf}&email=new%40example.com&role=viewer&password=short&confirm=short"
            ),
        )
        .cookie(admin)
        .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("12 characters"));
}

#[tokio::test]
async fn operator_can_manage_apps_but_not_users() {
    let (state, _dir) = test_state().await;
    seed_user_with_role(
        &state,
        "operator@example.com",
        "correct-horse-battery",
        Role::Operator,
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let operator = login(&app, "operator@example.com", "correct-horse-battery").await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(operator.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/users")
            .cookie(operator.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let csrf = csrf_for(&app, "/apps/new", &operator).await;
    let resp = test::call_service(
        &app,
        form_request("/apps", format!("csrf_token={csrf}&name=myapp"))
            .cookie(operator.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/apps/myapp");

    let resp = test::call_service(
        &app,
        form_request("/users", format!("csrf_token={csrf}&email=x%40y.com&role=viewer&password=correct-horse-battery&confirm=correct-horse-battery"))
            .cookie(operator)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn viewer_is_read_only() {
    let (state, _dir) = test_state().await;
    seed_user_with_role(
        &state,
        "viewer@example.com",
        "correct-horse-battery",
        Role::Viewer,
    )
    .await;
    let app = test::init_service(build_app(state)).await;
    let viewer = login(&app, "viewer@example.com", "correct-horse-battery").await;

    for path in ["/", "/activity", "/volumes"] {
        let resp = test::call_service(
            &app,
            test::TestRequest::get()
                .uri(path)
                .cookie(viewer.clone())
                .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    }

    let csrf = csrf_for(&app, "/apps/new", &viewer).await;
    for (path, body) in [
        ("/apps", format!("csrf_token={csrf}&name=myapp")),
        ("/refresh", format!("csrf_token={csrf}")),
        ("/apps/myapp/restart", format!("csrf_token={csrf}")),
        (
            "/volumes/mount",
            format!("csrf_token={csrf}&app=myapp&host_path=%2Ftmp%2Fx&container_path=%2Fdata"),
        ),
    ] {
        let resp = test::call_service(
            &app,
            form_request(path, body).cookie(viewer.clone()).to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "POST {path}");
    }

    let resp = test::call_service(
        &app,
        form_request("/apps/myapp/restart", format!("csrf_token={csrf}"))
            .insert_header(("HX-Request", "true"))
            .cookie(viewer.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "htmx errors stay 200");
    let body = get_body(resp).await;
    assert!(body.contains("permission"), "{body}");

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/users")
            .cookie(viewer)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn last_admin_cannot_demote_themselves() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state.clone())).await;
    let admin = complete_setup(&app).await;

    let csrf = csrf_for(&app, "/users", &admin).await;
    let resp = test::call_service(
        &app,
        form_request("/users/1/role", format!("csrf_token={csrf}&role=viewer"))
            .cookie(admin.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/users")
            .cookie(admin.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("last admin cannot be demoted"));

    let repo = SqliteUsersRepo::new(state.db.clone());
    let user = repo.find_by_id(1).await.expect("find").expect("some");
    assert_eq!(user.role, Role::Admin);
}

#[tokio::test]
async fn admins_can_demote_once_another_admin_exists() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state.clone())).await;
    let admin = complete_setup(&app).await;

    let csrf = csrf_for(&app, "/users", &admin).await;
    let resp = test::call_service(
        &app,
        form_request(
            "/users",
            format!(
                "csrf_token={csrf}&email=second%40example.com&role=admin&password=correct-horse-battery&confirm=correct-horse-battery"
            ),
        )
        .cookie(admin.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = test::call_service(
        &app,
        form_request("/users/1/role", format!("csrf_token={csrf}&role=operator"))
            .cookie(admin)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let repo = SqliteUsersRepo::new(state.db.clone());
    let user = repo.find_by_id(1).await.expect("find").expect("some");
    assert_eq!(user.role, Role::Operator);
}

#[tokio::test]
async fn self_delete_is_rejected() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state.clone())).await;
    let admin = complete_setup(&app).await;

    let csrf = csrf_for(&app, "/users", &admin).await;
    let resp = test::call_service(
        &app,
        form_request("/users/1/delete", format!("csrf_token={csrf}"))
            .cookie(admin.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/users")
            .cookie(admin)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("cannot delete your own account"));

    let repo = SqliteUsersRepo::new(state.db.clone());
    assert_eq!(repo.count().await.expect("count"), 1);
}

#[tokio::test]
async fn deleting_a_user_invalidates_their_session() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    seed_user_with_role(
        &state,
        "victim@example.com",
        "correct-horse-battery",
        Role::Viewer,
    )
    .await;
    let app = test::init_service(build_app(state.clone())).await;

    let victim = login(&app, "victim@example.com", "correct-horse-battery").await;
    let admin = login(&app, "admin@example.com", "correct-horse-battery").await;

    let repo = SqliteUsersRepo::new(state.db.clone());
    let victim_id = repo
        .find_by_email("victim@example.com")
        .await
        .expect("find")
        .expect("some")
        .id;

    let csrf = csrf_for(&app, "/users", &admin).await;
    let resp = test::call_service(
        &app,
        form_request(
            &format!("/users/{victim_id}/delete"),
            format!("csrf_token={csrf}"),
        )
        .cookie(admin)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(victim)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login");
}

#[tokio::test]
async fn password_change_flow() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let admin = complete_setup(&app).await;

    let csrf = csrf_for(&app, "/password", &admin).await;
    let resp = test::call_service(
        &app,
        form_request(
            "/password",
            format!(
                "csrf_token={csrf}&current_password=wrong-password&password=new-password-123&confirm=new-password-123"
            ),
        )
        .cookie(admin.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("Current password is incorrect"));

    let resp = test::call_service(
        &app,
        form_request(
            "/password",
            format!(
                "csrf_token={csrf}&current_password=correct-horse-battery&password=new-password-123&confirm=new-password-123"
            ),
        )
        .cookie(admin.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/password");

    let resp = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/logout")
            .cookie(admin.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    let cookie = login(&app, "admin@example.com", "new-password-123").await;
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/")
            .cookie(cookie)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn admin_nav_partial_is_role_gated() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    seed_user_with_role(
        &state,
        "viewer@example.com",
        "correct-horse-battery",
        Role::Viewer,
    )
    .await;
    let app = test::init_service(build_app(state)).await;

    let admin = login(&app, "admin@example.com", "correct-horse-battery").await;
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/partials/admin-nav")
            .cookie(admin)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(body.contains("/users"));

    let viewer = login(&app, "viewer@example.com", "correct-horse-battery").await;
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/partials/admin-nav")
            .cookie(viewer)
            .to_request(),
    )
    .await;
    let body = get_body(resp).await;
    assert!(!body.contains("/users"));
}

#[tokio::test]
async fn users_page_requires_csrf_on_post() {
    let (state, _dir) = test_state().await;
    let app = test::init_service(build_app(state)).await;
    let admin = complete_setup(&app).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/users",
            "email=x%40y.com&role=viewer&password=correct-horse-battery&confirm=correct-horse-battery"
                .to_owned(),
        )
        .cookie(admin.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = test::call_service(
        &app,
        form_request("/users/1/delete", String::new())
            .cookie(admin)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn session_cookie_is_not_required_for_password_page_but_auth_is() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    let resp =
        test::call_service(&app, test::TestRequest::get().uri("/password").to_request()).await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login");
}

#[tokio::test]
async fn palette_exposes_admin_entries_to_admins_only() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    seed_user_with_role(
        &state,
        "viewer@example.com",
        "correct-horse-battery",
        Role::Viewer,
    )
    .await;
    let app = test::init_service(build_app(state)).await;

    let admin = login(&app, "admin@example.com", "correct-horse-battery").await;
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/palette.json")
            .cookie(admin)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .expect("content type")
        .to_str()
        .expect("utf-8")
        .to_owned();
    assert!(content_type.contains("application/json"), "{content_type}");
    let body = get_body(resp).await;
    assert!(body.contains("/users"), "{body}");
    assert!(body.contains("/settings"), "{body}");
    assert!(body.contains("Dashboard"), "{body}");

    let viewer = login(&app, "viewer@example.com", "correct-horse-battery").await;
    let resp = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/palette.json")
            .cookie(viewer)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(!body.contains("/users"), "{body}");
    assert!(!body.contains("/settings"), "{body}");
}

#[tokio::test]
async fn palette_requires_login() {
    let (state, _dir) = test_state().await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;

    let resp = test::call_service(
        &app,
        test::TestRequest::get().uri("/palette.json").to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(location(&resp), "/login");
}
