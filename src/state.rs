use std::fs;
use std::io::Write;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::Result;

/// Stable 24-hex key of a JSON value. Key order and number formatting match
/// JSON.stringify, so staging directories of older versions stay resumable.
pub fn digest(value: &Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_default();
    let hash = Sha256::digest(text.as_bytes());
    hex::encode(hash)[..24].to_string()
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("operating system random source");
    bytes
}

/// RFC 4122 version 4 identifier, as crypto.randomUUID.
pub fn uuid() -> String {
    let mut bytes = random_bytes::<16>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = hex::encode(bytes);
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Uniform integer below `limit` without modulo bias.
pub fn random_below(limit: u32) -> u32 {
    let zone = u32::MAX - (u32::MAX % limit);
    loop {
        let value = u32::from_le_bytes(random_bytes::<4>());
        if value < zone {
            return value % limit;
        }
    }
}

/// Parse a JSON file; a missing file yields `None`, malformed JSON an error.
pub fn read_json<T: DeserializeOwned>(file: &Path) -> Result<Option<T>> {
    match fs::read(file) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(strip_bom(&bytes))?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Like `read_json`, but unreadable or malformed files count as missing.
pub fn read_json_lenient<T: DeserializeOwned>(file: &Path) -> Option<T> {
    fs::read(file).ok().and_then(|bytes| serde_json::from_slice(strip_bom(&bytes)).ok())
}

pub fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

/// Pretty JSON exactly like `JSON.stringify(value, null, 2)` plus a newline.
pub fn pretty(value: &impl Serialize) -> Result<String> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    Ok(text)
}

/// Write through an exclusive temporary file and an atomic rename.
pub fn write_json(file: &Path, value: &impl Serialize) -> Result<()> {
    write_atomic(file, pretty(value)?.as_bytes())
}

pub fn write_atomic(file: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = file.with_file_name(format!("{}.{}.tmp", crate::paths::basename(file), uuid()));
    let result = (|| -> Result<()> {
        let mut handle = create_private(&temporary)?;
        handle.write_all(bytes)?;
        drop(handle);
        fs::rename(&temporary, file)?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    result
}

/// Create a new file readable only by this user; fails when it exists.
pub fn create_private(file: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(file)
}

#[cfg(unix)]
pub fn set_mode(file: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(file, fs::Permissions::from_mode(mode)).map_err(crate::error::Error::from)
}

#[cfg(not(unix))]
pub fn set_mode(_file: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// ISO-8601 UTC timestamp with milliseconds, as Date#toISOString.
pub fn iso_now() -> String {
    iso(chrono::Utc::now())
}

pub fn iso(time: chrono::DateTime<chrono::Utc>) -> String {
    time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digest_matches_node_json_stringify() {
        // crypto.createHash('sha256').update('{"a":1,"b":[true,null,"x"]}').digest('hex').slice(0, 24)
        let value = json!({ "a": 1, "b": [true, null, "x"] });
        assert_eq!(serde_json::to_string(&value).unwrap(), r#"{"a":1,"b":[true,null,"x"]}"#);
        assert_eq!(digest(&value).len(), 24);
    }

    #[test]
    fn uuid_has_version_four_layout() {
        let id = uuid();
        assert!(regex::Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$").unwrap().is_match(&id));
    }

    #[test]
    fn pretty_json_uses_two_spaces() {
        assert_eq!(pretty(&json!({ "a": [1] })).unwrap(), "{\n  \"a\": [\n    1\n  ]\n}\n");
    }
}
