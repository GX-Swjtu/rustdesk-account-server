use axum::{
    Router,
    body::{Body, Bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use rustdesk_account_server::{
    admin,
    config::Config,
    crypto,
    db::Store,
    http::App,
    limit::{Limiter, client_ip},
};
use serde_json::{Value, json};
use std::net::SocketAddr;
use tempfile::TempDir;
use tower::ServiceExt;

const PASSWORD: &str = "Only-A-Disposable-Test-Password-42";
const CREDENTIAL: &str = "Disposable-Remote-Credential-8cd101";
struct Fixture {
    _dir: TempDir,
    store: Store,
    router: Router,
}
impl Fixture {
    fn new() -> Self {
        Self::configured(|_| {})
    }
    fn configured(f: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Config {
            database: dir.path().join("db.sqlite3"),
            master_key: dir.path().join("master.key"),
            login_per_account_per_minute: 100,
            login_per_ip_per_minute: 100,
            login_global_per_minute: 100,
            ..Config::default()
        };
        f(&mut c);
        Store::init(&c).unwrap();
        let store = Store::open(c).unwrap();
        admin::create_user(&store, "alice", PASSWORD).unwrap();
        admin::create_user(&store, "bob", PASSWORD).unwrap();
        let router = App::new(store.clone()).unwrap().router();
        Self {
            _dir: dir,
            store,
            router,
        }
    }
    async fn raw(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: &str,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Bytes) {
        raw(&self.router, method, path, token, body, headers).await
    }
    async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Value,
    ) -> (StatusCode, Value) {
        let (status, body) = self.raw(method, path, token, &body.to_string(), &[]).await;
        (
            status,
            if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&body)
                    .unwrap_or_else(|_| json!(String::from_utf8_lossy(&body)))
            },
        )
    }
    async fn login(&self, user: &str, id: &str, uuid: &str) -> String {
        let (s,v)=self.call("POST","/api/login",None,json!({"username":user,"password":PASSWORD,"id":id,"uuid":uuid,"type":"account","autoLogin":true,"deviceInfo":{"os":"windows","type":"client","name":"test-pc"}})).await;
        assert_eq!(s, 200, "{v}");
        v["access_token"].as_str().unwrap().into()
    }
    async fn personal(&self, token: &str) -> String {
        let (s, b) = self
            .raw(
                "POST",
                "/api/ab/personal",
                Some(token),
                "",
                &[("content-type", "application/json")],
            )
            .await;
        assert_eq!(s, 200);
        serde_json::from_slice::<Value>(&b).unwrap()["guid"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn peers(&self, token: &str, book: &str) -> Value {
        let (s, b) = self
            .raw(
                "POST",
                &format!("/api/ab/peers?current=1&pageSize=100&ab={book}"),
                Some(token),
                "",
                &[],
            )
            .await;
        assert_eq!(s, 200);
        serde_json::from_slice(&b).unwrap()
    }
}
async fn raw(
    router: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, Bytes) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .extension(ConnectInfo(
            "203.0.113.10:4567".parse::<SocketAddr>().unwrap(),
        ));
    if let Some(t) = token {
        request = request.header("authorization", format!("Bearer {t}"));
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    (
        response.status(),
        response.into_body().collect().await.unwrap().to_bytes(),
    )
}

#[tokio::test]
async fn native_login_empty_posts_inventory_and_logout() {
    let f = Fixture::new();
    let login = json!({"username":"ALICE","password":PASSWORD,"id":"100001","uuid":"bWFjaGluZTE=","autoLogin":true,"type":"account","deviceInfo":{"name":"test-pc","os":"windows"}});
    let (s, b) = f
        .raw(
            "POST",
            "/api/login",
            None,
            &login.to_string(),
            &[("content-type", "text/plain; charset=utf-8")],
        )
        .await;
    assert_eq!(s, 200);
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["type"], "access_token");
    assert_eq!(v["user"]["status"], 1);
    let token = v["access_token"].as_str().unwrap();
    assert_eq!(token.len(), 43);
    let (s, v) = f
        .call(
            "POST",
            "/api/currentUser",
            Some(token),
            json!({"id":"should-not-register","uuid":"x"}),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(v["name"], "alice");
    let (_, v) = f
        .call(
            "GET",
            "/api/peers?current=1&pageSize=100&accessible=&status=1",
            Some(token),
            Value::Null,
        )
        .await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["data"][0]["id"], "100001");
    assert_eq!(v["data"][0]["info"]["os"], "Windows");
    assert!(v["data"][0].get("uuid").is_none());
    let (_, v) = f
        .call(
            "GET",
            "/api/users?current=1&pageSize=100&accessible=&status=1",
            Some(token),
            Value::Null,
        )
        .await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["data"][0]["name"], "alice");
    let (_, v) = f
        .call(
            "GET",
            "/api/device-group/accessible",
            Some(token),
            Value::Null,
        )
        .await;
    assert_eq!(v, json!({"total":0,"data":[]}));
    let (s, b) = f
        .raw("POST", "/api/ab/settings", Some(token), "", &[])
        .await;
    assert_eq!(s, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&b).unwrap()["max_peer_one_ab"],
        1000
    );
    let book = f.personal(token).await;
    assert_eq!(f.peers(token, &book).await["total"], 0);
    let (_, v) = f.call("GET", "/api/login-options", None, Value::Null).await;
    assert_eq!(v, json!([]));
    assert_eq!(
        f.call(
            "POST",
            "/api/logout",
            Some(token),
            json!({"id":"someone-else"})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        f.call("POST", "/api/currentUser", Some(token), Value::Null)
            .await
            .0,
        401
    );
}

