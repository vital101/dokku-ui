mod common;

use actix_web::http::StatusCode;
use actix_web::test;

use common::{
    extract_csrf, form_request, get_body, location, login, seed_user, seed_user_with_role,
    test_state_with_shared_client,
};

use dokku_ui::auth::rbac::Role;
use dokku_ui::dokku::{DokkuOutput, MockClient};
use dokku_ui::domain::command::DokkuCommand;
use dokku_ui::web::build_app;

const SSH_KEYS: &str = include_str!("fixtures/ssh_keys_list.txt");
const PUBLIC_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILnLJJXqj2pzpcitocD+onmzABt0V33gxiRh1K0IS01D jack@laptop";

fn seeded_client() -> MockClient {
    MockClient::new()
        .stub(DokkuCommand::SshKeysList, Ok(DokkuOutput::ok(SSH_KEYS)))
        .stub(
            DokkuCommand::SshKeysAdd {
                name: "new-key".into(),
            },
            Ok(DokkuOutput::ok("-----> Importing SSH key...\n")),
        )
        .stub(
            DokkuCommand::SshKeysRemove {
                name: "deploy@ci".into(),
            },
            Ok(DokkuOutput::ok("-----> Removing SSH key...\n")),
        )
}

async fn keys_csrf<B, E>(
    app: &impl actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = E,
    >,
    cookie: &actix_web::cookie::Cookie<'static>,
) -> String
where
    B: actix_web::body::MessageBody,
    E: std::fmt::Debug,
{
    let resp = test::call_service(
        app,
        test::TestRequest::get()
            .uri("/keys")
            .cookie(cookie.clone())
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    extract_csrf(&get_body(resp).await)
}

#[tokio::test]
async fn admin_lists_keys_and_viewers_are_forbidden() {
    let (state, _client, _dir) = test_state_with_shared_client(seeded_client()).await;
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
            .uri("/keys")
            .cookie(admin)
            .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = get_body(resp).await;
    assert!(body.contains("jack@laptop"), "{body}");
    assert!(body.contains("deploy@ci"), "{body}");
    assert!(body.contains("SHA256:AbCdEf"), "{body}");
    assert!(
        body.contains("no-agent-forwarding"),
        "the authorized_keys options render: {body}"
    );

    let viewer = login(&app, "viewer@example.com", "correct-horse-battery").await;
    for path in ["/keys", "/keys/remove"] {
        let request = if path == "/keys/remove" {
            form_request(path, String::new())
        } else {
            test::TestRequest::get().uri(path)
        };
        let resp = test::call_service(&app, request.cookie(viewer.clone()).to_request()).await;
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn admin_adds_a_key_through_stdin_and_the_trail_records_it() {
    let (state, client, _dir) = test_state_with_shared_client(seeded_client()).await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state.clone())).await;
    let cookie = login(&app, "admin@example.com", "correct-horse-battery").await;
    let csrf = keys_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/keys",
            format!(
                "csrf_token={csrf}&name=new-key&key={}",
                urlencode(PUBLIC_KEY)
            ),
        )
        .cookie(cookie.clone())
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/keys");

    let stdin_calls = client.stdin_calls();
    assert_eq!(stdin_calls.len(), 1, "{stdin_calls:?}");
    assert_eq!(
        stdin_calls[0].0,
        DokkuCommand::SshKeysAdd {
            name: "new-key".into()
        }
    );
    assert_eq!(stdin_calls[0].1, format!("{PUBLIC_KEY}\n"));

    let runs = state
        .action_runs
        .list_for_actor_email("admin@example.com", 10)
        .await
        .expect("runs");
    assert!(
        runs.iter()
            .any(|run| run.operation == "ssh-key.add" && run.subject == "new-key"),
        "{runs:?}"
    );
}

#[tokio::test]
async fn invalid_keys_never_reach_the_host() {
    let (state, client, _dir) = test_state_with_shared_client(seeded_client()).await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state)).await;
    let cookie = login(&app, "admin@example.com", "correct-horse-battery").await;
    let csrf = keys_csrf(&app, &cookie).await;

    for (name, key) in [("new-key", "not-a-key"), ("my key", PUBLIC_KEY)] {
        let resp = test::call_service(
            &app,
            form_request(
                "/keys",
                format!(
                    "csrf_token={csrf}&name={}&key={}",
                    urlencode(name),
                    urlencode(key)
                ),
            )
            .cookie(cookie.clone())
            .to_request(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }
    assert!(
        client.stdin_calls().is_empty(),
        "nothing was piped over SSH"
    );
}

#[tokio::test]
async fn admin_removes_a_key_and_the_trail_records_it() {
    let (state, client, _dir) = test_state_with_shared_client(seeded_client()).await;
    seed_user(&state, "admin@example.com", "correct-horse-battery").await;
    let app = test::init_service(build_app(state.clone())).await;
    let cookie = login(&app, "admin@example.com", "correct-horse-battery").await;
    let csrf = keys_csrf(&app, &cookie).await;

    let resp = test::call_service(
        &app,
        form_request(
            "/keys/remove",
            format!("csrf_token={csrf}&name=deploy%40ci"),
        )
        .cookie(cookie)
        .to_request(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resp), "/keys");
    assert!(
        client.calls().contains(&DokkuCommand::SshKeysRemove {
            name: "deploy@ci".into(),
        }),
        "the remove ran"
    );

    let runs = state
        .action_runs
        .list_for_actor_email("admin@example.com", 10)
        .await
        .expect("runs");
    assert!(
        runs.iter()
            .any(|run| run.operation == "ssh-key.remove" && run.subject == "deploy@ci"),
        "{runs:?}"
    );
}

fn urlencode(input: &str) -> String {
    input
        .bytes()
        .flat_map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            other => format!("%{other:02X}").chars().collect(),
        })
        .collect()
}
