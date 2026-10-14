use crate::{
    crypto,
    db::{Store, audit, now},
    error,
};
use anyhow::{Result, bail};
use clap::{Subcommand, ValueEnum};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::io::IsTerminal;
use zeroize::Zeroizing;

#[derive(Subcommand)]
pub enum UserCommand {
    Create {
        username: String,
        #[arg(long)]
        generate: bool,
    },
    List,
    Disable {
        username: String,
    },
    Enable {
        username: String,
    },
    Password {
        username: String,
        #[arg(long)]
        generate: bool,
    },
}
#[derive(Subcommand)]
pub enum SessionCommand {
    List {
        #[arg(long)]
        username: Option<String>,
    },
    Revoke {
        session: String,
    },
    RevokeAll {
        username: String,
    },
}
#[derive(Subcommand)]
pub enum DeviceCommand {
    List,
    Unbind {
        id: String,
    },
    Transfer {
        id: String,
        #[arg(long)]
        owner: String,
    },
    Resolve {
        id: String,
        #[arg(long)]
        uuid: String,
        #[arg(long)]
        owner: String,
    },
}
#[derive(Clone, Copy, ValueEnum)]
pub enum Permission {
    Read,
    Write,
    None,
}
#[derive(Subcommand)]
pub enum BookCommand {
    Create {
        name: String,
        #[arg(long)]
        owner: String,
    },
    List,
    Rename {
        book: String,
        name: String,
    },
    Delete {
        book: String,
    },
    Grant {
        book: String,
        username: String,
        #[arg(value_enum)]
        permission: Permission,
    },
    Password {
        book: String,
        #[arg(long)]
        clear: bool,
    },
}

pub fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
pub fn bounded(value: &str, max: usize) -> bool {
    value.len() <= max && !value.chars().any(char::is_control)
}
pub fn user_id(c: &Connection, username: &str) -> Result<String> {
    c.query_row("SELECT id FROM users WHERE username=?1", [username], |r| {
        r.get(0)
    })
    .optional()?
    .ok_or_else(|| error::bad("Unknown account"))
}

pub fn create_user(store: &Store, username: &str, password: &str) -> Result<String> {
    if !valid_name(username) {
        bail!("Username must be 1-64 ASCII letters, numbers, dots, underscores or hyphens");
    }
    let hash = crypto::hash_password(password)?;
    store.with(|c, _, config| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
        if count >= config.max_accounts as i64 {
            return Err(error::bad("Account limit reached"));
        }
        if tx
            .query_row("SELECT 1 FROM users WHERE username=?1", [username], |_| {
                Ok(())
            })
            .optional()?
            .is_some()
        {
            return Err(error::conflict("Account already exists"));
        }
        let id = crypto::id();
        tx.execute(
            "INSERT INTO users(id,username,password_hash,created) VALUES(?1,?2,?3,?4)",
            params![id, username, hash, now()],
        )?;
        tx.execute(
            "INSERT INTO books(id,owner,name) VALUES(?1,?2,'Personal')",
            params![crypto::id(), id],
        )?;
        audit(&tx, config, "local-admin", "user.create", &id)?;
        tx.commit()?;
        Ok(id)
    })
}

pub fn reset_password(store: &Store, username: &str, password: &str) -> Result<()> {
    let hash = crypto::hash_password(password)?;
    store.with(|c, _, config| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = user_id(&tx, username)?;
        tx.execute(
            "UPDATE users SET password_hash=?1 WHERE id=?2",
            params![hash, id],
        )?;
        tx.execute(
            "UPDATE sessions SET revoked=?1 WHERE user_id=?2 AND revoked IS NULL",
            params![now(), id],
        )?;
        audit(
            &tx,
            config,
            "local-admin",
            "user.password_reset_and_revoke",
            &id,
        )?;
        tx.commit()?;
        Ok(())
    })
}