#[tokio::test]
async fn password_login_transfers_inventory_and_revokes_only_device_sessions() {
    let f = Fixture::new();
    let alice = f.login("alice", "device", "uuid").await;
    let other = f.login("alice", "other", "uuid2").await;
    let ab = f.personal(&alice).await;
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/ab/peer/add/{ab}"),
            Some(&alice),
            json!({"id":"device","hash":CREDENTIAL})
        )
        .await
        .0,
        200
    );
    let bob = f.login("bob", "device", "uuid").await;
    assert_eq!(
        f.call(
            "POST",
            "/api/currentUser",
            Some(&alice),
            json!({"id":"device","uuid":"uuid"})
        )
        .await
        .0,
        401
    );
    assert_eq!(
        f.call(
            "POST",
            "/api/currentUser",
            Some(&other),
            json!({"id":"device","uuid":"uuid"})
        )
        .await
        .0,
        200
    );
    f.call(
        "POST",
        "/api/heartbeat",
        Some(&alice),
        json!({"id":"device","uuid":"uuid","owner":"alice"}),
    )
    .await;
    let (_, v) = f.call("GET", "/api/peers", Some(&bob), Value::Null).await;
    assert_eq!(v["total"], 1);
    let (_, v) = f.call("GET", "/api/peers", Some(&other), Value::Null).await;
    assert_eq!(v["total"], 1);
    assert_eq!(v["data"][0]["id"], "other");
    assert_eq!(f.peers(&other, &ab).await["data"][0]["hash"], CREDENTIAL);
    let bb = f.personal(&bob).await;
    assert_eq!(f.peers(&bob, &bb).await["total"], 0);
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/ab/peers?ab={ab}"),
            Some(&bob),
            Value::Null
        )
        .await
        .0,
        403
    );
    let (s,_)=f.call("POST","/api/login",None,json!({"username":"alice","password":PASSWORD,"id":"device","uuid":"different-machine","type":"account"})).await;
    assert_eq!(s, 409);
    admin::transfer_device(&f.store, "device", "alice", Some("replacement-machine")).unwrap();
    assert_eq!(
        f.call("POST", "/api/currentUser", Some(&bob), Value::Null)
            .await
            .0,
        401
    );
    f.login("alice", "device", "replacement-machine").await;
}

