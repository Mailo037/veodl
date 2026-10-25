use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::backend::{self, MediaHooks};
use crate::cancel::Cancel;
use crate::error::{Error, Result};
use crate::output::Out;
use crate::paths::{self, exe_suffix};
use crate::process::{self, Command};
use crate::run_archive::{self, locate_run_files, read_run_archive};
use crate::terminal::Std;

const SIGNAL_THRESHOLD_DBFS: f64 = -60.0;
const OUTPUT_LIMIT: usize = 2 * 1024 * 1024;

pub const INSPECT_HELP: &str = "veo inspect <media-file> [--check-audio] [--json]
veo inspect run <id> [--check-audio] [--search <directory>] [--json]

Read local media metadata with ffprobe. --check-audio decodes every audio track
from start to end and measures its peak level with FFmpeg. A signal is detected
when the peak is above -60 dBFS; this is a level check, not a listening test.
Inspect a finished run by its persistent id. Missing media is searched by its
file fingerprint under the original output directory; --search adds another
directory after a move. Missing files are reported before media probing.
Inspection uses locally available media tools and does not contact the network.
";

pub struct ToolOutput {
    pub stdout: String,
    pub stderr: String,
}

pub fn run_media_tool(executable: &Path, args: &[String], cancel: &Cancel) -> Result<ToolOutput> {
    let captured =
        process::capture(&Command::new(executable).args(args.iter().cloned()), cancel, OUTPUT_LIMIT, "Media tool output exceeded the supported size.")?;
    if captured.code != Some(0) {
        let last = captured.stderr.trim().lines().last().map(str::to_string);
        return Err(Error::msg(
            last.filter(|line| !line.is_empty())
                .unwrap_or_else(|| format!("Media tool exited with code {}.", captured.code.map_or("null".into(), |code| code.to_string()))),
        ));
    }
    Ok(ToolOutput { stdout: captured.stdout, stderr: captured.stderr })
}

fn local_media_tools(cancel: &Cancel) -> Result<(PathBuf, PathBuf)> {
    let directory = backend::cache_directory(backend::RELEASE);
    fs::create_dir_all(&directory)?;
    let location = backend::resolve_media_tools(&directory, true, cancel, &|_| {}, &|_, _| {}, &MediaHooks::default())?;
    Ok((location.join(format!("ffmpeg{}", exe_suffix())), location.join(format!("ffprobe{}", exe_suffix()))))
}

fn number(value: Option<&Value>) -> Value {
    let parsed = match value {
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::String(text)) if !text.is_empty() => text.trim().parse::<f64>().ok(),
        _ => None,
    };
    match parsed.filter(|value| value.is_finite() && *value >= 0.0) {
        Some(value) => crate::downloader::number(value),
        None => Value::Null,
    }
}

pub fn parse_peak(output: &str) -> Result<Option<f64>> {
    let captures =
        re!(r"(?i)max_volume:\s*(-inf|-?[0-9]+(?:\.[0-9]+)?)\s*dB").captures(output).ok_or_else(|| Error::msg("FFmpeg did not report an audio peak level."))?;
    Ok(if captures[1].eq_ignore_ascii_case("-inf") { None } else { captures[1].parse().ok() })
}

pub type Tools<'a> = &'a dyn Fn(&Cancel) -> Result<(PathBuf, PathBuf)>;
pub type MediaRunner<'a> = &'a dyn Fn(&Path, &[String], &Cancel) -> Result<ToolOutput>;