pub fn set_disabled(store: &Store, username: &str, disabled: bool) -> Result<()> {
    store.with(|c, _, config| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = user_id(&tx, username)?;
        tx.execute(
            "UPDATE users SET disabled=?1 WHERE id=?2",
            params![disabled, id],
        )?;
        if disabled {
            tx.execute(
                "UPDATE sessions SET revoked=?1 WHERE user_id=?2 AND revoked IS NULL",
                params![now(), id],
            )?;
        }
        audit(
            &tx,
            config,
            "local-admin",
            if disabled {
                "user.disable_and_revoke"
            } else {
                "user.enable"
            },
            &id,
        )?;
        tx.commit()?;
        Ok(())
    })
}

pub fn create_book(store: &Store, name: &str, owner: &str) -> Result<String> {
    if name.trim().is_empty() || !bounded(name, 128) {
        return Err(error::bad("Invalid book name"));
    }
    store.with(|c, _, config| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let owner = user_id(&tx, owner)?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM books WHERE shared=1", [], |r| {
            r.get(0)
        })?;
        if count >= config.max_books as i64 {
            return Err(error::bad("Book limit reached"));
        }
        let id = crypto::id();
        tx.execute(
            "INSERT INTO books(id,owner,name,shared) VALUES(?1,?2,?3,1)",
            params![id, owner, name],
        )?;
        audit(&tx, config, "local-admin", "book.create", &id)?;
        tx.commit()?;
        Ok(id)
    })
}

pub fn grant(store: &Store, book: &str, username: &str, rule: Option<i64>) -> Result<()> {
    store.with(|c, _, config| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        shared_exists(&tx, book)?;
        let user = user_id(&tx, username)?;
        if let Some(rule) = rule {
            if rule != 1 && rule != 2 { return Err(error::bad("Invalid grant")); }
            tx.execute("INSERT INTO grants(book,user_id,rule) VALUES(?1,?2,?3) ON CONFLICT(book,user_id) DO UPDATE SET rule=excluded.rule", params![book,user,rule])?;
        } else { tx.execute("DELETE FROM grants WHERE book=?1 AND user_id=?2", params![book,user])?; }
        audit(&tx, config, "local-admin", "book.grant_change", &format!("{book}/{user}"))?;
        tx.commit()?; Ok(())
    })
}
fn shared_exists(c: &Connection, book: &str) -> Result<()> {
    if c.query_row(
        "SELECT 1 FROM books WHERE id=?1 AND shared=1",
        [book],
        |_| Ok(()),
    )
    .optional()?
    .is_none()
    {
        return Err(error::bad("Unknown shared book"));
    }
    Ok(())
}
pub fn book_password(store: &Store, book: &str, value: &str) -> Result<()> {
    if value.len() > 4096 {
        return Err(error::bad("Password too long"));
    }
    store.with(|c, secrets, config| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        shared_exists(&tx, book)?;
        let encrypted = secrets.seal(&format!("book/{book}/password"), value)?;
        tx.execute(
            "UPDATE books SET password=?1 WHERE id=?2",
            params![encrypted, book],
        )?;
        audit(&tx, config, "local-admin", "book.password_change", book)?;
        tx.commit()?;
        Ok(())
    })
}

pub fn transfer_device(store: &Store, id: &str, owner: &str, uuid: Option<&str>) -> Result<()> {
    if uuid.is_some_and(|u| u.is_empty() || !bounded(u, 128)) {
        return Err(error::bad("Invalid UUID"));
    }
    store.with(|c, _, config| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let owner = user_id(&tx, owner)?;
        let n = if let Some(uuid) = uuid {
            tx.execute(
                "UPDATE devices SET owner=?1,uuid=?2,info='{}',last_seen=0,updated=?3 WHERE id=?4",
                params![owner, uuid, now(), id],
            )?
        } else {
            tx.execute(
                "UPDATE devices SET owner=?1,updated=?2 WHERE id=?3",
                params![owner, now(), id],
            )?
        };
        if n == 0 {
            return Err(error::bad("Unknown device"));
        }
        tx.execute(
            "UPDATE sessions SET revoked=?1 WHERE device_id=?2 AND revoked IS NULL",
            params![now(), id],
        )?;
        audit(
            &tx,
            config,
            "local-admin",
            if uuid.is_some() {
                "device.resolve_and_revoke"
            } else {
                "device.transfer_and_revoke"
            },
            id,
        )?;
        tx.commit()?;
        Ok(())
    })
}

