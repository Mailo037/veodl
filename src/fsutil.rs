use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::cancel::Cancel;
use crate::error::{Error, Result};

const BUFFER: usize = 1024 * 1024;

/// SHA-256 of a file in large chunks (hardware-accelerated where available).
/// Fails when the file exceeds `limit` bytes.
pub fn hash_file(file: &Path, limit: u64, cancel: &Cancel) -> Result<String> {
    hash_reader(File::open(file)?, limit, cancel).map_err(|error| {
        if error.message == "limit" {
            Error::msg(format!("Backend file exceeds {} MiB: {}", limit / 1024 / 1024, file.display()))
        } else {
            error
        }
    })
}

pub fn hash_reader(mut reader: impl Read, limit: u64, cancel: &Cancel) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; BUFFER];
    let mut total = 0u64;
    loop {
        cancel.check()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        if total > limit {
            return Err(Error::msg("limit"));
        }
        hash.update(&buffer[..read]);
    }
    Ok(hex::encode(hash.finalize()))
}

/// Device and file number of an open file, like stat's dev/ino.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub dev: u64,
    pub ino: u64,
}

#[cfg(unix)]
pub fn identity(file: &File) -> Option<Identity> {
    use std::os::unix::fs::MetadataExt;
    file.metadata().ok().map(|info| Identity { dev: info.dev(), ino: info.ino() })
}

#[cfg(windows)]
pub fn identity(file: &File) -> Option<Identity> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid open handle and a zeroed out-parameter.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return None;
    }
    Some(Identity { dev: u64::from(info.dwVolumeSerialNumber), ino: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow) })
}

#[cfg(not(any(unix, windows)))]
pub fn identity(_file: &File) -> Option<Identity> {
    None
}

pub fn identity_of(path: &Path) -> Option<Identity> {
    File::open(path).ok().as_ref().and_then(identity)
}

/// Open for reading without following a final symlink where the OS allows.
pub fn open_nofollow(path: &Path) -> std::io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// A regular file the current user may run (readable on Windows).
pub fn is_executable(file: &Path) -> bool {
    let Ok(info) = fs::metadata(file) else { return false };
    if !info.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        let path = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(file.as_os_str()));
        // SAFETY: NUL-terminated path.
        path.is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0)
    }
    #[cfg(not(unix))]
    {
        File::open(file).is_ok()
    }
}

/// Like `rm -rf` with force: missing paths are fine.
pub fn remove_all(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(info) if info.is_dir() => fs::remove_dir_all(path).map_err(Error::from),
        Ok(_) => fs::remove_file(path).map_err(Error::from),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// A fresh private directory `<parent>/<prefix>XXXXXX`, like mkdtemp.
pub fn temp_dir(parent: &Path, prefix: &str) -> Result<std::path::PathBuf> {
    fs::create_dir_all(parent)?;
    for _ in 0..100 {
        let candidate = parent.join(format!("{prefix}{}", &hex::encode(crate::state::random_bytes::<4>())[..6]));
        match fs::create_dir(&candidate) {
            Ok(()) => {
                let _ = crate::state::set_mode(&candidate, 0o700);
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(Error::msg("Could not create a temporary directory."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_known_input() {
        let hash = hash_reader(&b"abc"[..], 10, &Cancel::new()).unwrap();
        assert_eq!(hash, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(hash_reader(&b"abcdef"[..], 3, &Cancel::new()).is_err());
    }
}
