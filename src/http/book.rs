use super::*;
use crate::{admin::bounded, config::Config, crypto::Secrets, db::audit};
use axum::extract::{Path, Query};
use rusqlite::{Transaction, TransactionBehavior, params};
use serde_json::Map;

// Returns whether this is a shared book. Missing/inaccessible IDs have the same result.
fn access(c: &Connection, user: &str, book: &str, write: bool) -> Result<bool> {
    let row: Option<(bool,i64)> = c.query_row("SELECT b.shared,CASE WHEN b.owner=?1 THEN 3 ELSE COALESCE(g.rule,0) END
        FROM books b LEFT JOIN grants g ON g.book=b.id AND g.user_id=?1 AND b.shared=1 WHERE b.id=?2",
        params![user,book], |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    match row {
        Some((shared, rule)) if rule >= if write { 2 } else { 1 } => Ok(shared),
        _ => Err(error::denied()),
    }
}

pub(super) async fn personal(State(app): State<App>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let token = token(&headers)?;
    Ok(Json(
        app.store
            .run(move |c, _, _| {
                let user = authorize(c, &token)?;
                let guid: String = c.query_row(
                    "SELECT id FROM books WHERE owner=?1 AND shared=0",
                    [user.id],
                    |r| r.get(0),
                )?;
                Ok(json!({"guid":guid}))
            })
            .await?,
    ))
}
pub(super) async fn settings(State(app): State<App>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    let token = token(&headers)?;
    Ok(Json(
        app.store
            .run(move |c, _, config| {
                authorize(c, &token)?;
                Ok(json!({"max_peer_one_ab":config.max_peers_per_book}))
            })
            .await?,
    ))
}
pub(super) async fn profiles(
    State(app): State<App>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let token = token(&headers)?;
    let (size, offset) = page.bounds()?;
    Ok(Json(app.store.run(move |c,secrets,_| {
        let tx = c.transaction()?; let user = authorize(&tx,&token)?;
        let total:i64 = tx.query_row("SELECT COUNT(*) FROM books b WHERE b.shared=1 AND (b.owner=?1 OR EXISTS(SELECT 1 FROM grants g WHERE g.book=b.id AND g.user_id=?1))",[&user.id],|r|r.get(0))?;
        let data = {
            let mut s = tx.prepare("SELECT b.id,b.name,u.username,b.note,b.password,CASE WHEN b.owner=?1 THEN 3 ELSE g.rule END
                FROM books b JOIN users u ON b.owner=u.id LEFT JOIN grants g ON g.book=b.id AND g.user_id=?1
                WHERE b.shared=1 AND (b.owner=?1 OR g.rule IS NOT NULL) ORDER BY b.id LIMIT ?2 OFFSET ?3")?;
            let rows = s.query_map(params![user.id,size,offset], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,Option<Vec<u8>>>(4)?,r.get::<_,i64>(5)?)))?;
            let mut data = Vec::new();
            for row in rows {
                let (id,name,owner,note,password,rule) = row?;
                let password = password.map(|p|secrets.open(&format!("book/{id}/password"),&p)).transpose()?.unwrap_or_default();
                data.push(json!({"guid":id,"name":name,"owner":owner,"note":note,"rule":rule,"info":{"password":password}}));
            }; data
        };
        tx.commit()?; Ok(json!({"total":total,"data":data}))
    }).await?))
}
pub(super) async fn peers(
    State(app): State<App>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let token = token(&headers)?;
    let (size, offset) = page.bounds()?;
    Ok(Json(app.store.run(move |c,secrets,_| {
        let tx = c.transaction()?; let user = authorize(&tx,&token)?;
        let shared = access(&tx,&user.id,&page.ab,false)?;
        let total:i64 = tx.query_row("SELECT COUNT(*) FROM peers WHERE book=?1",[&page.ab],|r|r.get(0))?;
        let data = {
            let mut s = tx.prepare("SELECT id,data,hash,password FROM peers WHERE book=?1 ORDER BY id LIMIT ?2 OFFSET ?3")?;
            let rows = s.query_map(params![page.ab,size,offset],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,Option<Vec<u8>>>(2)?,r.get::<_,Option<Vec<u8>>>(3)?)))?;
            let mut data=Vec::new();
            for row in rows {
                let (id,raw,hash,password) = row?;
                let mut value:Value = serde_json::from_str(&raw)?;
                value["id"]=json!(id);
                value["hash"]=json!(if shared {String::new()} else {decrypt(secrets,&page.ab,&id,"hash",hash)?});
                value["password"]=json!(if shared {decrypt(secrets,&page.ab,&id,"password",password)?} else {String::new()});
                let mut tags = tx.prepare("SELECT tag FROM peer_tags WHERE book=?1 AND peer=?2 ORDER BY tag")?;
                let names=tags.query_map(params![page.ab,id],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
                value["tags"]=json!(names); data.push(value);
            }; data
        };
        tx.commit()?; Ok(json!({"total":total,"data":data}))
    }).await?))
}
fn decrypt(
    secrets: &Secrets,
    book: &str,
    id: &str,
    field: &str,
    value: Option<Vec<u8>>,
) -> Result<String> {
    value
        .map(|v| secrets.open(&format!("peer/{book}/{id}/{field}"), &v))
        .transpose()
        .map(|v| v.unwrap_or_default())
}