pub fn inspect_media_with(file: &Path, check_audio: bool, cancel: &Cancel, runner: MediaRunner, tools: Tools) -> Result<Map<String, Value>> {
    let target = paths::resolve(file);
    if !fs::metadata(&target)?.is_file() {
        return Err(Error::msg(format!("Not a regular media file: {}", target.display())));
    }
    let (ffmpeg, ffprobe) = tools(cancel)?;
    let shown = paths::display(&target);
    let probe = runner(&ffprobe, &["-v", "error", "-show_format", "-show_streams", "-of", "json", "-i", &shown].map(str::to_string), cancel)?;
    let metadata: Value = serde_json::from_str(&probe.stdout).map_err(|_| Error::msg(format!("ffprobe returned invalid metadata for {shown}.")))?;
    let Some(raw_streams) = metadata.get("streams").and_then(Value::as_array) else {
        return Err(Error::msg(format!("ffprobe found no stream metadata for {shown}.")));
    };
    let text = |value: Option<&Value>| value.and_then(Value::as_str).filter(|text| !text.is_empty()).map_or(Value::Null, |text| Value::String(text.into()));
    let streams: Vec<Value> = raw_streams
        .iter()
        .map(|stream| {
            json!({
                "index": stream.get("index").cloned().unwrap_or(Value::Null),
                "type": text(stream.get("codec_type")),
                "codec": text(stream.get("codec_name")),
                "language": text(stream.pointer("/tags/language")),
                "width": number(stream.get("width")),
                "height": number(stream.get("height")),
                "channels": number(stream.get("channels")),
                "sampleRate": number(stream.get("sample_rate")),
            })
        })
        .collect();
    let audio: Vec<&Value> = streams.iter().filter(|stream| stream["type"] == "audio").collect();
    let mut result = Map::new();
    result.insert("file".into(), Value::String(shown.clone()));
    result.insert("format".into(), text(metadata.pointer("/format/format_name")));
    result.insert("durationSeconds".into(), number(metadata.pointer("/format/duration")));
    result.insert("sizeBytes".into(), number(metadata.pointer("/format/size")));
    result.insert("hasAudioTrack".into(), Value::Bool(!audio.is_empty()));
    result.insert("hasVideoTrack".into(), Value::Bool(streams.iter().any(|stream| stream["type"] == "video")));
    let mut audio_check = Value::Null;
    if check_audio {
        let mut tracks = Vec::new();
        for (audio_index, stream) in audio.iter().enumerate() {
            let args = [
                "-hide_banner",
                "-nostdin",
                "-nostats",
                "-i",
                &shown,
                "-map",
                &format!("0:a:{audio_index}"),
                "-vn",
                "-sn",
                "-dn",
                "-af",
                "volumedetect",
                "-f",
                "null",
                "-",
            ]
            .map(str::to_string);
            let report = runner(&ffmpeg, &args, cancel)?;
            let peak = parse_peak(&report.stderr)?;
            tracks.push(json!({ "streamIndex": stream["index"], "maxDbfs": peak.map_or(Value::Null, crate::downloader::number), "hasSignal": peak.is_some_and(|peak| peak > SIGNAL_THRESHOLD_DBFS) }));
        }
        let has_signal = tracks.iter().any(|track| track["hasSignal"] == true);
        audio_check = json!({ "checked": true, "scope": "full_file", "thresholdDbfs": -60, "hasSignal": has_signal, "tracks": tracks });
    }
    // Keep the published field order: streams before the track flags.
    let mut ordered = Map::new();
    for key in ["file", "format", "durationSeconds", "sizeBytes"] {
        ordered.insert(key.into(), result.remove(key).unwrap_or(Value::Null));
    }
    ordered.insert("streams".into(), Value::Array(streams));
    for key in ["hasAudioTrack", "hasVideoTrack"] {
        ordered.insert(key.into(), result.remove(key).unwrap_or(Value::Null));
    }
    ordered.insert("audioCheck".into(), audio_check);
    Ok(ordered)
}

fn default_runner(executable: &Path, args: &[String], cancel: &Cancel) -> Result<ToolOutput> {
    run_media_tool(executable, args, cancel)
}

pub fn inspect_media(file: &Path, check_audio: bool, cancel: &Cancel) -> Result<Map<String, Value>> {
    inspect_media_with(file, check_audio, cancel, &default_runner, &local_media_tools)
}

/// A fast post-download container/stream check; it does not decode the file.
pub fn verify_saved_media(file: &Path, audio: bool, cancel: &Cancel) -> Result<Map<String, Value>> {
    let report = inspect_media(file, false, cancel).map_err(|error| Error::msg(format!("Verification failed for {}: {}", file.display(), error.message)))?;
    let key = if audio { "hasAudioTrack" } else { "hasVideoTrack" };
    if report.get(key) != Some(&Value::Bool(true)) {
        return Err(Error::msg(format!("Verification failed for {}: no {} track found.", file.display(), if audio { "audio" } else { "video" })));
    }
    Ok(report)
}

