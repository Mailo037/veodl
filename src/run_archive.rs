use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cancel::Cancel;
use crate::error::{optional, Error, Result};
use crate::fsutil;
use crate::paths;
use crate::state;

const SAMPLE_BYTES: u64 = 64 * 1024;
const MAX_SEARCH_FILES: usize = 200_000;
const MEDIA_EXTENSIONS: [&str; 14] = [".mp4", ".mkv", ".webm", ".mov", ".m4v", ".mp3", ".m4a", ".aac", ".opus", ".ogg", ".flac", ".wav", ".mka", ".ts"];

pub fn valid_id(id: &str) -> bool {
    re!(r"^[0-9a-z]{6}$").is_match(id)
}

pub fn run_archive_file(id: &str, root: &Path) -> Result<PathBuf> {
    if !valid_id(id) {
        return Err(Error::msg("Run ids have 6 letters or digits."));
    }
    Ok(paths::resolve(root).join("run-history").join(format!("{id}.json")))
}

pub fn is_media_file(file: &str) -> bool {
    MEDIA_EXTENSIONS.contains(&paths::extname(file).to_lowercase().as_str())
}

fn regular_file(file: &Path) -> Result<Option<fs::Metadata>> {
    Ok(optional(fs::symlink_metadata(file))?.filter(|info| info.is_file() && !info.file_type().is_symlink()))
}

/// A cheap identity of a media file: its size plus SHA-256 over three 64 KiB
/// samples, so renamed files are found without rereading a huge video.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub size: u64,
    pub sample_sha256: String,
    #[serde(default)]
    pub device: Value,
    #[serde(default)]
    pub inode: Value,
}

