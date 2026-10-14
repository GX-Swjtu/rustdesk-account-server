use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub listen: SocketAddr,
    pub database: PathBuf,
    pub master_key: PathBuf,
    pub trusted_proxies: Vec<String>,
    pub max_accounts: usize,
    pub max_devices: usize,
    pub max_books: usize,
    pub max_peers_per_book: usize,
    pub max_tags_per_book: usize,
    pub max_sessions_per_user: usize,
    pub max_request_bytes: usize,
    pub max_concurrent_requests: usize,
    pub max_concurrent_requests_per_ip: usize,
    pub max_pending_database_jobs: usize,
    pub password_workers: usize,
    pub login_per_ip_per_minute: usize,
    pub login_per_account_per_minute: usize,
    pub login_global_per_minute: usize,
    pub requests_per_ip_per_minute: usize,
    pub limiter_capacity: usize,
    pub log_retention_days: u32,
    pub max_audit_rows: usize,
    pub max_client_report_rows: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:21114".parse().expect("constant address"),
            database: "data/accounts.sqlite3".into(),
            master_key: "secrets/master.key".into(),
            trusted_proxies: vec![],
            max_accounts: 10,
            max_devices: 100,
            max_books: 30,
            max_peers_per_book: 1000,
            max_tags_per_book: 100,
            max_sessions_per_user: 100,
            max_request_bytes: 65536,
            max_concurrent_requests: 32,
            max_concurrent_requests_per_ip: 4,
            max_pending_database_jobs: 32,
            password_workers: 1,
            login_per_ip_per_minute: 12,
            login_per_account_per_minute: 8,
            login_global_per_minute: 30,
            requests_per_ip_per_minute: 600,
            limiter_capacity: 4096,
            log_retention_days: 30,
            max_audit_rows: 20000,
            max_client_report_rows: 20000,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut c: Self = toml::from_str(
            &std::fs::read_to_string(path)
                .with_context(|| format!("Cannot read configuration {}", path.display()))?,
        )?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        if c.database.is_relative() {
            c.database = base.join(&c.database);
        }
        if c.master_key.is_relative() {
            c.master_key = base.join(&c.master_key);
        }
        c.validate()?;
        Ok(c)
    }

    pub fn validate(&self) -> Result<()> {
        for s in &self.trusted_proxies {
            let _: ipnet::IpNet = s.parse().context("Invalid trusted proxy CIDR")?;
        }
        for n in [
            self.max_accounts,
            self.max_devices,
            self.max_books,
            self.max_peers_per_book,
            self.max_tags_per_book,
            self.max_sessions_per_user,
            self.max_concurrent_requests,
            self.max_concurrent_requests_per_ip,
            self.max_pending_database_jobs,
            self.password_workers,
            self.login_per_ip_per_minute,
            self.login_per_account_per_minute,
            self.login_global_per_minute,
            self.requests_per_ip_per_minute,
            self.limiter_capacity,
            self.max_audit_rows,
            self.max_client_report_rows,
        ] {
            if n == 0 || n > 1_000_000 {
                bail!("Limits must be between 1 and 1000000");
            }
        }
        if !(1024..=1_048_576).contains(&self.max_request_bytes) {
            bail!("Request size must be 1 KiB to 1 MiB");
        }
        if self.password_workers > 4 || !(1..=3650).contains(&self.log_retention_days) {
            bail!("Invalid password worker count or retention");
        }
        Ok(())
    }
}