/// Reuse a successful compatibility probe only when saving kept the exact file
/// identity (a hard link). Copies and converted outputs still get a fresh probe.
pub fn verify_probe(probe: &Value, file: &Path, audio: bool) -> Result<()> {
    let wanted = if audio { "audio" } else { "video" };
    if !probe.get("streams").and_then(Value::as_array).is_some_and(|streams| streams.iter().any(|stream| stream["codec_type"] == wanted)) {
        return Err(Error::msg(format!("Verification failed for {}: no {wanted} track found.", file.display())));
    }
    Ok(())
}

pub fn inspect_run(
    id: &str,
    check_audio: bool,
    search_root: Option<&Path>,
    cancel: &Cancel,
    root: &Path,
    inspect: &dyn Fn(&Path, bool, &Cancel) -> Result<Map<String, Value>>,
) -> Result<Value> {
    if !run_archive::valid_id(id) {
        return Err(Error::msg("Run ids have 6 letters or digits. List active runs with veo runs --json or finished attempts with veo history --json."));
    }
    let archive = read_run_archive(id, root)?;
    let located = locate_run_files(&archive, search_root, cancel)?;
    let mut missing_files = 0;
    let mut items = Vec::new();
    for item in located {
        let mut files = Vec::new();
        for mut file in item.files {
            if !file.found {
                missing_files += 1;
            }
            let mut metadata = Value::Null;
            if file.found && file.media {
                match inspect(Path::new(file.path.as_deref().unwrap_or_default()), check_audio, cancel) {
                    Ok(report) => metadata = Value::Object(report),
                    Err(error) if error.not_found() => {
                        file.found = false;
                        file.path = None;
                        missing_files += 1;
                    }
                    Err(error) => return Err(error),
                }
            }
            let mut value = serde_json::to_value(&file)?;
            value["metadata"] = metadata;
            files.push(value);
        }
        items.push(json!({ "url": item.url, "status": item.status, "error": item.error, "files": files }));
    }
    Ok(
        json!({ "runId": archive.id, "status": archive.status, "startedAt": archive.started_at, "finishedAt": archive.finished_at, "missingFiles": missing_files, "items": items }),
    )
}