pub(super) async fn tags(
    State(app): State<App>,
    headers: HeaderMap,
    Path(book): Path<String>,
) -> ApiResult<Json<Value>> {
    let token = token(&headers)?;
    Ok(Json(
        app.store
            .run(move |c, _, _| {
                let tx = c.transaction()?;
                let user = authorize(&tx, &token)?;
                access(&tx, &user.id, &book, false)?;
                let data = {
                    let mut s =
                        tx.prepare("SELECT name,color FROM tags WHERE book=?1 ORDER BY name")?;
                    s.query_map([book], |r| {
                        Ok(json!({"name":r.get::<_,String>(0)?,"color":r.get::<_,u32>(1)?}))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
                };
                tx.commit()?;
                Ok(json!(data))
            })
            .await?,
    ))
}

pub(super) async fn add_peer(
    State(app): State<App>,
    headers: HeaderMap,
    Path(book): Path<String>,
    bytes: Bytes,
) -> ApiResult<StatusCode> {
    edit_peer(app, headers, book, bytes, true).await
}
pub(super) async fn update_peer(
    State(app): State<App>,
    headers: HeaderMap,
    Path(book): Path<String>,
    bytes: Bytes,
) -> ApiResult<StatusCode> {
    edit_peer(app, headers, book, bytes, false).await
}
fn string<'a>(map: &'a Map<String, Value>, key: &str, max: usize) -> Result<&'a str> {
    map.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && bounded(s, max))
        .ok_or_else(|| error::bad("Invalid or missing field"))
}
fn valid_text(v: &Value, max: usize) -> Result<&str> {
    v.as_str()
        .filter(|s| {
            s.len() <= max
                && !s
                    .chars()
                    .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
        })
        .ok_or_else(|| error::bad("Invalid text field"))
}
async fn edit_peer(
    app: App,
    headers: HeaderMap,
    book: String,
    bytes: Bytes,
    add: bool,
) -> ApiResult<StatusCode> {
    let token = token(&headers)?;
    let input: Map<String, Value> = parse(&bytes)?;
    app.store
        .run(move |c, secrets, config| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let user = authorize(&tx, &token)?;
            let shared = access(&tx, &user.id, &book, true)?;
            let id = string(&input, "id", 128)?;
            let old: Option<String> = tx
                .query_row(
                    "SELECT data FROM peers WHERE book=?1 AND id=?2",
                    params![book, id],
                    |r| r.get(0),
                )
                .optional()?;
            if add && old.is_some() {
                return Err(error::conflict("Peer already exists"));
            }
            if !add && old.is_none() {
                return Err(error::bad("Unknown peer"));
            }
            if add {
                let count: i64 =
                    tx.query_row("SELECT COUNT(*) FROM peers WHERE book=?1", [&book], |r| {
                        r.get(0)
                    })?;
                if count >= config.max_peers_per_book as i64 {
                    return Err(error::bad("Peer limit reached"));
                }
                tx.execute(
                    "INSERT INTO peers(book,id) VALUES(?1,?2)",
                    params![book, id],
                )?;
            }
            let mut data: Map<String, Value> =
                serde_json::from_str(old.as_deref().unwrap_or("{}"))?;
            for field in [
                "username",
                "hostname",
                "platform",
                "alias",
                "rdpPort",
                "rdpUsername",
                "loginName",
                "device_group_name",
                "note",
            ] {
                if let Some(value) = input.get(field) {
                    valid_text(value, if field == "note" { 4096 } else { 256 })?;
                    data.insert(field.into(), value.clone());
                }
            }
            if let Some(value) = input.get("forceAlwaysRelay") {
                if value != "true" && value != "false" {
                    return Err(error::bad("forceAlwaysRelay must be a string boolean"));
                }
                data.insert("forceAlwaysRelay".into(), value.clone());
            }
            // Client may send null. No sysinfo-derived same-server claim is made by this API.
            data.insert("same_server".into(), json!(false));
            tx.execute(
                "UPDATE peers SET data=?1 WHERE book=?2 AND id=?3",
                params![Value::Object(data).to_string(), book, id],
            )?;
            let secret_field = if shared { "password" } else { "hash" };
            if let Some(value) = input.get(secret_field) {
                let value = valid_text(value, 4096)?;
                let sealed = secrets.seal(&format!("peer/{book}/{id}/{secret_field}"), value)?;
                tx.execute(
                    if shared {
                        "UPDATE peers SET password=?1 WHERE book=?2 AND id=?3"
                    } else {
                        "UPDATE peers SET hash=?1 WHERE book=?2 AND id=?3"
                    },
                    params![sealed, book, id],
                )?;
            }
            if let Some(values) = input.get("tags") {
                let values = values
                    .as_array()
                    .filter(|v| v.len() <= config.max_tags_per_book)
                    .ok_or_else(|| error::bad("Invalid tags"))?;
                tx.execute(
                    "DELETE FROM peer_tags WHERE book=?1 AND peer=?2",
                    params![book, id],
                )?;
                for v in values {
                    let name = v
                        .as_str()
                        .filter(|s| !s.is_empty() && bounded(s, 128))
                        .ok_or_else(|| error::bad("Invalid tag name"))?;
                    ensure_tag(&tx, config, &book, name)?;
                    tx.execute(
                        "INSERT OR IGNORE INTO peer_tags VALUES(?1,?2,?3)",
                        params![book, id, name],
                    )?;
                }
            }
            audit(
                &tx,
                config,
                &user.id,
                if add { "peer.add" } else { "peer.update" },
                &format!("{book}/{id}"),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(StatusCode::OK)
}
fn ensure_tag(tx: &Transaction<'_>, config: &Config, book: &str, name: &str) -> Result<()> {
    if tx
        .query_row(
            "SELECT 1 FROM tags WHERE book=?1 AND name=?2",
            params![book, name],
            |_| Ok(()),
        )
        .optional()?
        .is_none()
    {
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM tags WHERE book=?1", [book], |r| {
            r.get(0)
        })?;
        if count >= config.max_tags_per_book as i64 {
            return Err(error::bad("Tag limit reached"));
        }
        tx.execute(
            "INSERT INTO tags(book,name) VALUES(?1,?2)",
            params![book, name],
        )?;
    };
    Ok(())
}