fn password(generate: bool) -> Result<Zeroizing<String>> {
    if !std::io::stdin().is_terminal() {
        bail!(
            "Password management requires a terminal; plaintext CLI arguments and redirected input are disabled"
        );
    }
    if generate {
        if !std::io::stdout().is_terminal() {
            bail!("Generated passwords may only be displayed on a terminal");
        }
        Ok(Zeroizing::new(crypto::random_token()))
    } else {
        let first = Zeroizing::new(rpassword::prompt_password("New password: ")?);
        let second = Zeroizing::new(rpassword::prompt_password("Confirm password: ")?);
        if *first != *second {
            bail!("Passwords do not match");
        }
        Ok(first)
    }
}
fn print(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub fn users(store: &Store, command: UserCommand) -> Result<()> {
    match command {
        UserCommand::Create {username, generate} => {
            let secret = password(generate)?;
            let id = create_user(store, &username, &secret)?;
            println!("Created account {username} ({id})");
            if generate { println!("Save this password now: {}", *secret); }
        }
        UserCommand::Password {username, generate} => {
            let secret = password(generate)?; reset_password(store,&username,&secret)?;
            println!("Password changed; all account sessions revoked");
            if generate { println!("Save this password now: {}", *secret); }
        }
        UserCommand::Disable{username} => set_disabled(store,&username,true)?,
        UserCommand::Enable{username} => set_disabled(store,&username,false)?,
        UserCommand::List => print(store.with(|c,_,_| {
            let mut s = c.prepare("SELECT id,username,disabled,created FROM users ORDER BY username")?;
            let rows = s.query_map([], |r| Ok(json!({"id":r.get::<_,String>(0)?,"username":r.get::<_,String>(1)?,"disabled":r.get::<_,bool>(2)?,"created":r.get::<_,i64>(3)?})))?;
            Ok(Value::Array(rows.collect::<rusqlite::Result<_>>()?))
        })?)?,
    };
    Ok(())
}

pub fn sessions(store: &Store, command: SessionCommand) -> Result<()> {
    store.with(|c,_,config| {
        match command {
            SessionCommand::List{username} => {
                let mut s = c.prepare("SELECT s.id,u.username,s.device_id,s.created,s.revoked FROM sessions s JOIN users u ON s.user_id=u.id WHERE (?1 IS NULL OR u.username=?1) ORDER BY s.created DESC LIMIT 1000")?;
                let rows = s.query_map([username], |r| Ok(json!({"id":r.get::<_,String>(0)?,"username":r.get::<_,String>(1)?,"device":r.get::<_,Option<String>>(2)?,"created":r.get::<_,i64>(3)?,"revoked":r.get::<_,Option<i64>>(4)?})))?;
                print(Value::Array(rows.collect::<rusqlite::Result<_>>()?))?;
            }
            SessionCommand::Revoke{session} => {
                let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
                if tx.execute("UPDATE sessions SET revoked=?1 WHERE id=?2", params![now(),session])? == 0 { return Err(error::bad("Unknown session")); }
                audit(&tx,config,"local-admin","session.revoke",&session)?; tx.commit()?;
            }
            SessionCommand::RevokeAll{username} => {
                let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let id = user_id(&tx,&username)?;
                tx.execute("UPDATE sessions SET revoked=?1 WHERE user_id=?2 AND revoked IS NULL", params![now(),id])?;
                audit(&tx,config,"local-admin","session.revoke_all",&id)?; tx.commit()?;
            }
        }; Ok(())
    })
}

pub fn devices(store: &Store, command: DeviceCommand) -> Result<()> {
    match command {
        DeviceCommand::Transfer{id,owner} => transfer_device(store,&id,&owner,None),
        DeviceCommand::Resolve{id,uuid,owner} => transfer_device(store,&id,&owner,Some(&uuid)),
        DeviceCommand::Unbind{id} => store.with(|c,_,config| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if tx.execute("DELETE FROM devices WHERE id=?1", [&id])? == 0 { return Err(error::bad("Unknown device")); }
            audit(&tx,config,"local-admin","device.unbind_and_revoke",&id)?; tx.commit()?; Ok(())
        }),
        DeviceCommand::List => print(store.with(|c,_,_| {
            let mut s = c.prepare("SELECT d.id,d.uuid,u.username,d.info,d.last_seen FROM devices d JOIN users u ON d.owner=u.id ORDER BY d.id")?;
            let rows = s.query_map([], |r| Ok(json!({"id":r.get::<_,String>(0)?,"uuid":r.get::<_,String>(1)?,"owner":r.get::<_,String>(2)?,"info":r.get::<_,String>(3)?,"last_reported_at":r.get::<_,i64>(4)?})))?;
            Ok(Value::Array(rows.collect::<rusqlite::Result<_>>()?))
        })?),
    }
}

