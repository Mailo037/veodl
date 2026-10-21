use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use crate::error::{Error, Result};
use crate::metadata::{positive, Format, Metadata};
use crate::options::Options;
use crate::paths;
use crate::progress::{to_fixed, Reporter};

static RESERVATIONS: Mutex<Option<HashMap<String, f64>>> = Mutex::new(None);

pub fn estimate_media_bytes(metadata: &Metadata, options: &Options) -> Option<f64> {
    let size = |direct: Option<f64>, approx: Option<f64>, tbr: Option<f64>| -> f64 {
        if let Some(direct) = positive(direct).or(positive(approx)) {
            return direct;
        }
        match (tbr.filter(|tbr| tbr.is_finite()), positive(metadata.duration)) {
            (Some(tbr), Some(duration)) => tbr * 1000.0 / 8.0 * duration,
            _ => 0.0,
        }
    };
    let own = size(metadata.filesize, metadata.filesize_approx, metadata.tbr);
    if own != 0.0 {
        return Some(own.ceil());
    }
    let target = if options.quality != "best" { crate::utils::quality_target(&options.quality) as f64 } else { f64::INFINITY };
    let formats: Vec<&Format> = metadata
        .formats()
        .iter()
        .filter(|format| !format.drm() && (options.closest_quality || positive(format.height).is_none_or(|height| height <= target)))
        .collect();
    let format_size = |format: &&Format| size(format.filesize, format.filesize_approx, format.tbr);
    let best_video = formats.iter().filter(|format| format.is_video()).map(format_size).fold(0.0, f64::max);
    let best_audio = formats.iter().filter(|format| !format.is_video() && format.acodec.as_deref() != Some("none")).map(format_size).fold(0.0, f64::max);
    if options.audio {
        return (best_audio > 0.0).then(|| best_audio.ceil());
    }
    if best_video <= 0.0 {
        return None;
    }
    // Separate video needs an audio estimate too; otherwise the total is unknown.
    if formats.iter().any(|format| format.is_video() && format.acodec.as_deref() == Some("none")) && best_audio <= 0.0 {
        return None;
    }
    Some((best_video + best_audio).ceil())
}

pub struct Volume {
    pub key: String,
    pub free: f64,
}

#[cfg(unix)]
fn free_space(directory: &Path) -> Result<Volume> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    let info = fs::metadata(directory)?;
    let path = std::ffi::CString::new(directory.as_os_str().as_bytes()).map_err(|_| Error::msg("invalid path"))?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid NUL-terminated path and a zeroed out-parameter.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(Volume { key: info.dev().to_string(), free: stats.f_bavail as f64 * stats.f_frsize as f64 })
}

#[cfg(windows)]
fn free_space(directory: &Path) -> Result<Volume> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumePathNameW};
    let wide: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    // SAFETY: NUL-terminated path and valid out-pointers.
    if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut volume = vec![0u16; 1024];
    // SAFETY: the buffer length is passed alongside the buffer.
    let key = if unsafe { GetVolumePathNameW(wide.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) } != 0 {
        String::from_utf16_lossy(&volume[..volume.iter().position(|c| *c == 0).unwrap_or(0)]).to_lowercase()
    } else {
        paths::display(directory)
    };
    Ok(Volume { key, free: available as f64 })
}

fn volume(directory: &Path) -> Result<Volume> {
    let mut current = paths::resolve(directory);
    loop {
        if fs::metadata(&current).is_ok_and(|info| info.is_dir()) {
            return free_space(&current);
        }
        let parent = paths::dirname(&current);
        if parent == current {
            return Err(Error::msg("No existing parent directory."));
        }
        current = parent;
    }
}

/// Releases a space reservation when dropped.
pub struct Reservation {
    needs: Vec<(String, f64)>,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut guard = RESERVATIONS.lock().unwrap_or_else(|poison| poison.into_inner());
        let map = guard.get_or_insert_with(HashMap::new);
        for (key, bytes) in &self.needs {
            let remaining = map.get(key).copied().unwrap_or(0.0) - bytes;
            if remaining > 0.0 {
                map.insert(key.clone(), remaining);
            } else {
                map.remove(key);
            }
        }
    }
}

fn gib(bytes: f64) -> String {
    to_fixed(bytes / 1_073_741_824.0, 1)
}

/// Check that cache and destination can hold the download (including the
/// temporary merge output) and reserve that space against parallel downloads.
pub fn reserve_space(cache: &Path, destination: &Path, bytes: Option<f64>, cached: bool, reporter: &dyn Reporter) -> Result<Reservation> {
    let (cache_volume, output_volume) = match (volume(cache), volume(destination)) {
        (Ok(cache), Ok(output)) => (cache, output),
        _ => {
            reporter.status("Free-space check unavailable on this filesystem; continuing.");
            return Ok(Reservation { needs: Vec::new() });
        }
    };
    let Some(bytes) = bytes.filter(|bytes| bytes.is_finite() && *bytes > 0.0) else {
        reporter.status(&format!("Size unknown; free space: cache {} GiB, destination {} GiB.", gib(cache_volume.free), gib(output_volume.free)));
        return Ok(Reservation { needs: Vec::new() });
    };
    let mut needs: Vec<(String, f64, f64)> = Vec::new();
    for (volume, amount) in [(&cache_volume, if cached { 0.0 } else { bytes * 2.0 }), (&output_volume, bytes)] {
        match needs.iter_mut().find(|(key, _, _)| *key == volume.key) {
            Some(entry) => {
                entry.1 = entry.1.min(volume.free);
                entry.2 += amount;
            }
            None => needs.push((volume.key.clone(), volume.free, amount)),
        }
    }
    let mut guard = RESERVATIONS.lock().unwrap_or_else(|poison| poison.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    for (key, free, need) in &needs {
        let available = free - map.get(key).copied().unwrap_or(0.0);
        if available < *need {
            return Err(Error::msg(format!(
                "Not enough disk space: approximately {} MiB needed including temporary files; {} MiB available after active downloads.",
                to_fixed(need / 1_048_576.0, 1),
                to_fixed(available.max(0.0) / 1_048_576.0, 1)
            )));
        }
    }
    for (key, _, need) in &needs {
        *map.entry(key.clone()).or_insert(0.0) += need;
    }
    Ok(Reservation { needs: needs.into_iter().map(|(key, _, need)| (key, need)).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_separate_streams() {
        let metadata: Metadata = serde_json::from_str(r#"{"duration":10,"formats":[{"height":1080,"vcodec":"avc1","acodec":"none","filesize":1000},{"height":2160,"vcodec":"vp9","acodec":"none","filesize":9000},{"vcodec":"none","acodec":"mp4a","tbr":8}]}"#).unwrap();
        let options = Options { quality: "1080p".into(), ..Options::default() };
        assert_eq!(estimate_media_bytes(&metadata, &options), Some(11000.0));
        let audio = Options { quality: "best".into(), audio: true, ..Options::default() };
        assert_eq!(estimate_media_bytes(&metadata, &audio), Some(10000.0));
    }
}