pub(super) async fn delete_peers(
    State(app): State<App>,
    headers: HeaderMap,
    Path(book): Path<String>,
    bytes: Bytes,
) -> ApiResult<StatusCode> {
    delete_items(app, headers, book, bytes, false).await
}
pub(super) async fn delete_tags(
    State(app): State<App>,
    headers: HeaderMap,
    Path(book): Path<String>,
    bytes: Bytes,
) -> ApiResult<StatusCode> {
    delete_items(app, headers, book, bytes, true).await
}
async fn delete_items(
    app: App,
    headers: HeaderMap,
    book: String,
    bytes: Bytes,
    tags: bool,
) -> ApiResult<StatusCode> {
    let token = token(&headers)?;
    let items: Vec<String> = parse(&bytes)?;
    if items.len() > 100 || items.iter().any(|s| s.is_empty() || !bounded(s, 128)) {
        return Err(error::bad("Invalid deletion list (maximum 100)").into());
    }
    app.store
        .run(move |c, _, config| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let user = authorize(&tx, &token)?;
            access(&tx, &user.id, &book, true)?;
            for item in items {
                tx.execute(
                    if tags {
                        "DELETE FROM tags WHERE book=?1 AND name=?2"
                    } else {
                        "DELETE FROM peers WHERE book=?1 AND id=?2"
                    },
                    params![book, item],
                )?;
                audit(
                    &tx,
                    config,
                    &user.id,
                    if tags { "tag.delete" } else { "peer.delete" },
                    &format!("{book}/{item}"),
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(StatusCode::OK)
}

pub(super) async fn add_tag(
    State(app): State<App>,
    headers: HeaderMap,
    Path(book): Path<String>,
    bytes: Bytes,
) -> ApiResult<StatusCode> {
    edit_tag(app, headers, book, bytes, "add").await
}
pub(super) async fn update_tag(
    State(app): State<App>,
    headers: HeaderMap,
    Path(book): Path<String>,
    bytes: Bytes,
) -> ApiResult<StatusCode> {
    edit_tag(app, headers, book, bytes, "update").await
}
pub(super) async fn rename_tag(
    State(app): State<App>,
    headers: HeaderMap,
    Path(book): Path<String>,
    bytes: Bytes,
) -> ApiResult<StatusCode> {
    edit_tag(app, headers, book, bytes, "rename").await
}
async fn edit_tag(
    app: App,
    headers: HeaderMap,
    book: String,
    bytes: Bytes,
    operation: &'static str,
) -> ApiResult<StatusCode> {
    let token = token(&headers)?;
    let input: Map<String, Value> = parse(&bytes)?;
    app.store
        .run(move |c, _, config| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let user = authorize(&tx, &token)?;
            access(&tx, &user.id, &book, true)?;
            let name = string(
                &input,
                if operation == "rename" { "old" } else { "name" },
                128,
            )?;
            let exists = tx
                .query_row(
                    "SELECT 1 FROM tags WHERE book=?1 AND name=?2",
                    params![book, name],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if operation == "add" {
                if exists {
                    return Err(error::conflict("Tag already exists"));
                }
                ensure_tag(&tx, config, &book, name)?;
            } else if !exists {
                return Err(error::bad("Unknown tag"));
            }
            if operation == "rename" {
                let new = string(&input, "new", 128)?;
                if name != new
                    && tx
                        .query_row(
                            "SELECT 1 FROM tags WHERE book=?1 AND name=?2",
                            params![book, new],
                            |_| Ok(()),
                        )
                        .optional()?
                        .is_some()
                {
                    return Err(error::conflict("Tag already exists"));
                }
                tx.execute(
                    "UPDATE tags SET name=?1 WHERE book=?2 AND name=?3",
                    params![new, book, name],
                )?;
            } else {
                let color = input
                    .get("color")
                    .and_then(Value::as_u64)
                    .filter(|c| *c <= u32::MAX as u64)
                    .ok_or_else(|| error::bad("Invalid tag color"))?;
                tx.execute(
                    "UPDATE tags SET color=?1 WHERE book=?2 AND name=?3",
                    params![color as i64, book, name],
                )?;
            }
            audit(
                &tx,
                config,
                &user.id,
                &format!("tag.{operation}"),
                &format!("{book}/{name}"),
            )?;
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(StatusCode::OK)
}
