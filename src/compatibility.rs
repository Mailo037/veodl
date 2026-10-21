use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::backend::Backend;
use crate::cancel::Cancel;
use crate::downloader::Runner;
use crate::error::{Error, Result};
use crate::paths;
use crate::progress::Reporter;

/// Check that a saved video plays broadly (H.264 yuv420p + AAC). Without
/// `convert` only a note is shown; with it the file becomes a compatible MP4,
/// re-encoding only the streams that need it.
pub fn ensure_compatibility(file: &Path, backend: &Backend, runner: &Runner, cancel: &Cancel, convert: bool, reporter: &dyn Reporter) -> Result<PathBuf> {
    let probe = runner(
        &backend.ffprobe(),
        &["-v", "error", "-show_streams", "-of", "json"].iter().map(|arg| arg.to_string()).chain([paths::display(file)]).collect::<Vec<_>>(),
        cancel,
        None,
    );
    let streams = probe.and_then(|raw| {
        let parsed: Value = serde_json::from_str(&raw)?;
        match parsed.get("streams") {
            Some(Value::Array(streams)) => Ok(streams.clone()),
            _ => Err(Error::msg("No stream metadata")),
        }
    });
    let streams = match streams {
        Ok(streams) => streams,
        Err(error) => {
            if cancel.is_cancelled() || convert {
                return Err(error);
            }
            reporter.status("Playback compatibility could not be checked; original media retained.");
            return Ok(file.to_path_buf());
        }
    };
    let field = |stream: &Value, name: &str| stream.get(name).and_then(Value::as_str).map(str::to_string);
    let video = streams.iter().find(|stream| {
        field(stream, "codec_type").as_deref() == Some("video")
            && !stream.pointer("/disposition/attached_pic").is_some_and(|value| value.as_i64().unwrap_or(0) != 0 || value.as_bool() == Some(true))
    });
    let audio: Vec<&Value> = streams.iter().filter(|stream| field(stream, "codec_type").as_deref() == Some("audio")).collect();
    let Some(video) = video else { return Ok(file.to_path_buf()) };
    let video_ok = field(video, "codec_name").as_deref() == Some("h264") && field(video, "pix_fmt").as_deref() == Some("yuv420p");
    let audio_ok = audio.iter().all(|stream| field(stream, "codec_name").as_deref() == Some("aac"));
    if !convert {
        if !video_ok || !audio_ok {
            let codecs = audio.iter().map(|stream| field(stream, "codec_name").unwrap_or_default()).collect::<Vec<_>>().join(",");
            reporter.status(&format!(
                "Playback note: {}/{} may require additional player codecs. Original quality retained; use --compatible for H.264/AAC conversion.",
                field(video, "codec_name").unwrap_or_else(|| "undefined".into()),
                if codecs.is_empty() { "no audio".into() } else { codecs }
            ));
        }
        return Ok(file.to_path_buf());
    }
    if video_ok && audio_ok && paths::extname(file) == ".mp4" {
        return Ok(file.to_path_buf());
    }
    let transfer = field(video, "color_transfer");
    if !video_ok && matches!(transfer.as_deref(), Some("smpte2084" | "arib-std-b67")) {
        return Err(Error::msg("HDR conversion needs tone mapping; original download retained. Use an HDR-capable player or disable --compatible."));
    }
    reporter.status(if video_ok && audio_ok {
        "Preparing compatible MP4 without re-encoding…"
    } else {
        "Converting to compatible H.264/AAC; this takes time and may lose quality…"
    });
    let directory = paths::dirname(file);
    let temporary = directory.join("compatible-output.mp4");
    let target = directory.join("media.mp4");
    let mut args: Vec<String> = ["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"].iter().map(|arg| arg.to_string()).collect();
    args.push(paths::display(file));
    args.extend(["-map", "0:v:0", "-map", "0:a?", "-map_metadata", "0", "-c:v"].iter().map(|arg| arg.to_string()));
    if video_ok {
        args.push("copy".into());
    } else {
        args.extend(["libx264", "-preset", "fast", "-crf", "18", "-pix_fmt", "yuv420p"].iter().map(|arg| arg.to_string()));
    }
    args.push("-c:a".into());
    if audio_ok {
        args.push("copy".into());
    } else {
        args.extend(["aac", "-b:a", "192k"].iter().map(|arg| arg.to_string()));
    }
    args.extend(["-movflags", "+faststart"].iter().map(|arg| arg.to_string()));
    args.push(paths::display(&temporary));
    let result = (|| -> Result<PathBuf> {
        runner(&backend.ffmpeg(), &args, cancel, None)?;
        fs::rename(&temporary, &target)?;
        if file != target {
            fs::remove_file(file)?;
        }
        Ok(target.clone())
    })();
    let _ = fs::remove_file(&temporary);
    result
}
