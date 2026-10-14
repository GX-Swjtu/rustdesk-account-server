use super::*;
use crate::{
    admin::{bounded, valid_name},
    db::{audit, now},
};
use axum::extract::{Extension, Query};
use rusqlite::{TransactionBehavior, params};
use zeroize::Zeroizing;

#[derive(Deserialize)]
struct Login {
    username: String,
    password: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    uuid: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default, rename = "deviceInfo")]
    device_info: Value,
}

pub(super) async fn login(
    State(app): State<App>,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    bytes: Bytes,
) -> ApiResult<Json<Value>> {
    let request: Login = parse(&bytes)?;
    if !valid_name(&request.username)
        || request.password.len() > 1024
        || (!request.kind.is_empty() && request.kind != "account")
    {
        return Err(error::unauthorized().into());
    }
    if !bounded(&request.id, 64)
        || !bounded(&request.uuid, 128)
        || (!request.id.is_empty() && request.uuid.is_empty())
    {
        return Err(error::bad("Invalid device identifiers").into());
    }
    let config = &app.store.config;
    if !app.limiter.allow(&[
        ("login/global".into(), config.login_global_per_minute),
        (format!("login/ip/{ip}"), config.login_per_ip_per_minute),
        (
            format!("login/account/{}", request.username.to_ascii_lowercase()),
            config.login_per_account_per_minute,
        ),
    ]) {
        return Err(error::busy().into());
    }
    let permit = app
        .password_workers
        .clone()
        .try_acquire_owned()
        .map_err(|_| error::busy())?;
    let name = request.username.clone();
    let record: Option<(String, String, bool)> = app
        .store
        .run(move |c, _, _| {
            Ok(c.query_row(
                "SELECT id,password_hash,disabled FROM users WHERE username=?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
        })
        .await?;
    let encoded = record
        .as_ref()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| (*app.dummy_hash).clone());
    let secret = Zeroizing::new(request.password);
    let check = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        crypto::verify_password(&secret, &encoded)
    })
    .await
    .map_err(|_| anyhow::anyhow!("Password worker stopped"))?;
    let Some((user, verified_hash, false)) = record.filter(|_| check) else {
        app.store
            .run(move |c, _, config| {
                let tx = c.transaction()?;
                audit(&tx, config, "anonymous", "login.failed", &request.username)?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        return Err(error::unauthorized().into());
    };
    let value = app.store.run(move |c,_,config| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Recheck after the expensive hash so reset/disable cannot race session issuance.
        let username: Option<String> = tx.query_row("SELECT username FROM users WHERE id=?1 AND password_hash=?2 AND disabled=0", params![user,verified_hash], |r| r.get(0)).optional()?;
        let username = username.ok_or_else(error::unauthorized)?;
        let mut device: Option<String> = None;
        if !request.id.is_empty() {
            let existing: Option<(String,String)> = tx.query_row("SELECT uuid,owner FROM devices WHERE id=?1", [&request.id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((uuid,owner)) = existing {
                if uuid != request.uuid { return Err(error::conflict("Device ID conflicts with another machine; contact administrator")); }
                if owner != user {
                    tx.execute("UPDATE sessions SET revoked=?1 WHERE device_id=?2 AND revoked IS NULL", params![now(),request.id])?;
                    tx.execute("UPDATE devices SET owner=?1,updated=?2 WHERE id=?3", params![user,now(),request.id])?;
                    audit(&tx,config,&user,"device.transfer_and_revoke",&request.id)?;
                }
            } else {
                let count:i64 = tx.query_row("SELECT COUNT(*) FROM devices", [], |r| r.get(0))?;
                if count >= config.max_devices as i64 { return Err(error::bad("Device limit reached")); }
                let mut info = json!({});
                if let Some(v) = request.device_info.get("name").and_then(Value::as_str).filter(|s| bounded(s,256)) { info["device_name"] = json!(v); }
                if let Some(v) = request.device_info.get("os").and_then(Value::as_str).filter(|s| bounded(s,256)) {
                    info["os"] = json!(match v {"windows"=>"Windows","linux"=>"Linux","macos"=>"Mac OS","android"=>"Android",other=>other});
                }
                tx.execute("INSERT INTO devices(id,uuid,owner,info,updated) VALUES(?1,?2,?3,?4,?5)", params![request.id,request.uuid,user,info.to_string(),now()])?;
                audit(&tx,config,&user,"device.register",&request.id)?;
            }
            device = Some(request.id);
        }
        // Keep revoked session storage bounded too. Audit records preserve revocation history.
        tx.execute("DELETE FROM sessions WHERE revoked IS NOT NULL", [])?;
        let count:i64 = tx.query_row("SELECT COUNT(*) FROM sessions WHERE user_id=?1", [&user], |r| r.get(0))?;
        if count >= config.max_sessions_per_user as i64 { return Err(error::bad("Session limit reached; revoke an existing session")); }
        let bearer = crypto::random_token(); let session = crypto::id();
        tx.execute("INSERT INTO sessions(id,token_hash,user_id,device_id,machine_uuid,created) VALUES(?1,?2,?3,?4,?5,?6)",
            params![session,crypto::token_hash(&bearer),user,device,request.uuid,now()])?;
        audit(&tx,config,&user,"login.success",&session)?;
        tx.commit()?;
        Ok(json!({"access_token":bearer,"type":"access_token","user":user_payload(&Principal{id:user,username,session})}))
    }).await?;
    Ok(Json(value))
}

pub(super) async fn current_user(
    State(app): State<App>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let token = token(&headers)?;
    Ok(Json(
        app.store
            .run(move |c, _, _| Ok(user_payload(&authorize(c, &token)?)))
            .await?,
    ))
}
pub(super) async fn logout(State(app): State<App>, headers: HeaderMap) -> ApiResult<StatusCode> {
    let token = token(&headers)?;
    app.store
        .run(move |c, _, config| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let user = authorize(&tx, &token)?;
            tx.execute(
                "UPDATE sessions SET revoked=?1 WHERE id=?2",
                params![now(), user.session],
            )?;
            audit(&tx, config, &user.id, "session.logout", &user.session)?;
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(StatusCode::OK)
}
pub(super) async fn groups(
    State(app): State<App>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    page.bounds()?;
    let token = token(&headers)?;
    app.store
        .run(move |c, _, _| {
            authorize(c, &token)?;
            Ok(())
        })
        .await?;
    Ok(Json(json!({"total":0,"data":[]})))
}
pub(super) async fn users(
    State(app): State<App>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let (_, offset) = page.bounds()?;
    let token = token(&headers)?;
    Ok(Json(
        app.store
            .run(move |c, _, _| {
                let user = authorize(c, &token)?;
                Ok(json!({"total":1,"data":if offset==0 {vec![user_payload(&user)]} else {vec![]}}))
            })
            .await?,
    ))
}
pub(super) async fn devices(
    State(app): State<App>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let (size, offset) = page.bounds()?;
    let token = token(&headers)?;
    Ok(Json(app.store.run(move |c,_,_| {
        let tx = c.transaction()?; let user = authorize(&tx,&token)?;
        let total:i64 = tx.query_row("SELECT COUNT(*) FROM devices WHERE owner=?1", [&user.id],|r|r.get(0))?;
        let data = {
            let mut s = tx.prepare("SELECT id,info FROM devices WHERE owner=?1 ORDER BY id LIMIT ?2 OFFSET ?3")?;
            let rows = s.query_map(params![user.id,size,offset], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?;
            let mut data = Vec::new();
            for row in rows { let (id,info) = row?; data.push(json!({"id":id,"info":serde_json::from_str::<Value>(&info)?,"status":1,"user":user.id,"user_name":user.username,"device_group_name":"","note":""})); }
            data
        };
        tx.commit()?; Ok(json!({"total":total,"data":data}))
    }).await?))
}