pub fn books(store: &Store, command: BookCommand) -> Result<()> {
    match command {
        BookCommand::Create{name,owner} => { println!("{}",create_book(store,&name,&owner)?); Ok(()) },
        BookCommand::Grant{book,username,permission} => grant(store,&book,&username,match permission {Permission::Read=>Some(1),Permission::Write=>Some(2),Permission::None=>None}),
        BookCommand::Password{book,clear} => {
            let value = if clear { Zeroizing::new(String::new()) } else { password(false)? };
            book_password(store,&book,&value)
        }
        BookCommand::Rename{book,name} => store.with(|c,_,config| {
            if name.trim().is_empty() || !bounded(&name,128) { return Err(error::bad("Invalid book name")); }
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?; shared_exists(&tx,&book)?;
            tx.execute("UPDATE books SET name=?1 WHERE id=?2",params![name,book])?;
            audit(&tx,config,"local-admin","book.rename",&book)?; tx.commit()?; Ok(())
        }),
        BookCommand::Delete{book} => store.with(|c,_,config| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?; shared_exists(&tx,&book)?;
            tx.execute("DELETE FROM books WHERE id=?1",[&book])?;
            audit(&tx,config,"local-admin","book.delete",&book)?; tx.commit()?; Ok(())
        }),
        BookCommand::List => print(store.with(|c,_,_| {
            let mut s = c.prepare("SELECT b.id,b.name,u.username FROM books b JOIN users u ON b.owner=u.id WHERE b.shared=1 ORDER BY b.name")?;
            let rows = s.query_map([], |r| Ok(json!({"guid":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"owner":r.get::<_,String>(2)?})))?;
            Ok(Value::Array(rows.collect::<rusqlite::Result<_>>()?))
        })?),
    }
}

pub fn logs(store: &Store, client_reports: bool, limit: usize) -> Result<()> {
    if !(1..=1000).contains(&limit) {
        bail!("Limit must be between 1 and 1000");
    }
    print(store.with(|c,_,_| {
        let mut s = c.prepare(if client_reports {"SELECT ts,device,kind,data FROM client_reports ORDER BY id DESC LIMIT ?1"} else {"SELECT ts,actor,event,target FROM audit ORDER BY id DESC LIMIT ?1"})?;
        let rows = s.query_map([limit as i64], |r| Ok(if client_reports {
            json!({"source":"unverified_client_report","ts":r.get::<_,i64>(0)?,"device":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"data":r.get::<_,String>(3)?})
        } else { json!({"source":"server","ts":r.get::<_,i64>(0)?,"actor":r.get::<_,String>(1)?,"event":r.get::<_,String>(2)?,"target":r.get::<_,String>(3)?}) }))?;
        Ok(Value::Array(rows.collect::<rusqlite::Result<_>>()?))
    })?)
}