#[tokio::test]
async fn shared_book_permissions_and_password_sync_are_enforced_server_side() {
    let f = Fixture::new();
    let alice = f.login("alice", "a", "ua").await;
    let bob = f.login("bob", "b", "ub").await;
    let book = admin::create_book(&f.store, "Shared", "alice").unwrap();
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/ab/peers?ab={book}"),
            Some(&bob),
            Value::Null
        )
        .await
        .0,
        403
    );
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/ab/peer/add/{book}"),
            Some(&alice),
            json!({"id":"200001","password":CREDENTIAL,"hash":"must-not-be-stored"})
        )
        .await
        .0,
        200
    );
    admin::book_password(&f.store, &book, "Disposable-default-connection-password").unwrap();
    admin::grant(&f.store, &book, "bob", Some(1)).unwrap();
    assert_eq!(
        f.peers(&bob, &book).await["data"][0]["password"],
        CREDENTIAL
    );
    assert_eq!(f.peers(&bob, &book).await["data"][0]["hash"], "");
    let (_, profiles) = f
        .call("POST", "/api/ab/shared/profiles", Some(&bob), Value::Null)
        .await;
    assert_eq!(profiles["data"][0]["rule"], 1);
    assert_eq!(
        profiles["data"][0]["info"]["password"],
        "Disposable-default-connection-password"
    );
    for (method, path, body) in [
        (
            "PUT",
            format!("/api/ab/peer/update/{book}"),
            json!({"id":"200001","alias":"stolen"}),
        ),
        ("DELETE", format!("/api/ab/peer/{book}"), json!(["200001"])),
        (
            "POST",
            format!("/api/ab/tag/add/{book}"),
            json!({"name":"x","color":1}),
        ),
        (
            "PUT",
            format!("/api/ab/tag/rename/{book}"),
            json!({"old":"x","new":"y"}),
        ),
    ] {
        assert_eq!(f.call(method, &path, Some(&bob), body).await.0, 403);
    }
    admin::grant(&f.store, &book, "bob", Some(2)).unwrap();
    let (s, b) = f
        .raw(
            "PUT",
            &format!("/api/ab/peer/update/{book}"),
            Some(&bob),
            r#"{"id":"200001","alias":"shared edit"}"#,
            &[],
        )
        .await;
    assert_eq!(s, 200);
    assert!(b.is_empty());
    admin::grant(&f.store, &book, "bob", None).unwrap();
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/ab/peers?ab={book}"),
            Some(&bob),
            Value::Null
        )
        .await
        .0,
        403
    );
    assert_eq!(
        f.call("POST", "/api/ab/tags/guessed-guid", Some(&bob), Value::Null)
            .await
            .0,
        403
    );
}

