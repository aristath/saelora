use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::util::now_ms;
use super::DbError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitlistRecord {
    pub email: String,
    pub created_at: i64,
}

pub(crate) fn add(path: &Path, email: &str) -> Result<(), DbError> {
    let e = email.trim();
    if e.is_empty() {
        return Ok(());
    }

    // Avoid duplicates.
    if has(path, e)? {
        return Ok(());
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let f = OpenOptions::new().create(true).append(true).open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    let mut w = BufWriter::new(f);
    let rec = WaitlistRecord {
        email: e.to_string(),
        created_at: now_ms(),
    };
    let mut b = serde_json::to_vec(&rec)?;
    b.push(b'\n');
    w.write_all(&b)?;
    w.flush()?;
    Ok(())
}

pub(crate) fn list(path: &Path) -> Result<Vec<WaitlistRecord>, DbError> {
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(DbError::Io(e)),
    };
    let br = BufReader::new(f);
    let mut by_email: HashMap<String, WaitlistRecord> = HashMap::new();
    for line in br.lines() {
        let line = line?;
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Ok(r) = serde_json::from_str::<WaitlistRecord>(t) {
            let e = r.email.trim();
            if !e.is_empty() {
                let key = e.to_ascii_lowercase();
                // Keep the earliest timestamp for a given email.
                match by_email.get(&key) {
                    Some(existing) if existing.created_at <= r.created_at => {}
                    _ => {
                        by_email.insert(key, r);
                    }
                }
            }
        }
    }
    let mut out = by_email.into_values().collect::<Vec<_>>();
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(out)
}

pub(crate) fn has(path: &Path, email: &str) -> Result<bool, DbError> {
    let target = email.trim();
    if target.is_empty() {
        return Ok(false);
    }
    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(DbError::Io(e)),
    };
    let br = BufReader::new(f);
    for line in br.lines() {
        let line = line?;
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Ok(r) = serde_json::from_str::<WaitlistRecord>(t) {
            if r.email.trim().eq_ignore_ascii_case(target) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(crate) fn remove(path: &Path, email: &str) -> Result<(), DbError> {
    let target = email.trim();
    if target.is_empty() {
        return Ok(());
    }

    let f = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(DbError::Io(e)),
    };

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    let br = BufReader::new(f);
    let mut bw = BufWriter::new(tmp.as_file_mut());

    for line in br.lines() {
        let line = line?;
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if let Ok(r) = serde_json::from_str::<WaitlistRecord>(t) {
            if r.email.trim().eq_ignore_ascii_case(target) {
                continue;
            }
            writeln!(bw, "{}", t)?;
        } else {
            // Keep unknown lines.
            writeln!(bw, "{}", t)?;
        }
    }
    bw.flush()?;
    drop(bw);

    tmp.persist(path).map_err(|e| DbError::Io(e.error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}
