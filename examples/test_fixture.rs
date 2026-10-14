//! Disposable local fixtures only. Not included in the production container.
use anyhow::{Result, bail};
use rustdesk_account_server::{admin, config::Config, db::Store};
use std::path::PathBuf;
fn main() -> Result<()> {
    let dir = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("Usage: test_fixture NEW_DIRECTORY"))?,
    );
    if dir.exists() {
        bail!("Test directory must not already exist");
    }
    std::fs::create_dir_all(&dir)?;
    let config = Config {
        database: dir.join("accounts.sqlite3"),
        master_key: dir.join("master.key"),
        login_per_ip_per_minute: 60,
        login_per_account_per_minute: 60,
        login_global_per_minute: 60,
        ..Config::default()
    };
    Store::init(&config)?;
    let store = Store::open(config.clone())?;
    admin::create_user(&store, "loadtest", "Disposable-Load-Test-Password-Only")?;
    std::fs::write(dir.join("account.toml"), toml::to_string(&config)?)?;
    println!("Disposable test fixture created; never expose this fixture to the Internet.");
    Ok(())
}