#[tokio::test]
async fn peer_tag_crud_pagination_and_field_merge_follow_native_protocol() {
    let f = Fixture::new();
    let token = f.login("alice", "a", "ua").await;
    let book = f.personal(&token).await;
    for (method, path, body) in [
        (
            "POST",
            format!("/api/ab/tag/add/{book}"),
            json!({"name":"家用","color":4294901760u32}),
        ),
        (
            "POST",
            format!("/api/ab/peer/add/{book}"),
            json!({"id":"1","alias":"一","hash":CREDENTIAL,"password":"ignore-wrong-field","tags":["家用"],"forceAlwaysRelay":"true","same_server":null}),
        ),
        (
            "POST",
            format!("/api/ab/peer/add/{book}"),
            json!({"id":"2"}),
        ),
        (
            "PUT",
            format!("/api/ab/peer/update/{book}"),
            json!({"id":"1","note":"multi\nline"}),
        ),
        (
            "PUT",
            format!("/api/ab/tag/rename/{book}"),
            json!({"old":"家用","new":"Home"}),
        ),
        (
            "PUT",
            format!("/api/ab/tag/update/{book}"),
            json!({"name":"Home","color":4278190335u32}),
        ),
    ] {
        let (s, b) = f
            .raw(method, &path, Some(&token), &body.to_string(), &[])
            .await;
        assert_eq!(s, 200, "{b:?}");
        assert!(b.is_empty());
    }
    let v = f.peers(&token, &book).await;
    assert_eq!(v["total"], 2);
    assert_eq!(v["data"][0]["alias"], "一");
    assert_eq!(v["data"][0]["hash"], CREDENTIAL);
    assert_eq!(v["data"][0]["tags"], json!(["Home"]));
    assert_eq!(v["data"][0]["password"], "");
    let (_, v) = f
        .call(
            "POST",
            &format!("/api/ab/peers?current=2&pageSize=1&ab={book}"),
            Some(&token),
            Value::Null,
        )
        .await;
    assert_eq!(v["total"], 2);
    assert_eq!(v["data"][0]["id"], "2");
    let (_, v) = f
        .call(
            "POST",
            &format!("/api/ab/tags/{book}"),
            Some(&token),
            Value::Null,
        )
        .await;
    assert_eq!(v, json!([{"name":"Home","color":4278190335u32}]));
    assert_eq!(
        f.call(
            "PUT",
            &format!("/api/ab/peer/update/{book}"),
            Some(&token),
            json!({"id":"not-present","hash":CREDENTIAL})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        f.call(
            "POST",
            "/api/ab/peers?pageSize=101",
            Some(&token),
            Value::Null
        )
        .await
        .0,
        400
    );
    assert_eq!(
        f.call(
            "DELETE",
            &format!("/api/ab/tag/{book}"),
            Some(&token),
            json!(["Home"])
        )
        .await
        .0,
        200
    );
    assert_eq!(f.peers(&token, &book).await["data"][0]["tags"], json!([]));
    assert_eq!(
        f.call(
            "DELETE",
            &format!("/api/ab/peer/{book}"),
            Some(&token),
            json!(["1", "2"])
        )
        .await
        .0,
        200
    );
    assert_eq!(f.peers(&token, &book).await["total"], 0);
}

#[tokio::test]
async fn concurrent_partial_updates_from_separate_connections_do_not_lose_fields() {
    let f = Fixture::new();
    let token = f.login("alice", "a", "ua").await;
    let book = f.personal(&token).await;
    f.call(
        "POST",
        &format!("/api/ab/peer/add/{book}"),
        Some(&token),
        json!({"id":"1","hash":CREDENTIAL}),
    )
    .await;
    let other = App::new(Store::open((*f.store.config).clone()).unwrap())
        .unwrap()
        .router();
    let url = format!("/api/ab/peer/update/{book}");
    let (a, b) = tokio::join!(
        f.raw(
            "PUT",
            &url,
            Some(&token),
            r#"{"id":"1","alias":"updated"}"#,
            &[]
        ),
        raw(
            &other,
            "PUT",
            &url,
            Some(&token),
            r#"{"id":"1","note":"concurrent"}"#,
            &[]
        )
    );
    assert_eq!(a.0, 200);
    assert_eq!(b.0, 200);
    let v = f.peers(&token, &book).await;
    assert_eq!(v["data"][0]["alias"], "updated");
    assert_eq!(v["data"][0]["note"], "concurrent");
    assert_eq!(v["data"][0]["hash"], CREDENTIAL);
}

#[tokio::test]
async fn untrusted_reports_cannot_enroll_reassign_or_store_secrets() {
    let f = Fixture::new();
    let bad = json!({"id":"unknown","uuid":"guess","owner":"alice","password":CREDENTIAL,"token":CREDENTIAL,"preset-password":CREDENTIAL});
    for path in [
        "/api/heartbeat",
        "/api/sysinfo",
        "/api/audit/conn",
        "/api/audit/file",
    ] {
        assert_eq!(f.call("POST", path, None, bad.clone()).await.0, 200);
    }
    f.store
        .with(|c, _, _| {
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM devices", [], |r| r.get::<_, i64>(0))?,
                0
            );
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM client_reports", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
    let token = f.login("alice", "a", "ua").await;
    let (_, v) = f
        .call(
            "POST",
            "/api/heartbeat",
            None,
            json!({"id":"a","uuid":"ua","disconnect":[1],"strategy":{"x":"y"}}),
        )
        .await;
    assert_eq!(v, json!({"sysinfo":true}));
    f.call(
        "POST",
        "/api/sysinfo",
        None,
        json!({"id":"a","uuid":"wrong","hostname":"attacker"}),
    )
    .await;
    let (s,b)=f.raw("POST","/api/sysinfo",None,&json!({"id":"a","uuid":"ua","hostname":"client-reported-name","os":"Windows","username":"os-user","owner":"bob","password":CREDENTIAL,"preset-password":CREDENTIAL}).to_string(),&[]).await;
    assert_eq!(s, 200);
    assert_eq!(b, "SYSINFO_UPDATED");
    let (_, v) = f
        .call(
            "POST",
            "/api/heartbeat",
            None,
            json!({"id":"a","uuid":"ua"}),
        )
        .await;
    assert_eq!(v, json!({}));
    let before = f
        .store
        .with(|c, _, _| {
            Ok(
                c.query_row("SELECT last_seen FROM devices WHERE id='a'", [], |r| {
                    r.get::<_, i64>(0)
                })?,
            )
        })
        .unwrap();
    f.call(
        "POST",
        "/api/heartbeat",
        None,
        json!({"id":"a","uuid":"ua"}),
    )
    .await;
    let report = json!({"id":"a","uuid":"ua","nonce":"test-nonce","peer_id":"2","password":CREDENTIAL,"info":json!({"ip":"127.0.0.1","password":CREDENTIAL,"files":[["file.txt",123]]}).to_string()});
    f.call("POST", "/api/audit/file", None, report.clone())
        .await;
    f.call("POST", "/api/audit/file", None, report).await;
    f.store
        .with(|c, _, _| {
            assert_eq!(
                c.query_row("SELECT last_seen FROM devices WHERE id='a'", [], |r| r
                    .get::<_, i64>(0))?,
                before
            );
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM client_reports", [], |r| r
                    .get::<_, i64>(0))?,
                1
            );
            let data: String = c.query_row("SELECT data FROM client_reports", [], |r| r.get(0))?;
            assert!(!data.contains(CREDENTIAL));
            assert!(data.contains("unverified_client_report"));
            let info: String =
                c.query_row("SELECT info FROM devices WHERE id='a'", [], |r| r.get(0))?;
            assert!(!info.contains(CREDENTIAL));
            Ok(())
        })
        .unwrap();
    let (_, v) = f.call("GET", "/api/peers", Some(&token), Value::Null).await;
    assert_eq!(v["data"][0]["user_name"], "alice");
}

