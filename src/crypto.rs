use anyhow::{Context, Result, bail};
use argon2::{
    Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version,
    password_hash::SaltString,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};
use zeroize::Zeroizing;

pub struct Secrets(XChaCha20Poly1305);

impl Secrets {
    pub fn load(path: &Path) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(path)
            .context("Master key is missing; restore the original key, do not regenerate it")?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("Master key must be a regular file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                bail!("Master key permissions must be 0600 or 0400");
            }
        }
        let mut key = Zeroizing::new(Vec::new());
        File::open(path)?.take(33).read_to_end(&mut key)?;
        if key.len() != 32 {
            bail!("Master key must contain exactly 32 raw bytes");
        }
        Ok(Self(
            XChaCha20Poly1305::new_from_slice(&key).map_err(|_| anyhow::anyhow!("Invalid key"))?,
        ))
    }

    pub fn create(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(path)
            .context("Refusing to overwrite master key")?;
        let mut key = Zeroizing::new([0u8; 32]);
        OsRng.fill_bytes(&mut *key);
        file.write_all(&*key)?;
        file.sync_all()?;
        Self::load(path)
    }

    pub fn seal(&self, context: &str, value: &str) -> Result<Vec<u8>> {
        let mut nonce = [0u8; 24];
        OsRng.fill_bytes(&mut nonce);
        let encrypted = self
            .0
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: value.as_bytes(),
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("Credential encryption failed"))?;
        let mut bytes = vec![1];
        bytes.extend(nonce);
        bytes.extend(encrypted);
        Ok(bytes)
    }

    pub fn open(&self, context: &str, bytes: &[u8]) -> Result<String> {
        if bytes.len() < 41 || bytes[0] != 1 {
            bail!("Invalid encrypted credential");
        }
        let plaintext = self
            .0
            .decrypt(
                XNonce::from_slice(&bytes[1..25]),
                Payload {
                    msg: &bytes[25..],
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| anyhow::anyhow!("Wrong master key or damaged encrypted credential"))?;
        String::from_utf8(plaintext).context("Invalid credential encoding")
    }
}

fn argon() -> Result<Argon2<'static>> {
    let params = Params::new(19456, 2, 1, Some(32))
        .map_err(|_| anyhow::anyhow!("Invalid Argon2 parameters"))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

pub fn hash_password(password: &str) -> Result<String> {
    if password.is_empty() || password.len() > 1024 {
        bail!("Password must not be empty and must be at most 1024 bytes");
    }
    let salt = SaltString::generate(&mut OsRng);
    argon()?
        .hash_password(password.as_bytes(), &salt)
        .map(|x| x.to_string())
        .map_err(|_| anyhow::anyhow!("Password hashing failed"))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(hash) = PasswordHash::new(hash) else {
        return false;
    };
    argon()
        .map(|a| a.verify_password(password.as_bytes(), &hash).is_ok())
        .unwrap_or(false)
}

pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
