//! Snapshot tests pinning the on-the-wire JSON shape of every request and
//! response variant. Any accidental field rename, casing change, or default
//! shift will break these — review the diff carefully on update.
//!
//! Serialize snapshots pin what `Response`'s internally tagged `Serialize`
//! emits (`{"type":"connected",...}`). Live daemon frames are untagged
//! `{id, success, ...}` objects; those shapes are pinned by the
//! `snapshot_decode_live_*` tests.

use mapepire::protocol::request::Request;
use mapepire::protocol::response::{
    ClMessage, Column, ErrorResponse, ParameterDetail, ParameterResult, QueryMetaData, QueryResult,
    Response,
};

#[test]
fn snapshot_request_connect() {
    let r = Request::Connect {
        id: "test".into(),
        technique: "tcp".into(),
        application: "mapepire-rs".into(),
        props: None,
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
        terse: None,
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
        terse: None,
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_sql_terse() {
    let r = Request::Sql {
        id: "test".into(),
        sql: "SELECT EMPNO, LASTNAME FROM EMPLOYEE".into(),
        rows: Some(5),
        parameters: None,
        terse: Some(true),
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_prepare_sql() {
    let r = Request::PrepareSql {
        id: "test".into(),
        sql: "SELECT * FROM T WHERE ID=?".into(),
        terse: None,
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
        terse: None,
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_execute() {
    let r = Request::Execute {
        id: "test".into(),
        cont_id: "stmt-7".into(),
        parameters: Some(vec![serde_json::json!("hello")]),
        rows: None,
        terse: None,
    };
    insta::assert_json_snapshot!(r);
}

#[test]
fn snapshot_request_prepare_sql_execute_single() {
    let r = Request::PrepareSqlExecute {
        id: "test".into(),
        sql: "VALUES (CAST(? AS INTEGER))".into(),
        parameters: Some(vec![vec![serde_json::json!(7)]]),
        rows: Some(100),
        terse: None,
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
        terse: None,
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
            terse: None,
        }
    );
}

#[test]
fn set_config_trace_off() {
    // Pins the typical shape produced by `Job::set_trace(TraceLevel::Off)`:
    // `tracelevel: "OFF"` + empty-`tracedest` (default destination).
    let r = Request::SetConfig {
        id: "1".into(),
        tracelevel: "OFF".into(),
        tracedest: String::new(),
    };
    insta::assert_json_snapshot!(r);
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
            job: None,
            parameters: vec![],
        },
        data: vec![{
            let mut m = serde_json::Map::new();
            m.insert("ID".into(), serde_json::json!(42));
            m
        }],
        execution_time: 1.23,
        error: None,
        sqlcode: None,
        sqlstate: None,
        parameter_count: None,
        output_parms: vec![],
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
        error: None,
        sqlcode: None,
        sqlstate: None,
        parameter_count: None,
        output_parms: vec![],
    };
    insta::assert_json_snapshot!(Response::QueryResult(q));
}

#[test]
fn snapshot_response_query_result_call_out() {
    let q = QueryResult {
        id: "call1".into(),
        success: true,
        has_results: false,
        update_count: 0,
        cont_id: None,
        is_done: true,
        metadata: QueryMetaData {
            column_count: 0,
            columns: vec![],
            job: None,
            parameters: vec![
                ParameterDetail {
                    type_name: "INTEGER".into(),
                    mode: "IN".into(),
                    precision: 10,
                    scale: Some(0),
                    name: "P1".into(),
                },
                ParameterDetail {
                    type_name: "INTEGER".into(),
                    mode: "INOUT".into(),
                    precision: 10,
                    scale: Some(0),
                    name: "P2".into(),
                },
                ParameterDetail {
                    type_name: "INTEGER".into(),
                    mode: "OUT".into(),
                    precision: 10,
                    scale: Some(0),
                    name: "P3".into(),
                },
            ],
        },
        data: vec![],
        execution_time: 5.0,
        error: None,
        sqlcode: None,
        sqlstate: None,
        parameter_count: Some(3),
        output_parms: vec![
            ParameterResult {
                index: 1,
                type_name: "INTEGER".into(),
                precision: 10,
                scale: Some(0),
                name: "P1".into(),
                ccsid: None,
                value: None,
            },
            ParameterResult {
                index: 2,
                type_name: "INTEGER".into(),
                precision: 10,
                scale: Some(0),
                name: "P2".into(),
                ccsid: None,
                value: Some(serde_json::json!(0)),
            },
            ParameterResult {
                index: 3,
                type_name: "INTEGER".into(),
                precision: 10,
                scale: Some(0),
                name: "P3".into(),
                ccsid: None,
                value: Some(serde_json::json!(10)),
            },
        ],
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
    // Tagged serialize output — not the live daemon frame. Decode of the
    // untagged connect body is `snapshot_decode_live_connect`.
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

#[test]
fn snapshot_decode_live_connect() {
    let json = serde_json::json!({
        "id": "test",
        "job": "nnnnnn/QUSER/QZDASOINIT",
        "success": true,
        "execution_time": 1.0
    });
    let r: Response = serde_json::from_value(json).unwrap();
    insta::assert_debug_snapshot!(r);
}

#[test]
fn snapshot_decode_live_terse_query_result() {
    let json = serde_json::json!({
        "id": "q1",
        "has_results": true,
        "update_count": -1,
        "metadata": {
            "column_count": 1,
            "columns": [{
                "name": "1",
                "type": "INTEGER",
                "display_size": 11,
                "label": "1",
                "precision": 10,
                "scale": 0
            }]
        },
        "data": [[7]],
        "is_done": true,
        "success": true,
        "execution_time": 1
    });
    let r: Response = serde_json::from_value(json).unwrap();
    insta::assert_debug_snapshot!(r);
}

#[test]
fn snapshot_decode_live_query_result() {
    let json = serde_json::json!({
        "id": "query3",
        "has_results": true,
        "update_count": -1,
        "metadata": {
            "column_count": 1,
            "job": "123456/QUSER/QZDASOINIT",
            "columns": [{
                "name": "READY",
                "type": "INTEGER",
                "display_size": 11,
                "label": "READY",
                "precision": 10,
                "scale": 0
            }]
        },
        "data": [{"READY": 1}],
        "is_done": true,
        "success": true,
        "execution_time": 215
    });
    let r: Response = serde_json::from_value(json).unwrap();
    insta::assert_debug_snapshot!(r);
}

#[test]
fn snapshot_decode_live_call_output_parms() {
    let json = serde_json::json!({
        "id": "call1",
        "success": true,
        "has_results": false,
        "update_count": 0,
        "is_done": true,
        "parameter_count": 3,
        "metadata": {
            "column_count": 0,
            "columns": [],
            "parameters": [
                {"type": "INTEGER", "mode": "IN", "precision": 10, "scale": 0, "name": "P1"},
                {"type": "INTEGER", "mode": "INOUT", "precision": 10, "scale": 0, "name": "P2"},
                {"type": "INTEGER", "mode": "OUT", "precision": 10, "scale": 0, "name": "P3"}
            ]
        },
        "data": [],
        "output_parms": [
            {"index": 1, "type": "INTEGER", "precision": 10, "scale": 0, "name": "P1"},
            {"index": 2, "type": "INTEGER", "precision": 10, "scale": 0, "name": "P2", "value": 0},
            {"index": 3, "type": "INTEGER", "precision": 10, "scale": 0, "name": "P3", "value": 10}
        ],
        "execution_time": 5
    });
    let r: Response = serde_json::from_value(json).unwrap();
    insta::assert_debug_snapshot!(r);
}

#[test]
fn snapshot_decode_live_pong() {
    let json = serde_json::json!({
        "id": "p1",
        "success": true
    });
    let r: Response = serde_json::from_value(json).unwrap();
    insta::assert_debug_snapshot!(r);
}

#[test]
fn snapshot_decode_live_prepare() {
    let json = serde_json::json!({
        "id": "pr1",
        "success": true,
        "cont_id": "stmt-7",
        "execution_time": 1.5,
        "is_done": true
    });
    let r: Response = serde_json::from_value(json).unwrap();
    insta::assert_debug_snapshot!(r);
}

#[test]
fn snapshot_decode_live_error() {
    let json = serde_json::json!({
        "id": "x",
        "success": false,
        "error": "nope",
        "sql_rc": -803,
        "sql_state": "23505"
    });
    let r: Response = serde_json::from_value(json).unwrap();
    insta::assert_debug_snapshot!(r);
}

#[test]
fn snapshot_decode_live_cl_success() {
    let json = serde_json::json!({
        "id": "cl1",
        "success": true,
        "has_results": true,
        "is_done": true,
        "data": [{
            "MESSAGE_ID": "CPC2102",
            "SEVERITY": "0",
            "MESSAGE_TIMESTAMP": "2026-08-27-12.00.00.000000",
            "FROM_LIBRARY": "QSYS",
            "FROM_PROGRAM": "QCAEXEC",
            "MESSAGE_TYPE": "COMPLETION",
            "MESSAGE_TEXT": "Library QGPL displayed.",
            "MESSAGE_SECOND_LEVEL_TEXT": ""
        }]
    });
    let r: Response = serde_json::from_value(json).unwrap();
    insta::assert_debug_snapshot!(r);
}

#[test]
fn snapshot_decode_live_cl_failure() {
    let json = serde_json::json!({
        "id": "cl2",
        "success": false,
        "is_done": true,
        "error": "[CPF0006] Errors occurred in command.",
        "sql_rc": -443,
        "sql_state": "38501",
        "data": [{
            "MESSAGE_ID": "CPF0006",
            "SEVERITY": 40,
            "MESSAGE_TIMESTAMP": "2026-08-27-12.00.00.000000",
            "FROM_LIBRARY": "QSYS",
            "FROM_PROGRAM": "QCAEXEC",
            "MESSAGE_TYPE": "ESCAPE",
            "MESSAGE_TEXT": "[CPF0006] Errors occurred in command.",
            "MESSAGE_SECOND_LEVEL_TEXT": "Cause . . . . :   Errors occurred."
        }]
    });
    let r: Response = serde_json::from_value(json).unwrap();
    insta::assert_debug_snapshot!(r);
}