pub fn file_fingerprint(file: &Path, known: Option<&fs::Metadata>) -> Result<Option<Fingerprint>> {
    let size = match known {
        Some(info) => info.len(),
        None => match regular_file(file)? {
            Some(info) => info.len(),
            None => return Ok(None),
        },
    };
    let mut digest = Sha256::new();
    digest.update(format!("veo-file-v1:{size}:").as_bytes());
    let mut positions = vec![0, size.saturating_sub(SAMPLE_BYTES) / 2, size.saturating_sub(SAMPLE_BYTES)];
    positions.dedup();
    let mut seen = BTreeSet::new();
    positions.retain(|position| seen.insert(*position));
    let mut handle = File::open(file)?;
    for position in positions {
        let bytes = SAMPLE_BYTES.min(size - position) as usize;
        let mut buffer = vec![0u8; bytes];
        handle.seek(SeekFrom::Start(position))?;
        handle.read_exact(&mut buffer).map_err(|_| Error::msg(format!("File changed while recording its identity: {}", file.display())))?;
        digest.update(position.to_string().as_bytes());
        digest.update(b":");
        digest.update(&buffer);
    }
    let identity = fsutil::identity(&handle);
    Ok(Some(Fingerprint {
        size,
        sample_sha256: hex::encode(digest.finalize()),
        device: identity.map_or(Value::Null, |identity| identity.dev.into()),
        inode: identity.map_or(Value::Null, |identity| identity.ino.into()),
    }))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedFile {
    pub original_path: String,
    #[serde(default)]
    pub media: bool,
    #[serde(default)]
    pub fingerprint: Option<Fingerprint>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ArchivedItem {
    pub url: String,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    pub output: String,
    #[serde(default)]
    pub files: Vec<ArchivedFile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Archive {
    pub version: Value,
    pub id: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    pub status: String,
    pub items: Vec<ArchivedItem>,
}

/// One item of a run as the job records it, for archiving.
pub struct JobItemView<'a> {
    pub url: &'a str,
    pub status: &'a str,
    pub error: Option<&'a str>,
    pub output: String,
    pub files: &'a [String],
    pub fingerprints: &'a BTreeMap<String, Option<Fingerprint>>,
}

pub fn archive_run(id: &str, started_at: Option<&str>, items: &[JobItemView], root: &Path) -> Result<Option<Archive>> {
    if !valid_id(id) {
        return Ok(None);
    }
    let mut archived_items = Vec::new();
    for item in items {
        let mut files = Vec::new();
        for file in item.files {
            let media = is_media_file(file);
            let fingerprint = if media {
                match item.fingerprints.get(file) {
                    Some(Some(fingerprint)) => Some(fingerprint.clone()),
                    _ => file_fingerprint(Path::new(file), None).ok().flatten(),
                }
            } else {
                None
            };
            files.push(ArchivedFile { original_path: paths::display(paths::resolve(file)), media, fingerprint });
        }
        archived_items.push(ArchivedItem {
            url: item.url.into(),
            status: item.status.into(),
            error: item.error.map(str::to_string),
            output: paths::display(paths::resolve(&item.output)),
            files,
        });
    }
    let archive = Archive {
        version: 1.into(),
        id: id.into(),
        started_at: started_at.map(str::to_string),
        finished_at: Some(state::iso_now()),
        status: if archived_items.iter().all(|item| matches!(item.status.as_str(), "saved" | "skipped")) { "completed" } else { "incomplete" }.into(),
        items: archived_items,
    };
    let file = run_archive_file(id, root)?;
    if optional(fs::symlink_metadata(&file))?.is_some() {
        return Err(Error::msg(format!("Run archive {id} already exists; refusing to overwrite it.")));
    }
    state::write_json(&file, &archive)?;
    Ok(Some(archive))
}

pub fn read_run_archive(id: &str, root: &Path) -> Result<Archive> {
    let raw: Option<Value> = state::read_json(&run_archive_file(id, root)?)?;
    let raw = raw.ok_or_else(|| Error::msg(format!("No finished run with id {id} was found. Active runs appear in veo runs --json.")))?;
    let archive: Option<Archive> = serde_json::from_value(raw).ok();
    match archive {
        Some(archive) if archive.version.as_i64() == Some(1) && archive.id == id => Ok(archive),
        _ => Err(Error::msg(format!("Invalid archived run: {id}"))),
    }
}

fn scan(root: &Path, wanted: &BTreeSet<u64>, cancel: &Cancel) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    if regular_file(root)?.is_some() {
        return Err(Error::msg(format!("Search root is a file: {}", root.display())));
    }
    let Some(directory) = optional(fs::symlink_metadata(root))? else { return Ok(Vec::new()) };
    if !directory.is_dir() || directory.file_type().is_symlink() {
        return Err(Error::msg(format!("Search root is not a plain directory: {}", root.display())));
    }
    let mut candidates = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    let mut seen = 0usize;
    while let Some(current) = pending.pop() {
        cancel.check()?;
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let file = entry.path();
            if kind.is_dir() {
                pending.push(file);
            } else if kind.is_file() {
                seen += 1;
                if seen > MAX_SEARCH_FILES {
                    return Err(Error::msg(format!("Too many files under {}; use --search with a narrower directory.", root.display())));
                }
                if let Some(details) = regular_file(&file)? {
                    if wanted.contains(&details.len()) {
                        candidates.push((file, details));
                    }
                }
            }
        }
    }
    Ok(candidates)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocatedFile {
    pub original_path: String,
    pub path: Option<String>,
    pub found: bool,
    pub renamed: bool,
    pub media: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LocatedItem {
    pub url: String,
    pub status: String,
    pub error: Option<String>,
    pub files: Vec<LocatedFile>,
}

fn fingerprint_or_none(file: &Path, info: &fs::Metadata) -> Result<Option<Fingerprint>> {
    match file_fingerprint(file, Some(info)) {
        Ok(value) => Ok(value),
        Err(error) if error.not_found() => Ok(None),
        Err(error) => Err(error),
    }
}

/// Resolve original paths first, then use fingerprints to find renamed media.
pub fn locate_run_files(archive: &Archive, search_root: Option<&Path>, cancel: &Cancel) -> Result<Vec<LocatedItem>> {
    let mut items = Vec::new();
    let mut missing: Vec<(usize, usize, Fingerprint, String)> = Vec::new();
    for item in &archive.items {
        cancel.check()?;
        let mut files = Vec::new();
        for entry in &item.files {
            let details = regular_file(Path::new(&entry.original_path))?;
            let valid = match (&details, &entry.fingerprint) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(info), Some(expected)) => fingerprint_or_none(Path::new(&entry.original_path), info)?
                    .is_some_and(|current| current.size == expected.size && current.sample_sha256 == expected.sample_sha256),
            };
            if !valid && entry.media {
                if let Some(fingerprint) = &entry.fingerprint {
                    missing.push((items.len(), files.len(), fingerprint.clone(), item.output.clone()));
                }
            }
            files.push(LocatedFile {
                original_path: entry.original_path.clone(),
                path: valid.then(|| entry.original_path.clone()),
                found: valid,
                renamed: false,
                media: entry.media,
                candidates: None,
            });
        }
        items.push(LocatedItem { url: item.url.clone(), status: item.status.clone(), error: item.error.clone(), files });
    }
    if missing.is_empty() {
        return Ok(items);
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in missing.iter().map(|(_, _, _, output)| PathBuf::from(output)).chain(search_root.map(paths::resolve)) {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    let wanted: BTreeSet<u64> = missing.iter().map(|(_, _, fingerprint, _)| fingerprint.size).collect();
    let mut matches: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for root in &roots {
        for (file, details) in scan(root, &wanted, cancel)? {
            cancel.check()?;
            let Some(fingerprint) = fingerprint_or_none(&file, &details)? else { continue };
            matches.entry(format!("{}:{}", fingerprint.size, fingerprint.sample_sha256)).or_default().insert(paths::display(&file));
        }
    }
    for (item, index, fingerprint, _) in missing {
        let key = format!("{}:{}", fingerprint.size, fingerprint.sample_sha256);
        let found: Vec<String> = matches.get(&key).map(|set| set.iter().cloned().collect()).unwrap_or_default();
        let mut selected = (found.len() == 1).then(|| found[0].clone());
        let inode = fingerprint.inode.as_u64().unwrap_or(0);
        if selected.is_none() && found.len() > 1 && inode > 0 {
            let same: Vec<&String> = found
                .iter()
                .filter(|file| {
                    fsutil::identity_of(Path::new(file)).is_some_and(|identity| Some(identity.dev) == fingerprint.device.as_u64() && identity.ino == inode)
                })
                .collect();
            if same.len() == 1 {
                selected = Some(same[0].clone());
            }
        }
        let target = &mut items[item].files[index];
        if let Some(selected) = selected {
            target.renamed = selected != target.original_path;
            target.path = Some(selected);
            target.found = true;
        } else if found.len() > 1 {
            target.candidates = Some(found);
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_find_renamed_media() {
        let root = fsutil::temp_dir(&std::env::temp_dir(), "veo-archive-").unwrap();
        let output = root.join("out");
        fs::create_dir_all(&output).unwrap();
        let original = output.join("video.mp4");
        fs::write(&original, vec![7u8; 200_000]).unwrap();
        let fingerprints: BTreeMap<String, Option<Fingerprint>> = BTreeMap::new();
        let files = vec![paths::display(&original)];
        let item = JobItemView {
            url: "https://example.com/v",
            status: "saved",
            error: None,
            output: paths::display(&output),
            files: &files,
            fingerprints: &fingerprints,
        };
        let cache = root.join("cache");
        archive_run("abc123", Some("2026-01-01T00:00:00.000Z"), &[item], &cache).unwrap();
        fs::rename(&original, output.join("renamed.mp4")).unwrap();
        let archive = read_run_archive("abc123", &cache).unwrap();
        assert_eq!(archive.status, "completed");
        let located = locate_run_files(&archive, None, &Cancel::new()).unwrap();
        let file = &located[0].files[0];
        assert!(file.found && file.renamed, "{file:?}");
        assert!(file.path.as_ref().unwrap().ends_with("renamed.mp4"));
        let _ = fs::remove_dir_all(root);
    }
}