#[tokio::test]
async fn reset_disable_explicit_revoke_and_logout_are_immediate() {
    let f = Fixture::new();
    let first = f.login("alice", "a", "ua").await;
    let second = f.login("alice", "b", "ub").await;
    admin::reset_password(&f.store, "alice", PASSWORD).unwrap();
    for token in [first, second] {
        assert_eq!(
            f.call("POST", "/api/currentUser", Some(&token), Value::Null)
                .await
                .0,
            401
        );
    }
    let token = f.login("alice", "a", "ua").await;
    admin::set_disabled(&f.store, "alice", true).unwrap();
    assert_eq!(
        f.call("POST", "/api/currentUser", Some(&token), Value::Null)
            .await
            .0,
        401
    );
    let (s, disabled) = f
        .call(
            "POST",
            "/api/login",
            None,
            json!({"username":"alice","password":PASSWORD}),
        )
        .await;
    assert_eq!(s, 401);
    let (_, absent) = f
        .call(
            "POST",
            "/api/login",
            None,
            json!({"username":"absent","password":PASSWORD}),
        )
        .await;
    assert_eq!(disabled, absent);
    admin::set_disabled(&f.store, "alice", false).unwrap();
    assert_eq!(
        f.call("POST", "/api/currentUser", Some(&token), Value::Null)
            .await
            .0,
        401
    );
    let token = f.login("alice", "a", "ua").await;
    let session = f
        .store
        .with(|c, _, _| {
            Ok(c.query_row(
                "SELECT id FROM sessions WHERE token_hash=?1",
                [crypto::token_hash(&token)],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    admin::sessions(&f.store, admin::SessionCommand::Revoke { session }).unwrap();
    assert_eq!(
        f.call("POST", "/api/currentUser", Some(&token), Value::Null)
            .await
            .0,
        401
    );
}

#[tokio::test]
async fn restart_backup_restore_preserve_encrypted_data_and_reject_bad_keys() {
    let f = Fixture::new();
    let token = f.login("alice", "a", "ua").await;
    let book = f.personal(&token).await;
    f.call(
        "POST",
        &format!("/api/ab/peer/add/{book}"),
        Some(&token),
        json!({"id":"1","hash":CREDENTIAL}),
    )
    .await;
    let shared = admin::create_book(&f.store, "Team", "alice").unwrap();
    admin::book_password(&f.store, &shared, CREDENTIAL).unwrap();
    f.call(
        "POST",
        &format!("/api/ab/peer/add/{shared}"),
        Some(&token),
        json!({"id":"1","password":CREDENTIAL}),
    )
    .await;
    let backup = f._dir.path().join("backup.sqlite3");
    f.store.backup(&backup).unwrap();
    assert!(f.store.backup(&backup).is_err());
    for path in [
        &f.store.config.database,
        &backup,
        &f.store.config.database.with_extension("sqlite3-wal"),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            for secret in [PASSWORD, CREDENTIAL, &token] {
                assert!(
                    !bytes.windows(secret.len()).any(|s| s == secret.as_bytes()),
                    "plaintext secret in {}",
                    path.display()
                );
            }
        }
    }
    let restarted = App::new(Store::open((*f.store.config).clone()).unwrap())
        .unwrap()
        .router();
    assert_eq!(
        raw(
            &restarted,
            "POST",
            "/api/currentUser",
            Some(&token),
            "",
            &[]
        )
        .await
        .0,
        200
    );
    let mut restored = (*f.store.config).clone();
    restored.database = f._dir.path().join("restored.sqlite3");
    Store::restore(&restored, &backup).unwrap();
    let restored = App::new(Store::open(restored).unwrap()).unwrap().router();
    let (s, b) = raw(
        &restored,
        "POST",
        &format!("/api/ab/peers?ab={book}"),
        Some(&token),
        "",
        &[],
    )
    .await;
    assert_eq!(s, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&b).unwrap()["data"][0]["hash"],
        CREDENTIAL
    );
    let mut wrong = (*f.store.config).clone();
    wrong.master_key = f._dir.path().join("wrong.key");
    crypto::Secrets::create(&wrong.master_key).unwrap();
    assert!(Store::open(wrong.clone()).is_err());
    wrong.master_key = f._dir.path().join("missing.key");
    assert!(Store::open(wrong).is_err());
    assert!(Store::init(&f.store.config).is_err());
}

#[tokio::test]
async fn ip_limits_ignore_untrusted_forwarded_headers_and_bound_payloads() {
    let f = Fixture::configured(|c| {
        c.login_per_ip_per_minute = 2;
        c.max_request_bytes = 1024;
    });
    let body = json!({"username":"alice","password":"invalid-password"}).to_string();
    for ip in ["1.1.1.1", "8.8.8.8"] {
        assert_eq!(
            f.raw(
                "POST",
                "/api/login",
                None,
                &body,
                &[("x-forwarded-for", ip)]
            )
            .await
            .0,
            401
        );
    }
    assert_eq!(
        f.raw(
            "POST",
            "/api/login",
            None,
            &body,
            &[("x-forwarded-for", "9.9.9.9")]
        )
        .await
        .0,
        429
    );
    assert_eq!(
        f.raw("POST", "/api/login", None, &"x".repeat(1025), &[])
            .await
            .0,
        413
    );
    assert_eq!(
        f.raw("POST", "/api/ab/settings", None, &"x".repeat(1025), &[])
            .await
            .0,
        413
    );
    assert_eq!(
        f.raw("POST", "/api/login", None, "invalid-json", &[])
            .await
            .0,
        400
    );
}

#[test]
fn proxy_chain_and_limiter_capacity_fail_closed() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "x-forwarded-for",
        "6.6.6.6, 203.0.113.20, 10.0.0.2".parse().unwrap(),
    );
    let trusted = vec!["10.0.0.0/24".parse().unwrap()];
    assert_eq!(
        client_ip("10.0.0.1".parse().unwrap(), &headers, &trusted),
        "203.0.113.20".parse::<std::net::IpAddr>().unwrap()
    );
    assert_eq!(
        client_ip("203.0.113.21".parse().unwrap(), &headers, &trusted),
        "203.0.113.21".parse::<std::net::IpAddr>().unwrap()
    );
    headers.insert("x-forwarded-for", "invalid".parse().unwrap());
    assert_eq!(
        client_ip("10.0.0.1".parse().unwrap(), &headers, &trusted),
        "10.0.0.1".parse::<std::net::IpAddr>().unwrap()
    );
    let limit = Limiter::new(2);
    assert!(limit.allow(&[("a".into(), 2)]));
    assert!(limit.allow(&[("b".into(), 2)]));
    assert!(!limit.allow(&[("c".into(), 2)]));
    assert!(limit.allow(&[("a".into(), 2)]));
    assert!(!limit.allow(&[("a".into(), 2)]));
}