pub fn inspect_main(args: &[String]) -> Result<i32> {
    let stdout = Out::command(Std::Out, args);
    let args = crate::output::strip_color_flags(args);
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        stdout.write(INSPECT_HELP);
        return Ok(0);
    }
    let json = args.iter().any(|arg| arg == "--json");
    let check_audio = args.iter().any(|arg| arg == "--check-audio");
    let run_mode = args.first().is_some_and(|arg| arg == "run");
    let mut search_root: Option<String> = None;
    let mut positionals: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--json" || arg == "--check-audio" {
            index += 1;
            continue;
        }
        if arg == "--search" && run_mode && search_root.is_none() {
            search_root = args.get(index + 1).cloned();
            index += 2;
            continue;
        }
        positionals.push(arg.clone());
        index += 1;
    }
    let usage = "Usage: veo inspect <media-file> [--check-audio] [--json] | veo inspect run <id> [--check-audio] [--search <directory>] [--json]";
    let invalid = if run_mode {
        positionals.len() != 2
            || positionals[0] != "run"
            || !run_archive::valid_id(&positionals[1])
            || (args.iter().any(|arg| arg == "--search") && search_root.as_ref().is_none_or(|root| root.starts_with("--")))
    } else {
        positionals.len() != 1 || positionals[0].starts_with("--")
    };
    if invalid {
        return Err(Error::msg(usage));
    }
    let cancel = Cancel::global();
    if run_mode {
        let result = inspect_run(&positionals[1], check_audio, search_root.as_deref().map(Path::new), cancel, &paths::cache_base(), &inspect_media)?;
        if json {
            stdout.line(serde_json::to_string(&result)?);
        } else {
            stdout.line(format!("Run: {} ({})", result["runId"].as_str().unwrap_or(""), result["status"].as_str().unwrap_or("")));
            for item in result["items"].as_array().into_iter().flatten() {
                for file in item["files"].as_array().into_iter().flatten() {
                    let found = file["found"] == true;
                    let path = file["path"].as_str().or(file["originalPath"].as_str()).unwrap_or("");
                    let renamed =
                        if file["renamed"] == true { format!(" (renamed from {})", file["originalPath"].as_str().unwrap_or("")) } else { String::new() };
                    stdout.line(format!("{}: {path}{renamed}", if found { "File" } else { "Missing" }));
                    if let Some(metadata) = file["metadata"].as_object() {
                        let tracks = metadata["streams"].as_array().map_or(0, |streams| streams.iter().filter(|stream| stream["type"] == "audio").count());
                        let signal = metadata
                            .get("audioCheck")
                            .and_then(Value::as_object)
                            .map(|check| format!("; signal: {}", if check["hasSignal"] == true { "detected" } else { "not detected" }))
                            .unwrap_or_default();
                        stdout.line(format!("  Format: {}; audio tracks: {tracks}{signal}", metadata["format"].as_str().unwrap_or("unknown")));
                    }
                }
            }
        }
        return Ok(if result["missingFiles"].as_u64().unwrap_or(0) > 0 { 1 } else { 0 });
    }
    let result = inspect_media(Path::new(&positionals[0]), check_audio, cancel)?;
    if json {
        stdout.line(serde_json::to_string(&result)?);
        return Ok(0);
    }
    let streams = result["streams"].as_array().cloned().unwrap_or_default();
    stdout.line(format!("File: {}", result["file"].as_str().unwrap_or("")));
    let duration = if result["durationSeconds"].is_null() { "unknown".to_string() } else { result["durationSeconds"].to_string() };
    stdout.line(format!("Format: {}; duration: {duration} s", result["format"].as_str().unwrap_or("unknown")));
    let listed = streams
        .iter()
        .map(|stream| format!("{} {}", stream["type"].as_str().unwrap_or("unknown"), stream["codec"].as_str().unwrap_or("unknown")))
        .collect::<Vec<_>>()
        .join(", ");
    stdout.line(format!("Streams: {}", if listed.is_empty() { "none".into() } else { listed }));
    stdout.line(format!("Audio tracks: {}", streams.iter().filter(|stream| stream["type"] == "audio").count()));
    if let Some(check) = result.get("audioCheck").and_then(Value::as_object) {
        stdout.line(format!("Audio signal: {} (threshold -60 dBFS, full file)", if check["hasSignal"] == true { "detected" } else { "not detected" }));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_peaks() {
        assert_eq!(parse_peak("[Parsed_volumedetect_0] max_volume: -3.2 dB").unwrap(), Some(-3.2));
        assert_eq!(parse_peak("max_volume: -inf dB").unwrap(), None);
        assert!(parse_peak("nothing").is_err());
    }

    #[test]
    fn reports_tracks_and_signal() {
        let file = std::env::temp_dir().join(format!("veo-inspect-{}.mp4", crate::state::uuid()));
        fs::write(&file, b"x").unwrap();
        let runner = |executable: &Path, _: &[String], _: &Cancel| -> Result<ToolOutput> {
            if executable.ends_with("ffprobe") {
                Ok(ToolOutput { stdout: r#"{"streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":640},{"index":1,"codec_type":"audio","codec_name":"aac","sample_rate":"48000","tags":{"language":"eng"}}],"format":{"format_name":"mov,mp4","duration":"2.000000","size":"1234"}}"#.into(), stderr: String::new() })
            } else {
                Ok(ToolOutput { stdout: String::new(), stderr: "max_volume: -20.0 dB".into() })
            }
        };
        let tools = |_: &Cancel| Ok((PathBuf::from("ffmpeg"), PathBuf::from("ffprobe")));
        let report = inspect_media_with(&file, true, &Cancel::new(), &runner, &tools).unwrap();
        assert_eq!(report["durationSeconds"], json!(2));
        assert_eq!(report["streams"][1]["sampleRate"], json!(48000));
        assert_eq!(report["streams"][1]["language"], json!("eng"));
        assert_eq!(report["audioCheck"]["hasSignal"], json!(true));
        assert_eq!(
            report.keys().cloned().collect::<Vec<_>>(),
            ["file", "format", "durationSeconds", "sizeBytes", "streams", "hasAudioTrack", "hasVideoTrack", "audioCheck"]
        );
        let _ = fs::remove_file(file);
    }
}
