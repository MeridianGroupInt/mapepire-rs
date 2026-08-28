//! Cover `Executor::{execute_opts, execute_with_opts}` and the inherent
//! `Pool` / `Reserved` opts methods. Existing SQL and pool tests call
//! `execute` / `execute_with`, which resolve to inherent methods and leave
//! the trait impls and opts wrappers at 0% patch coverage.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use mapepire::{ExecuteOptions, Executor};
#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;
#[cfg(feature = "rustls-tls")]
use serde_json::json;

#[cfg(feature = "rustls-tls")]
fn page() -> mapepire::QueryResult {
    use mapepire::{Column, QueryMetaData, QueryResult};
    use serde_json::{Map, Value};

    let mut row: Map<String, Value> = Map::new();
    row.insert("1".into(), json!(1));
    QueryResult {
        id: "placeholder".into(),
        success: true,
        has_results: true,
        update_count: -1,
        metadata: QueryMetaData {
            column_count: 1,
            columns: vec![Column {
                name: "1".into(),
                label: Some("1".into()),
                type_name: Some("INTEGER".into()),
                display_size: Some(11),
                precision: Some(10),
                scale: Some(0),
            }],
            job: None,
            parameters: vec![],
        },
        data: vec![row],
        cont_id: None,
        is_done: true,
        execution_time: 1.0,
        error: None,
        sqlcode: None,
        sqlstate: None,
        parameter_count: None,
        output_parms: vec![],
    }
}

#[cfg(feature = "rustls-tls")]
fn opts_ten() -> ExecuteOptions {
    ExecuteOptions {
        rows: Some(10),
        terse: false,
    }
}

/// Drive `execute_opts` through the trait (not `Job::execute_opts`).
#[cfg(feature = "rustls-tls")]
async fn trait_execute_opts<E: Executor>(exe: &E, sql: &str) -> mapepire::Rows {
    exe.execute_opts(sql, opts_ten())
        .await
        .expect("Executor::execute_opts")
}

/// Drive `execute_with_opts` through `&dyn Executor`.
#[cfg(feature = "rustls-tls")]
async fn dyn_execute_with_opts(
    exe: &dyn Executor,
    sql: &str,
    params: &[serde_json::Value],
) -> mapepire::Rows {
    exe.execute_with_opts(sql, params, opts_ten())
        .await
        .expect("dyn Executor::execute_with_opts")
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_job_executor_opts_and_execute_with_opts() {
    use mapepire::protocol::Request;

    let (job, recorder) = common::connect_to_mock_with_recorder(vec![page(), page()]).await;

    drop(trait_execute_opts(&job, "SELECT 1 FROM SYSIBM.SYSDUMMY1").await);
    drop(dyn_execute_with_opts(&job, "VALUES (CAST(? AS INTEGER))", &[json!(7)]).await);

    let observed = recorder.lock().expect("recorder mutex").clone();
    let sql: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::Sql { .. }))
        .collect();
    assert_eq!(sql.len(), 1, "full trace: {observed:?}");
    match sql[0] {
        Request::Sql { rows, terse, .. } => {
            assert_eq!(*rows, Some(10));
            assert_eq!(*terse, None);
        }
        other => panic!("expected Sql, got {other:?}"),
    }
    let bound: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::PrepareSqlExecute { .. }))
        .collect();
    assert_eq!(bound.len(), 1, "full trace: {observed:?}");
    match bound[0] {
        Request::PrepareSqlExecute {
            rows, parameters, ..
        } => {
            assert_eq!(*rows, Some(10));
            assert_eq!(parameters.as_ref(), Some(&vec![vec![json!(7)]]));
        }
        other => panic!("expected PrepareSqlExecute, got {other:?}"),
    }
}

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_pool_and_reserved_execute_opts() {
    use common::spawn_mock_pool;

    let (pool, _mock) = spawn_mock_pool(2).await;
    let sql = "SELECT 1 FROM SYSIBM.SYSDUMMY1";
    let bound = "VALUES (CAST(? AS INTEGER))";
    let params = [json!(7)];
    let opts = opts_ten();

    drop(
        Box::pin(pool.execute_opts(sql, opts))
            .await
            .expect("Pool::execute_opts"),
    );
    drop(
        Box::pin(pool.execute_with_opts(bound, &params, opts))
            .await
            .expect("Pool::execute_with_opts"),
    );
    drop(trait_execute_opts(&pool, sql).await);
    drop(dyn_execute_with_opts(&pool, bound, &params).await);

    let conn = Box::pin(pool.acquire()).await.expect("acquire");
    drop(
        Box::pin(conn.execute_opts(sql, opts))
            .await
            .expect("Reserved::execute_opts"),
    );
    drop(
        Box::pin(conn.execute_with_opts(bound, &params, opts))
            .await
            .expect("Reserved::execute_with_opts"),
    );
    drop(trait_execute_opts(&conn, sql).await);
    drop(dyn_execute_with_opts(&conn, bound, &params).await);
}

/// Pool `default_page_size` is the wire `rows` when opts omit it;
/// `execute_opts(rows: Some(10))` still sends 10.
#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_pool_default_page_size_on_execute() {
    use common::spawn_mock_pool_with_recorder;
    use mapepire::protocol::Request;
    use mapepire::{ExecuteOptions, Pool};

    let (server, recorder) = spawn_mock_pool_with_recorder(vec![page(), page()]);
    let pool = Box::pin(
        Pool::builder(server)
            .max_size(1)
            .default_page_size(50)
            .build(),
    )
    .await
    .expect("pool builds");

    drop(
        Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
            .await
            .expect("pool.execute"),
    );
    drop(
        Box::pin(pool.execute_opts(
            "SELECT 1 FROM SYSIBM.SYSDUMMY1",
            ExecuteOptions {
                rows: Some(10),
                terse: false,
            },
        ))
        .await
        .expect("pool.execute_opts"),
    );

    let observed = recorder.lock().expect("recorder mutex").clone();
    let sql: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::Sql { .. }))
        .collect();
    assert_eq!(sql.len(), 2, "full trace: {observed:?}");
    match sql[0] {
        Request::Sql { rows, .. } => assert_eq!(*rows, Some(50), "builder default_page_size"),
        other => panic!("expected Sql, got {other:?}"),
    }
    match sql[1] {
        Request::Sql { rows, .. } => assert_eq!(*rows, Some(10), "explicit execute_opts rows"),
        other => panic!("expected Sql, got {other:?}"),
    }
}
