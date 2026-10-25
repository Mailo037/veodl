//! Behaves like yt-dlp, ffmpeg or ffprobe depending on its file name, with
//! fixed fixture data and no network access. The URL path selects a scenario:
//! `/missing` fails with HTTP 404, `/busy` fails once with HTTP 503,
//! `/playlist` is a three-entry collection, `/page` is an unsupported page,
//! anything else is a 720p/360p video with English subtitles.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn value_after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|arg| arg == flag).and_then(|index| args.get(index + 1)).map(String::as_str)
}

const FORMATS: &str = r#"[{"format_id":"18","height":360,"vcodec":"avc1","acodec":"mp4a","filesize":1024,"ext":"mp4","tbr":500},{"format_id":"22","height":720,"vcodec":"avc1","acodec":"mp4a","filesize":2048,"ext":"mp4","tbr":1000}]"#;

fn entry(index: usize) -> String {
    let titles = ["First", "Second", "Third"];
    format!(
        r#"{{"id":"e{index}","title":"{}","url":"https://example.com/e{index}","playlist_index":{index},"filesize":1000,"extractor_key":"Generic","formats":{FORMATS}}}"#,
        titles[index - 1]
    )
}

fn state_file(name: &str) -> PathBuf {
    env::temp_dir().join(format!("veo-fake-{name}"))
}

fn yt_dlp(args: &[String]) {
    if let Ok(log) = env::var("VEO_FAKE_LOG") {
        use std::io::Write;
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(file, "{}", serde_json::to_string(args).unwrap());
        }
    }
    if args.iter().any(|arg| arg == "--version") {
        println!("2026.08.19");
        return;
    }
    let url = match value_after(args, "--load-info-json") {
        Some(file) => {
            let info: serde_json::Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
            info["webpage_url"].as_str().unwrap_or("https://example.com/v").to_string()
        }
        None => args.last().cloned().unwrap_or_default(),
    };
    if url.contains("/missing") {
        eprintln!("ERROR: [generic] Unable to download webpage: HTTP Error 404: Not Found");
        exit(1);
    }
    if url.contains("/page") {
        eprintln!("ERROR: Unsupported URL: {url}");
        exit(1);
    }
    if url.contains("/expired") && value_after(args, "--load-info-json").is_some() {
        eprintln!("ERROR: HTTP Error 403: Expired media URL");
        exit(1);
    }
    if url.contains("/busy") {
        let marker = state_file("busy");
        if !marker.exists() {
            let _ = fs::write(&marker, "1");
            eprintln!("ERROR: HTTP Error 503: Service Unavailable");
            exit(1);
        }
        let _ = fs::remove_file(marker);
    }
    let playlist = url.contains("/playlist");
    let item = value_after(args, "--playlist-items").and_then(|value| value.parse::<usize>().ok());
    if args.iter().any(|arg| arg == "--dump-single-json") {
        if playlist {
            let entries: Vec<String> = match item {
                Some(index) => vec![entry(index)],
                None => (1..=3).map(entry).collect(),
            };
            println!(r#"{{"_type":"playlist","id":"list","title":"Fixture List","extractor_key":"Generic","entries":[{}]}}"#, entries.join(","));
        } else if let Some(index) = url
            .rsplit('/')
            .next()
            .and_then(|value| value.strip_prefix('e'))
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|index| (1..=3).contains(index))
        {
            let mut info: serde_json::Value = serde_json::from_str(&entry(index)).unwrap();
            info["webpage_url"] = serde_json::Value::String(url.clone());
            println!("{info}");
        } else {
            println!(
                r#"{{"id":"vid1","webpage_url":{},"title":"Fixture Video","extractor":"generic","extractor_key":"Generic","channel":"Fixture Channel","upload_date":"20240102","formats":{FORMATS},"subtitles":{{"en":[{{"ext":"vtt","name":"English"}},{{"ext":"srt"}}],"de":[{{"ext":"vtt"}}]}},"automatic_captions":{{}}}}"#,
                quote(&url)
            );
        }
        return;
    }
    if args.iter().any(|arg| arg == "-F") {
        println!("ID  EXT RESOLUTION\n18  mp4 640x360\n22  mp4 1280x720");
        return;
    }
    let Some(template) = value_after(args, "-o") else {
        eprintln!("ERROR: no output template");
        exit(2);
    };
    let audio = args.iter().any(|arg| arg == "--extract-audio");
    let extension = if audio {
        value_after(args, "--audio-format").unwrap_or("mp3")
    } else {
        value_after(args, "--remux-video").or_else(|| value_after(args, "--recode-video")).unwrap_or("mp4")
    };
    let index = item.unwrap_or(1);
    let target = template.replace("%(playlist_index)03d", &format!("{index:03}")).replace("%(ext)s", extension).replace("%%", "%");
    let target = PathBuf::from(target);
    eprintln!(
        r#"veo-progress:{{"progress":{{"status":"downloading","downloaded_bytes":1024,"total_bytes":2048,"speed":1000,"eta":1}},"video":"avc1","audio":"mp4a"}}"#
    );
    let mut content = vec![b'v'; 2048];
    content[..url.len().min(2048)].copy_from_slice(&url.as_bytes()[..url.len().min(2048)]);
    fs::write(&target, &content).expect("write media");
    if args.iter().any(|arg| arg == "--write-subs") {
        let stem = target.with_extension("");
        fs::write(format!("{}.en.vtt", stem.display()), "WEBVTT\n").expect("write subtitles");
    }
    eprintln!(r#"veo-progress:{{"progress":{{"status":"finished","downloaded_bytes":2048,"total_bytes":2048}},"video":"avc1","audio":"mp4a"}}"#);
    eprintln!(r#"veo-postprocess:{{"status":"started","postprocessor":"Merger"}}"#);
    eprintln!(r#"veo-postprocess:{{"status":"finished","postprocessor":"Merger"}}"#);
    println!("veo-file:{}", quote(&target.display().to_string()));
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let program = env::args().next().unwrap_or_default();
    let name = Path::new(&program).file_stem().map(|stem| stem.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.starts_with("ffprobe") {
        if let Ok(log) = env::var("VEO_FAKE_PROBE_LOG") {
            use std::io::Write;
            let mut file = fs::OpenOptions::new().create(true).append(true).open(log).unwrap();
            writeln!(file, "{}", serde_json::to_string(&args).unwrap()).unwrap();
        }
        if args.iter().any(|arg| arg == "-version") {
            println!("ffprobe version fake");
            return;
        }
        println!(
            r#"{{"streams":[{{"index":0,"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p","width":640,"height":360}},{{"index":1,"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2,"tags":{{"language":"eng"}}}}],"format":{{"format_name":"mov,mp4,m4a","duration":"2.000000","size":"2048"}}}}"#
        );
        return;
    }
    if name.starts_with("ffmpeg") {
        if args.iter().any(|arg| arg == "-version") {
            println!("ffmpeg version fake");
            return;
        }
        eprintln!("[Parsed_volumedetect_0] max_volume: -12.5 dB");
        return;
    }
    yt_dlp(&args);
}
