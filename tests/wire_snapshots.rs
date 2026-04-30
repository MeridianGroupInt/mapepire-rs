//! Snapshot tests pinning the on-the-wire JSON shape of every request and
//! response variant. Any accidental field rename, casing change, or default
//! shift will break these — review the diff carefully on update.

use mapepire::protocol::request::Request;
use mapepire::protocol::response::{
    ClMessage, Column, ErrorResponse, QueryMetaData, QueryResult, Response,
};

#[test]
fn snapshot_request_connect() {
    let r = Request::Connect {
        id: "test".into(),
        user: "DCURTIS".into(),
        password: "hunter2".into(),
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_sql_minimal() {
    let r = Request::Sql {
        id: "test".into(),
        sql: "SELECT 1 FROM SYSIBM.SYSDUMMY1".into(),
        rows: None,
        parameters: None,
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_sql_with_params_and_rows() {
    let r = Request::Sql {
        id: "test".into(),
        sql: "SELECT * FROM T WHERE ID=?".into(),
        rows: Some(50),
        parameters: Some(vec![serde_json::json!(42)]),
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_prepare_sql() {
    let r = Request::PrepareSql {
        id: "test".into(),
        sql: "SELECT * FROM T WHERE ID=?".into(),
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_prepare_sql_execute_batched() {
    let r = Request::PrepareSqlExecute {
        id: "test".into(),
        sql: "INSERT INTO T VALUES(?,?)".into(),
        parameters: Some(vec![
            vec![serde_json::json!(1), serde_json::json!("a")],
            vec![serde_json::json!(2), serde_json::json!("b")],
        ]),
        rows: None,
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_execute() {
    let r = Request::Execute {
        id: "test".into(),
        cont_id: "stmt-7".into(),
        parameters: Some(vec![serde_json::json!("hello")]),
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_sqlmore_sqlclose() {
    insta::assert_json_snapshot!(
        "sqlmore",
        Request::SqlMore {
            id: "test".into(),
            cont_id: "cur-1".into(),
            rows: 100,
        }
    );
    insta::assert_json_snapshot!(
        "sqlclose",
        Request::SqlClose {
            id: "test".into(),
            cont_id: "cur-1".into(),
        }
    );
}

#[test]
fn snapshot_request_cl() {
    let r = Request::Cl {
        id: "test".into(),
        cmd: "WRKACTJOB".into(),
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_metadata_and_diagnostics() {
    insta::assert_json_snapshot!("ping", Request::Ping { id: "test".into() });
    insta::assert_json_snapshot!("exit", Request::Exit { id: "test".into() });
    insta::assert_json_snapshot!("getversion", Request::GetVersion { id: "test".into() });
    insta::assert_json_snapshot!("getdbjob", Request::GetDbJob { id: "test".into() });
    insta::assert_json_snapshot!("gettracedata", Request::GetTraceData { id: "test".into() });
    insta::assert_json_snapshot!(
        "setconfig",
        Request::SetConfig {
            id: "test".into(),
            tracelevel: "DATASTREAM".into(),
            tracedest: "FILE".into(),
        }
    );
    insta::assert_json_snapshot!(
        "dove",
        Request::Dove {
            id: "test".into(),
            sql: "SELECT 1 FROM SYSIBM.SYSDUMMY1".into(),
        }
    );
}

#[test]
fn snapshot_response_query_result_select() {
    let q = QueryResult {
        id: "test".into(),
        success: true,
        has_results: true,
        update_count: -1,
        cont_id: Some("cur-1".into()),
        is_done: false,
        metadata: QueryMetaData {
            column_count: 1,
            columns: vec![Column {
                name: "ID".into(),
                label: None,
                type_name: Some("INTEGER".into()),
                display_size: Some(11),
                precision: Some(10),
                scale: Some(0),
            }],
        },
        data: vec![{
            let mut m = serde_json::Map::new();
            m.insert("ID".into(), serde_json::json!(42));
            m
        }],
        execution_time: 1.23,
    };
    insta::assert_json_snapshot!(Response::QueryResult(q));
}

#[test]
fn snapshot_response_query_result_dml() {
    let q = QueryResult {
        id: "test".into(),
        success: true,
        has_results: false,
        update_count: 3,
        cont_id: None,
        is_done: true,
        metadata: QueryMetaData::default(),
        data: vec![],
        execution_time: 0.5,
    };
    insta::assert_json_snapshot!(Response::QueryResult(q));
}

#[test]
fn snapshot_response_error() {
    let r = Response::Error(ErrorResponse {
        id: "test".into(),
        success: false,
        sqlstate: Some("23505".into()),
        sqlcode: Some(-803),
        error: Some("duplicate key".into()),
        job: Some("QZDASOINIT/QUSER/123456".into()),
    });
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_response_cl_result() {
    let r = Response::ClResult {
        id: "test".into(),
        success: true,
        messages: vec![ClMessage {
            id: Some("CPF1234".into()),
            kind: Some("INFO".into()),
            text: Some("ok".into()),
        }],
    };
    insta::assert_json_snapshot!(r);
}

// The remaining response variants below are pinned at their current
// snake_case auto-rename shape. The Mapepire daemon may use bare-form
// tags (e.g., `sqlclosed`, `dbjob`, `configset`, `tracedata`); when
// integration tests against a live daemon — or v0.2 transport work —
// surfaces the actual tags, the per-variant `#[serde(rename)]` overrides
// land in `src/protocol/response.rs` and these snapshots get updated.
// The .snap diff is what tells you what changed; pinning the current
// shape now makes that diff loud and reviewable.

#[test]
fn snapshot_response_connected() {
    let r = Response::Connected {
        id: "test".into(),
        version: "2.3.5".into(),
        job: "QZDASOINIT/QUSER/123456".into(),
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_response_pong_exited() {
    insta::assert_json_snapshot!("pong", Response::Pong { id: "test".into() });
    insta::assert_json_snapshot!("exited", Response::Exited { id: "test".into() });
}

#[test]
fn snapshot_response_prepared_statement() {
    let r = Response::PreparedStatement {
        id: "test".into(),
        success: true,
        cont_id: "stmt-7".into(),
        execution_time: 0.3,
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_response_sql_closed() {
    let r = Response::SqlClosed {
        id: "test".into(),
        success: true,
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_response_version() {
    let r = Response::Version {
        id: "test".into(),
        success: true,
        version: "2.3.5".into(),
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_response_db_job() {
    let r = Response::DbJob {
        id: "test".into(),
        success: true,
        job: "QZDASOINIT/QUSER/123456".into(),
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_response_config_set() {
    let r = Response::ConfigSet {
        id: "test".into(),
        success: true,
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_response_trace_data() {
    let r = Response::TraceData {
        id: "test".into(),
        success: true,
        tracedata: "+++ trace start +++\nrow 1\nrow 2".into(),
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_response_dove_result() {
    let r = Response::DoveResult {
        id: "test".into(),
        success: true,
        result: serde_json::json!({
            "operator": "TableScan",
            "table": "ORDERS",
            "estimated_rows": 1000
        }),
    };
    insta::assert_json_snapshot!(r);
}
