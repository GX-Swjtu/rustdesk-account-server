use crate::{config::Config, crypto::Secrets, error};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::{
    fs::{File, OpenOptions},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;

pub const SCHEMA_VERSION: i64 = 1;
const SCHEMA: &str = r#"
CREATE TABLE meta(key TEXT PRIMARY KEY, value BLOB NOT NULL);
CREATE TABLE users(id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE COLLATE NOCASE,
 password_hash TEXT NOT NULL, disabled INTEGER NOT NULL DEFAULT 0, created INTEGER NOT NULL);
CREATE TABLE devices(id TEXT PRIMARY KEY, uuid TEXT NOT NULL, owner TEXT NOT NULL REFERENCES users(id),
 info TEXT NOT NULL DEFAULT '{}', last_seen INTEGER NOT NULL DEFAULT 0, updated INTEGER NOT NULL);
CREATE INDEX devices_owner ON devices(owner);
CREATE TABLE sessions(id TEXT PRIMARY KEY, token_hash BLOB NOT NULL UNIQUE, user_id TEXT NOT NULL REFERENCES users(id),
 device_id TEXT REFERENCES devices(id) ON DELETE CASCADE, machine_uuid TEXT,
 created INTEGER NOT NULL, revoked INTEGER);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE TABLE books(id TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES users(id), name TEXT NOT NULL,
 shared INTEGER NOT NULL DEFAULT 0, note TEXT NOT NULL DEFAULT '', password BLOB);
CREATE UNIQUE INDEX personal_owner ON books(owner) WHERE shared=0;
CREATE UNIQUE INDEX shared_name ON books(name) WHERE shared=1;
CREATE TABLE grants(book TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE,
 user_id TEXT NOT NULL REFERENCES users(id), rule INTEGER NOT NULL CHECK(rule IN (1,2)), PRIMARY KEY(book,user_id));
CREATE TABLE peers(book TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE, id TEXT NOT NULL,
 data TEXT NOT NULL DEFAULT '{}', hash BLOB, password BLOB, PRIMARY KEY(book,id));
CREATE TABLE tags(book TEXT NOT NULL REFERENCES books(id) ON DELETE CASCADE, name TEXT NOT NULL,
 color INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(book,name));
CREATE TABLE peer_tags(book TEXT NOT NULL, peer TEXT NOT NULL, tag TEXT NOT NULL,
 PRIMARY KEY(book,peer,tag),
 FOREIGN KEY(book,peer) REFERENCES peers(book,id) ON DELETE CASCADE,
 FOREIGN KEY(book,tag) REFERENCES tags(book,name) ON DELETE CASCADE ON UPDATE CASCADE);
CREATE TABLE audit(id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, actor TEXT NOT NULL, event TEXT NOT NULL, target TEXT NOT NULL);
CREATE INDEX audit_ts ON audit(ts);
CREATE TABLE client_reports(id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, device TEXT NOT NULL, kind TEXT NOT NULL, data TEXT NOT NULL, nonce TEXT,
 UNIQUE(device,kind,nonce));
CREATE INDEX client_reports_ts ON client_reports(ts);
PRAGMA user_version=1;
"#;

#[derive(Clone)]
pub struct Store {
    pub config: Arc<Config>,
    pub secrets: Arc<Secrets>,
    connection: Arc<Mutex<Connection>>,
    jobs: Arc<Semaphore>,
    _lock: Arc<File>,
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn private_dir(path: &Path) -> Result<()> {
    if !path.as_os_str().is_empty() && !path.exists() {
        std::fs::create_dir_all(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

pub fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("Refusing to overwrite {}", path.display()))
}

pub fn lock_database(config: &Config, exclusive: bool) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let f = options.open(config.database.with_extension("lockfile"))?;
    if exclusive {
        FileExt::try_lock_exclusive(&f)
    } else {
        FileExt::try_lock_shared(&f)
    }
    .context(
        "Database is in use; stop the API and other management commands before offline operations",
    )?;
    Ok(f)
}

fn configure(c: &Connection) -> Result<()> {
    c.busy_timeout(Duration::from_secs(5))?;
    c.execute_batch(
        "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
        PRAGMA trusted_schema=OFF; PRAGMA secure_delete=ON; PRAGMA temp_store=MEMORY;
        PRAGMA cache_size=-2048; PRAGMA journal_size_limit=8388608; PRAGMA wal_autocheckpoint=100;",
    )?;
    Ok(())
}

fn verify(c: &Connection, secrets: &Secrets) -> Result<()> {
    let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        bail!(
            "Unsupported database schema {version}; back up before migrating with a compatible program"
        );
    }
    let value: Vec<u8> = c.query_row("SELECT value FROM meta WHERE key='key_check'", [], |r| {
        r.get(0)
    })?;
    if secrets.open("meta/key_check/v1", &value)? != "rustdesk-account-server/key/v1" {
        bail!("Master key verification failed");
    }
    Ok(())
}

impl Store {
    pub fn init(config: &Config) -> Result<()> {
        config.validate()?;
        if config.database.exists() {
            bail!("Database already exists; initialization is never an overwrite operation");
        }
        if let Some(p) = config.database.parent() {
            private_dir(p)?;
        }
        if let Some(p) = config.master_key.parent() {
            private_dir(p)?;
        }
        let _lock = lock_database(config, true)?;
        let secrets = if config.master_key.exists() {
            Secrets::load(&config.master_key)?
        } else {
            Secrets::create(&config.master_key)?
        };
        private_file(&config.database)?.sync_all()?;
        let mut c = Connection::open(&config.database)?;
        configure(&c)?;
        let tx = c.transaction()?;
        tx.execute_batch(SCHEMA)?;
        tx.execute(
            "INSERT INTO meta VALUES('key_check', ?1)",
            [secrets.seal("meta/key_check/v1", "rustdesk-account-server/key/v1")?],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn open(config: Config) -> Result<Self> {
        config.validate()?;
        if !config.database.is_file() {
            bail!("Initialize a new database with the init command first");
        }
        let lock = lock_database(&config, false)?;
        let secrets = Secrets::load(&config.master_key)?;
        let c = Connection::open_with_flags(
            &config.database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        verify(&c, &secrets)?;
        configure(&c)?;
        Ok(Self {
            jobs: Arc::new(Semaphore::new(config.max_pending_database_jobs)),
            config: Arc::new(config),
            secrets: Arc::new(secrets),
            connection: Arc::new(Mutex::new(c)),
            _lock: Arc::new(lock),
        })
    }

    pub fn with<T>(
        &self,
        f: impl FnOnce(&mut Connection, &Secrets, &Config) -> Result<T>,
    ) -> Result<T> {
        let mut c = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Database worker failed"))?;
        f(&mut c, &self.secrets, &self.config)
    }

    pub async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection, &Secrets, &Config) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self
            .jobs
            .clone()
            .try_acquire_owned()
            .map_err(|_| error::busy())?;
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            store.with(f)
        })
        .await
        .context("Database worker stopped")?
    }

    pub fn backup(&self, path: &Path) -> Result<()> {
        private_file(path)?;
        self.with(|c, _, _| {
            let mut dst = Connection::open(path)?;
            rusqlite::backup::Backup::new(c, &mut dst)?.run_to_completion(
                64,
                Duration::from_millis(10),
                None,
            )?;
            Ok(())
        })
    }

    // Restores into a NEW path only; switching configuration is an explicit offline step.
    pub fn restore(config: &Config, source: &Path) -> Result<()> {
        if config.database.exists() {
            bail!("Restore target must not exist; preserve the old database and select a new path");
        }
        if let Some(parent) = config.database.parent() {
            private_dir(parent)?;
        }
        let _lock = lock_database(config, true)?;
        let secrets = Secrets::load(&config.master_key)?;
        let src = Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        verify(&src, &secrets)?;
        let integrity: String = src.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            bail!("Backup integrity check failed");
        }
        let fk: Option<String> = src
            .query_row("PRAGMA foreign_key_check", [], |r| r.get(0))
            .optional()?;
        if fk.is_some() {
            bail!("Backup foreign key check failed");
        }
        private_file(&config.database)?;
        let mut dst = Connection::open(&config.database)?;
        rusqlite::backup::Backup::new(&src, &mut dst)?.run_to_completion(
            64,
            Duration::from_millis(10),
            None,
        )?;
        Ok(())
    }
}

pub fn audit(
    tx: &Transaction<'_>,
    config: &Config,
    actor: &str,
    event: &str,
    target: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO audit(ts,actor,event,target) VALUES(?1,?2,?3,?4)",
        params![now(), actor, event, target],
    )?;
    prune(
        tx,
        "audit",
        config.max_audit_rows,
        config.log_retention_days,
    )?;
    Ok(())
}

pub fn prune(c: &Connection, table: &str, limit: usize, days: u32) -> Result<()> {
    // Table identifiers are internal constants, never request parameters.
    if table != "audit" && table != "client_reports" {
        bail!("Invalid retention table");
    }
    c.execute(
        &format!("DELETE FROM {table} WHERE ts < ?1"),
        [now() - i64::from(days) * 86400],
    )?;
    c.execute(&format!("DELETE FROM {table} WHERE id <= (SELECT id FROM {table} ORDER BY id DESC LIMIT 1 OFFSET ?1)"), [limit as i64])?;
    Ok(())
}