#[test]
fn password_policy_argon_parameters_and_aead_context_binding() {
    assert!(crypto::hash_password("").is_err());
    assert!(crypto::hash_password(&"x".repeat(1025)).is_err());
    let short_hash = crypto::hash_password("short").unwrap();
    assert!(crypto::verify_password("short", &short_hash));
    let hash = crypto::hash_password(PASSWORD).unwrap();
    assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    assert!(crypto::verify_password(PASSWORD, &hash));
    assert!(!crypto::verify_password("wrong", &hash));
    let dir = tempfile::tempdir().unwrap();
    let key = crypto::Secrets::create(&dir.path().join("master.key")).unwrap();
    let cipher = key.seal("peer/one/password", CREDENTIAL).unwrap();
    assert_eq!(key.open("peer/one/password", &cipher).unwrap(), CREDENTIAL);
    assert!(key.open("peer/two/password", &cipher).is_err());
}

#[tokio::test]
async fn configured_caps_and_retention_are_enforced() {
    let f = Fixture::configured(|c| {
        c.max_accounts = 2;
        c.max_devices = 1;
        c.max_books = 1;
        c.max_peers_per_book = 1;
        c.max_tags_per_book = 1;
        c.max_sessions_per_user = 1;
        c.max_audit_rows = 3;
        c.max_client_report_rows = 2;
    });
    assert!(admin::create_user(&f.store, "third", PASSWORD).is_err());
    let token = f.login("alice", "a", "ua").await;
    let (s, _) = f
        .call(
            "POST",
            "/api/login",
            None,
            json!({"username":"alice","password":PASSWORD,"id":"a","uuid":"ua"}),
        )
        .await;
    assert_eq!(s, 400);
    let (s, _) = f
        .call(
            "POST",
            "/api/login",
            None,
            json!({"username":"bob","password":PASSWORD,"id":"b","uuid":"ub"}),
        )
        .await;
    assert_eq!(s, 400);
    let book = f.personal(&token).await;
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/ab/peer/add/{book}"),
            Some(&token),
            json!({"id":"1","tags":["tag1"]})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/ab/peer/add/{book}"),
            Some(&token),
            json!({"id":"2"})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        f.call(
            "POST",
            &format!("/api/ab/tag/add/{book}"),
            Some(&token),
            json!({"name":"tag2","color":1})
        )
        .await
        .0,
        400
    );
    // Failed tag mutation rolls back the other fields in the same request.
    assert_eq!(
        f.call(
            "PUT",
            &format!("/api/ab/peer/update/{book}"),
            Some(&token),
            json!({"id":"1","alias":"must-rollback","tags":["tag2"]})
        )
        .await
        .0,
        400
    );
    assert!(
        f.peers(&token, &book).await["data"][0]
            .get("alias")
            .is_none()
    );
    admin::create_book(&f.store, "one", "alice").unwrap();
    assert!(admin::create_book(&f.store, "two", "alice").is_err());
    for n in 0..5 {
        assert_eq!(
            f.call(
                "POST",
                "/api/audit/conn",
                None,
                json!({"id":"a","uuid":"ua","nonce":format!("n{n}"),"conn_id":n})
            )
            .await
            .0,
            200
        );
    }
    f.store
        .with(|c, _, config| {
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM client_reports", [], |r| r
                    .get::<_, i64>(0))?,
                2
            );
            assert!(c.query_row("SELECT COUNT(*) FROM audit", [], |r| r.get::<_, i64>(0))? <= 3);
            c.execute("UPDATE client_reports SET ts=0", [])?;
            rustdesk_account_server::db::prune(
                c,
                "client_reports",
                config.max_client_report_rows,
                config.log_retention_days,
            )?;
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM client_reports", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn account_global_limits_and_bad_query_errors_are_json() {
    let f = Fixture::configured(|c| {
        c.login_per_account_per_minute = 1;
        c.login_global_per_minute = 2;
    });
    let bad = |username: &str| json!({"username":username,"password":"wrong-password"});
    assert_eq!(
        f.call("POST", "/api/login", None, bad("ALICE")).await.0,
        401
    );
    assert_eq!(
        f.call("POST", "/api/login", None, bad("alice")).await.0,
        429
    );
    assert_eq!(f.call("POST", "/api/login", None, bad("bob")).await.0, 401);
    assert_eq!(
        f.call("POST", "/api/login", None, bad("nobody")).await.0,
        429
    );
    let (s, b) = f
        .raw("GET", "/api/peers?current=not-a-number", None, "", &[])
        .await;
    assert_eq!(s, 400);
    assert!(
        serde_json::from_slice::<Value>(&b)
            .unwrap()
            .get("error")
            .is_some()
    );
}

