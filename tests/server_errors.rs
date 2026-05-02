//! Phase 6 integration test: server-side errors classify correctly.
//!
//! Verifies that `Job::execute` returns `Err(Error::Server(ServerError {...}))`
//! when the mock responds with an `Error`, and that the `ServerError`'s
//! SQLSTATE classification predicates (from v0.1's PR #13) correctly
//! identify the error class.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;

#[cfg(feature = "rustls-tls")]
fn server_error_response(sqlstate: &str, message: &str) -> mapepire::ErrorResponse {
    mapepire::ErrorResponse {
        id: "placeholder".into(),
        success: false,
        sqlstate: Some(sqlstate.to_string()),
        sqlcode: Some(-803), // arbitrary; test asserts on sqlstate
        error: Some(message.to_string()),
        job: Some("DAEMON/QUSER/000001".into()),
    }
}

#[cfg(feature = "rustls-tls")]
async fn assert_execute_returns_classified_error<F>(sqlstate: &str, predicate: F)
where
    F: FnOnce(&mapepire::ServerError) -> bool,
{
    use mapepire::Error;

    let job = common::connect_to_mock(common::MockBehavior::ReturnError(server_error_response(
        sqlstate,
        "test error message",
    )))
    .await;

    let result = job.execute("INSERT INTO T VALUES (1)").await;
    match result {
        Err(Error::Server(server_err)) => {
            assert_eq!(
                server_err.sqlstate.as_deref(),
                Some(sqlstate),
                "sqlstate round-trip"
            );
            assert!(
                predicate(&server_err),
                "ServerError predicate should classify sqlstate {sqlstate} correctly"
            );
        }
        other => panic!("expected Err(Error::Server(...)), got {other:?}"),
    }
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_constraint_violation_classified() {
    assert_execute_returns_classified_error(
        "23505",
        mapepire::ServerError::is_constraint_violation,
    )
    .await;
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_authorization_classified() {
    assert_execute_returns_classified_error("42501", mapepire::ServerError::is_authorization).await;
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_transient_classified() {
    assert_execute_returns_classified_error("08001", mapepire::ServerError::is_transient).await;
}
