use chrono::Utc;
use sha2::{Digest as _, Sha256};

use super::DbError;

pub(crate) fn random_token_hex(nbytes: usize) -> String {
    use rand::RngCore as _;
    let mut b = vec![0u8; nbytes];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex::encode(b)
}

pub(crate) fn sha256_hex(b: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b);
    let out = h.finalize();
    hex::encode(out)
}

pub(crate) fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub fn hash_password(password: &str) -> Result<String, DbError> {
    use argon2::password_hash::{PasswordHasher as _, SaltString};
    use argon2::Argon2;

    let pw = password.as_bytes();
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(pw, &salt)
        .map_err(|_| DbError::Unauthorized)?
        .to_string();
    Ok(hash)
}

pub(crate) fn verify_password(password: &str, hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier as _};
    use argon2::Argon2;

    let Ok(ph) = PasswordHash::new(hash) else {
        // Constant-time-ish failure path: still run a hash with a dummy salt.
        let _ = hash_password("dummy");
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &ph)
        .is_ok()
}

pub fn looks_like_email(s: &str) -> bool {
    // We keep this intentionally simple; we just need to stop obvious garbage.
    let s = s.trim();
    if s.is_empty() || s.len() > 254 {
        return false;
    }
    if s.contains(' ') || s.contains('\t') || s.contains('\n') || s.contains('\r') {
        return false;
    }
    let Some(at) = s.find('@') else { return false };
    if at == 0 || at + 1 >= s.len() {
        return false;
    }
    let domain = &s[at + 1..];
    domain.contains('.')
}