#[tokio::test]
async fn guessed_book_ids_and_all_read_only_mutations_are_denied() {
    let f = Fixture::new();
    let alice = f.login("alice", "a", "ua").await;
    let bob = f.login("bob", "b", "ub").await;
    let private = f.personal(&alice).await;
    let shared = admin::create_book(&f.store, "ro", "alice").unwrap();
    admin::grant(&f.store, &shared, "bob", Some(1)).unwrap();
    for book in [private, shared, "unknown".into()] {
        for (method, path, body) in [
            (
                "POST",
                format!("/api/ab/peer/add/{book}"),
                json!({"id":"x"}),
            ),
            (
                "PUT",
                format!("/api/ab/peer/update/{book}"),
                json!({"id":"x","password":CREDENTIAL}),
            ),
            ("DELETE", format!("/api/ab/peer/{book}"), json!(["x"])),
            (
                "POST",
                format!("/api/ab/tag/add/{book}"),
                json!({"name":"x","color":1}),
            ),
            (
                "PUT",
                format!("/api/ab/tag/update/{book}"),
                json!({"name":"x","color":1}),
            ),
            (
                "PUT",
                format!("/api/ab/tag/rename/{book}"),
                json!({"old":"x","new":"y"}),
            ),
            ("DELETE", format!("/api/ab/tag/{book}"), json!(["x"])),
        ] {
            assert_eq!(f.call(method, &path, Some(&bob), body).await.0, 403);
        }
    }
}

