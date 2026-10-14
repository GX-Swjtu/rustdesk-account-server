use super::*;
use crate::{
    admin::bounded,
    db::{now, prune},
};
use axum::extract::Path;
use rusqlite::params;

fn identity(value: &Value) -> Result<(String, String)> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && bounded(s, 64));
    let uuid = value
        .get("uuid")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && bounded(s, 128));
    match (id, uuid) {
        (Some(id), Some(uuid)) => Ok((id.into(), uuid.into())),
        _ => Err(error::bad("Invalid device identifiers")),
    }
}
fn known(c: &Connection, id: &str, uuid: &str) -> Result<bool> {
    Ok(c.query_row("SELECT 1 FROM devices d JOIN users u ON d.owner=u.id WHERE d.id=?1 AND d.uuid=?2 AND u.disabled=0",params![id,uuid],|_|Ok(())).optional()?.is_some())
}
pub(super) async fn sysinfo(State(app): State<App>, bytes: Bytes) -> ApiResult<&'static str> {
    let input: Value = parse(&bytes)?;
    let (id, uuid) = identity(&input)?;
    // No per-ID limiter allocation before checking the bounded inventory.
    let limiter = app.limiter.clone();
    app.store
        .run(move |c, _, _| {
            let tx = c.transaction()?;
            if known(&tx, &id, &uuid)? && limiter.allow(&[(format!("sysinfo/{id}"), 4)]) {
                let mut info = json!({"_reported":true});
                for (source, target) in [
                    ("hostname", "device_name"),
                    ("username", "username"),
                    ("os", "os"),
                    ("version", "version"),
                ] {
                    if let Some(v) = input
                        .get(source)
                        .and_then(Value::as_str)
                        .filter(|s| bounded(s, 256))
                    {
                        info[target] = json!(v);
                    }
                }
                tx.execute(
                    "UPDATE devices SET info=?1 WHERE id=?2 AND uuid=?3 AND info<>?1",
                    params![info.to_string(), id, uuid],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    // Unknown IDs are ignored without revealing enrollment. A later registered heartbeat requests sysinfo.
    Ok("SYSINFO_UPDATED")
}
pub(super) async fn heartbeat(State(app): State<App>, bytes: Bytes) -> ApiResult<Json<Value>> {
    let input: Value = parse(&bytes)?;
    let (id, uuid) = identity(&input)?;
    Ok(Json(
        app.store
            .run(move |c, _, _| {
                let tx = c.transaction()?;
                let mut response = json!({});
                if known(&tx, &id, &uuid)? {
                    let info: String =
                        tx.query_row("SELECT info FROM devices WHERE id=?1", [&id], |r| r.get(0))?;
                    if serde_json::from_str::<Value>(&info)?.get("_reported")
                        != Some(&Value::Bool(true))
                    {
                        response["sysinfo"] = json!(true);
                    }
                    tx.execute(
                        "UPDATE devices SET last_seen=?1 WHERE id=?2 AND uuid=?3 AND last_seen<=?4",
                        params![now(), id, uuid, now() - 60],
                    )?;
                }
                tx.commit()?;
                Ok(response)
            })
            .await?,
    ))
}

fn copy_fields(input: &Value, output: &mut Value, fields: &[&str], max: usize) {
    for field in fields {
        if let Some(v) = input.get(*field)
            && (v.is_boolean()
                || v.is_i64()
                || v.is_u64()
                || v.as_str().is_some_and(|s| bounded(s, max)))
        {
            output[*field] = v.clone();
        }
    }
}
pub(super) async fn report(
    State(app): State<App>,
    Path(kind): Path<String>,
    bytes: Bytes,
) -> ApiResult<StatusCode> {
    if kind != "conn" && kind != "file" && kind != "alarm" {
        return Err(ApiError(
            crate::error::Rejected(StatusCode::NOT_FOUND, "Not found").into(),
        ));
    }
    let input: Value = parse(&bytes)?;
    let (id, uuid) = identity(&input)?;
    let mut data = json!({"source":"unverified_client_report"});
    copy_fields(
        &input,
        &mut data,
        &[
            "peer_id",
            "conn_id",
            "session_id",
            "type",
            "typ",
            "action",
            "ip",
            "name",
            "is_file",
            "primary_auth",
            "two_factor",
        ],
        256,
    );
    // Native connection reports encode the remote ID and display name as a tuple.
    if let Some(peer) = input.get("peer").and_then(Value::as_array)
        && peer.len() == 2
        && peer
            .iter()
            .all(|v| v.as_str().is_some_and(|s| bounded(s, 256)))
    {
        data["peer"] = json!(peer);
    }
    copy_fields(&input, &mut data, &["path"], 1024);
    let nested = input
        .get("info")
        .map(|v| {
            if let Some(s) = v.as_str() {
                serde_json::from_str::<Value>(s).unwrap_or(Value::Null)
            } else {
                v.clone()
            }
        })
        .unwrap_or(Value::Null);
    let mut info = json!({});
    copy_fields(
        &nested,
        &mut info,
        &["ip", "name", "num", "is_remote", "conn_type"],
        256,
    );
    if let Some(files) = nested.get("files").and_then(Value::as_array) {
        let filtered: Vec<Value> = files
            .iter()
            .take(10)
            .filter_map(|v| {
                let pair = v.as_array()?;
                let name = pair.first()?.as_str().filter(|s| bounded(s, 512))?;
                let size = pair.get(1)?.as_i64()?;
                Some(json!([name, size]))
            })
            .collect();
        info["files"] = json!(filtered);
    }
    data["info"] = info;
    let nonce = input
        .get("nonce")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && bounded(s, 64))
        .map(str::to_owned);
    let limiter = app.limiter.clone();
    app.store.run(move |c,_,config| {
        let tx=c.transaction()?;
        if known(&tx,&id,&uuid)? && limiter.allow(&[(format!("report/{id}"),30)]) {
            tx.execute("INSERT OR IGNORE INTO client_reports(ts,device,kind,data,nonce) VALUES(?1,?2,?3,?4,?5)",params![now(),id,kind,data.to_string(),nonce])?;
            prune(&tx,"client_reports",config.max_client_report_rows,config.log_retention_days)?;
        }
        tx.commit()?; Ok(())
    }).await?;
    Ok(StatusCode::OK)
}