#[cfg(unix)]
#[test]
fn permissive_key_files_are_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    std::fs::set_permissions(
        &f.store.config.master_key,
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(Store::open((*f.store.config).clone()).is_err());
}

#[tokio::test]
async fn native_connection_report_preserves_bounded_peer_tuple_not_raw_credentials() {
    let f = Fixture::new();
    f.login("alice", "a", "ua").await;
    assert_eq!(f.call("POST","/api/audit/conn",None,json!({"id":"a","uuid":"ua","peer":["123456","remote-pc"],"conn_id":7,"session_id":123,"type":0,"primary_auth":1,"two_factor":0,"password":CREDENTIAL,"hash":CREDENTIAL,"access_token":CREDENTIAL})).await.0,200);
    f.store
        .with(|c, _, _| {
            let raw: String = c.query_row(
                "SELECT data FROM client_reports WHERE kind='conn'",
                [],
                |r| r.get(0),
            )?;
            let value: Value = serde_json::from_str(&raw)?;
            assert_eq!(value["peer"], json!(["123456", "remote-pc"]));
            assert_eq!(value["primary_auth"], 1);
            assert!(!raw.contains(CREDENTIAL));
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn slow_bodies_are_limited_per_source_and_release_permits() {
    use tokio::{
        net::TcpStream,
        time::{Duration, timeout},
    };

    let f = Fixture::configured(|c| {
        c.max_concurrent_requests = 2;
        c.max_concurrent_requests_per_ip = 1;
        c.trusted_proxies = vec!["127.0.0.1/32".into()];
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = f.router.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let slow = TcpStream::connect(addr).await.unwrap();
    let partial = b"POST /api/login HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\nX-Forwarded-For: 203.0.113.10\r\n\r\n{";
    slow.writable().await.unwrap();
    assert_eq!(slow.try_write(partial).unwrap(), partial.len());
    // Wait until the slow request occupies its source's only permit.
    timeout(Duration::from_secs(2), async {
        loop {
            if f.raw("GET", "/healthz", None, "", &[]).await.0 == 429 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let response = f
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .extension(ConnectInfo(
                    "203.0.113.11:4567".parse::<SocketAddr>().unwrap(),
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    // A source outside trusted_proxies cannot change its concurrency bucket with XFF.
    assert_eq!(
        f.raw(
            "GET",
            "/healthz",
            None,
            "",
            &[("x-forwarded-for", "203.0.113.12")]
        )
        .await
        .0,
        429
    );
    drop(slow);
    timeout(Duration::from_secs(2), async {
        loop {
            if f.raw("GET", "/healthz", None, "", &[]).await.0 == 200 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn shared_book_create_and_rename_reject_native_profile_names() {
    let f = Fixture::new();
    let book = admin::create_book(&f.store, "Family", "alice").unwrap();
    for name in ["My address book", "Legacy address book"] {
        assert!(admin::create_book(&f.store, name, "alice").is_err());
        assert!(
            admin::books(
                &f.store,
                admin::BookCommand::Rename {
                    book: book.clone(),
                    name: name.into()
                }
            )
            .is_err()
        );
    }
    admin::books(
        &f.store,
        admin::BookCommand::Rename {
            book: book.clone(),
            name: "Home".into(),
        },
    )
    .unwrap();
    let token = f.login("alice", "a", "ua").await;
    let (status, profiles) = f
        .call("POST", "/api/ab/shared/profiles", Some(&token), Value::Null)
        .await;
    assert_eq!(status, 200);
    assert_eq!(profiles["data"].as_array().unwrap().len(), 1);
    assert_eq!(profiles["data"][0]["name"], "Home");
    assert_eq!(profiles["data"][0]["guid"], book);
}
