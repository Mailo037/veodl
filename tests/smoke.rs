//! Real end-to-end downloads through the actual CLI, yt-dlp and FFmpeg. The
//! media is generated locally and served from 127.0.0.1; no third-party
//! content is fetched. The tools themselves are downloaded once (network), so
//! this test is ignored by default: `cargo test --test smoke -- --ignored`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::{fs, thread};

use serde_json::Value;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn run(&self, args: &[&str]) -> Output {
        self.run_in(&self.root, args)
    }

    fn run_in(&self, directory: &Path, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_veo"));
        command
            .args(args)
            .current_dir(directory)
            .env("VEO_CONFIG", self.root.join("config.json"))
            .env("VEO_NO_UPDATE_CHECK", "1")
            .env("LOCALAPPDATA", &self.root)
            .env("XDG_CACHE_HOME", &self.root)
            .env("HOME", &self.root)
            .env_remove("VEO_YT_DLP_PATH")
            .env_remove("VEO_FFMPEG_PATH");
        // Optional pre-existing real tools keep local regression runs offline.
        if let Some(directory) = std::env::var_os("VEO_SMOKE_TOOLS") {
            let directory = PathBuf::from(directory);
            command.env("VEO_YT_DLP_PATH", directory.join(format!("yt-dlp{}", std::env::consts::EXE_SUFFIX))).env("VEO_FFMPEG_PATH", directory);
        }
        command.output().expect("veo runs")
    }
}

/// The first file with this name below `directory` (the managed tool cache).
fn find(directory: &Path, name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(directory).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find(&path, name) {
                return Some(found);
            }
        } else if entry.file_name() == name {
            return Some(path);
        }
    }
    None
}

fn json(output: &Output) -> Value {
    let text = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(text.trim().lines().last().expect("a JSON line")).expect("valid JSON")
}

fn assert_ok(output: &Output) {
    assert!(output.status.success(), "exit {:?}\n{}", output.status.code(), String::from_utf8_lossy(&output.stderr));
}

struct Server {
    port: u16,
    requests: Arc<AtomicUsize>,
    recovered: Arc<AtomicBool>,
}

fn serve(media: Vec<u8>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let recovered = Arc::new(AtomicBool::new(false));
    let media = Arc::new(media);
    let (count, ready) = (requests.clone(), recovered.clone());
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (media, count, ready) = (media.clone(), count.clone(), ready.clone());
            thread::spawn(move || respond(stream, &media, &count, &ready));
        }
    });
    Server { port, requests, recovered }
}

fn respond(mut stream: TcpStream, media: &[u8], requests: &AtomicUsize, recovered: &AtomicBool) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
            break;
        }
    }
    requests.fetch_add(1, Ordering::SeqCst);
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let (status, kind, body): (&str, &str, Vec<u8>) = match path {
        "/playlist.html" => ("200 OK", "text/html", b"<html><head><title>Fixture playlist</title></head><body><video src=\"/original-title.mp4\"></video><video src=\"/recover.mp4\"></video></body></html>".to_vec()),
        "/original-title.mp4" => ("200 OK", "video/mp4", media.to_vec()),
        "/recover.mp4" if recovered.load(Ordering::SeqCst) => ("200 OK", "video/mp4", media.to_vec()),
        _ => ("404 Not Found", "text/plain", Vec::new()),
    };
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    if method != "HEAD" {
        let _ = stream.write_all(&body);
    }
}

#[test]
#[ignore = "downloads yt-dlp and FFmpeg; run with --ignored"]
fn real_downloads_through_yt_dlp() {
    let root = std::env::temp_dir().join(format!("veo-smoke-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let fixture = Fixture { root: root.clone() };

    // First use: download, verify and run the pinned tools.
    assert_ok(&fixture.run(&["doctor", "fix"]));
    // doctor fix installs the pinned pair into the cache unless a system pair exists.
    let name = format!("ffmpeg{}", std::env::consts::EXE_SUFFIX);
    let ffmpeg = std::env::var_os("VEO_SMOKE_TOOLS")
        .map(|directory| PathBuf::from(directory).join(&name))
        .or_else(|| find(&root, &name))
        .or_else(|| std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).map(|directory| directory.join(&name)).find(|file| file.is_file()))
        .expect("an ffmpeg after doctor fix");
    let source = root.join("fixture.mp4");
    let generated = Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=640x360:r=24",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440",
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-movflags",
            "+faststart",
        ])
        .arg(&source)
        .status()
        .unwrap();
    assert!(generated.success());
    let media = fs::read(&source).unwrap();
    let server = serve(media.clone());
    let url = format!("http://127.0.0.1:{}/original-title.mp4", server.port);

    let native = fixture.run(&[&url, "--json", "--verify", "--staging-dir", "output", "-o", "./native"]);
    assert_ok(&native);
    assert!(String::from_utf8_lossy(&native.stderr).contains("Rust HTTP"), "{}", String::from_utf8_lossy(&native.stderr));
    assert_eq!(fs::read(json(&native)["files"][0].as_str().unwrap()).unwrap(), media);
    let backend = fixture.run(&[&url, "--no-native-http", "--verify", "--json", "-o", "./backend"]);
    assert_ok(&backend);
    assert!(!String::from_utf8_lossy(&backend.stderr).contains("Rust HTTP"));
    assert_eq!(fs::read(json(&backend)["files"][0].as_str().unwrap()).unwrap(), media);

    assert_ok(&fixture.run(&[&url]));
    assert_ok(&fixture.run(&[&url, "-q", "720p"]));
    assert_ok(&fixture.run(&[&url, "--audio", "-o", "./audio"]));
    assert_ok(&fixture.run(&[&url, "-r", "My Video"]));
    assert_ok(&fixture.run(&[&url, "-r", "My Video"]));
    assert_ok(&fixture.run(&[&url, "--format", "webm", "--recode", "-o", "./converted"]));
    assert_ok(&fixture.run(&[&url, "--format", "mkv", "-o", "./remuxed"]));
    let incompatible = fixture.run(&[&url, "--format", "webm", "-o", "./incompatible"]);
    assert_eq!(incompatible.status.code(), Some(1), "incompatible container must fail instead of silently re-encoding");
    assert_eq!(fixture.run(&[&url.replace("original-title", "deleted")]).status.code(), Some(1));

    let dry = fixture.run(&[&url, "--dry-run", "-o", "./dry"]);
    assert_ok(&dry);
    assert!(String::from_utf8_lossy(&dry.stdout).contains("Would save: "));
    assert!(!root.join("dry").exists(), "dry run must not create the output directory");
    let formats = fixture.run(&[&url, "--list-formats"]);
    assert_ok(&formats);
    assert!(String::from_utf8_lossy(&formats.stdout).contains("mp4"));

    let saved = fixture.run(&[&url, "--json", "-o", "./json"]);
    assert_ok(&saved);
    let saved = json(&saved);
    assert_eq!(saved["status"], "saved");
    assert_eq!(fs::metadata(saved["files"][0].as_str().unwrap()).unwrap().len(), media.len() as u64);
    let inspected = fixture.run(&["inspect", saved["files"][0].as_str().unwrap(), "--check-audio", "--json"]);
    assert_ok(&inspected);
    assert_eq!(json(&inspected)["audioCheck"]["hasSignal"], true);

    // A batch with one failing URL reports failure and names it.
    let batch = fixture.run(&[&url, &url.replace("original-title", "deleted"), "-o", "./batch"]);
    assert_eq!(batch.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&batch.stderr).contains("deleted.mp4"));
    assert_eq!(fs::metadata(root.join("batch").join("original-title.mp4")).unwrap().len(), media.len() as u64);

    // Profiles, URL lists and a persisted retry from another directory.
    fs::write(
        root.join("config.json"),
        format!("{{\"profiles\":{{\"small\":{{\"quality\":\"720p\",\"output\":{}}}}}}}", serde_json::to_string(&root.join("profile-output")).unwrap()),
    )
    .unwrap();
    fs::write(root.join("urls.txt"), format!("# Own generated videos\n{url}\n\n{}\n", url.replace("original-title", "recover"))).unwrap();
    let list = fixture.run(&["--batch-file", root.join("urls.txt").to_str().unwrap(), "--profile", "small", "--json"]);
    assert_eq!(list.status.code(), Some(1));
    let lines: Vec<Value> = String::from_utf8_lossy(&list.stdout).lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let failed = lines.iter().find(|line| line["status"] == "failed").unwrap();
    let id = failed["runId"].as_str().unwrap().to_string();
    server.recovered.store(true, Ordering::SeqCst);
    let elsewhere = root.join("other-cwd");
    fs::create_dir_all(&elsewhere).unwrap();
    let retried = fixture.run_in(&elsewhere, &["--retry-failed", &id, "--json"]);
    assert_ok(&retried);
    assert_eq!(Path::new(json(&retried)["files"][0].as_str().unwrap()).parent().unwrap(), root.join("profile-output"));

    // Generic HTML playlist with entry selection.
    let playlist = fixture.run(&[
        &url.replace("original-title.mp4", "playlist.html"),
        "--playlist-items",
        "2",
        "-r",
        "Selected",
        "-o",
        "./playlist",
        "--resume",
        "--json",
    ]);
    assert_ok(&playlist);
    assert!(json(&playlist)["files"][0].as_str().unwrap().ends_with("Selected - 002.mp4"));

    // A finished download survives an unavailable destination and is
    // transferred later without contacting the source again.
    let blocked = root.join("blocked-output");
    fs::write(&blocked, "destination unavailable").unwrap();
    let failed_transfer = fixture.run(&[&url, "-o", blocked.to_str().unwrap(), "--json"]);
    assert_eq!(failed_transfer.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&failed_transfer.stderr).contains("Completed download kept locally"));
    fs::remove_file(&blocked).unwrap();
    let before = server.requests.load(Ordering::SeqCst);
    let transfer = fixture.run(&[&url, "-o", blocked.to_str().unwrap(), "--json"]);
    assert_ok(&transfer);
    assert_eq!(server.requests.load(Ordering::SeqCst), before, "transfer retry must not contact the source again");

    let files: Vec<String> = fs::read_dir(&root).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    for name in ["original-title.mp4", "original-title (1).mp4", "My Video.mp4", "My Video (1).mp4"] {
        assert!(files.iter().any(|file| file == name), "{name} missing in {files:?}");
    }
    assert!(fs::metadata(root.join("converted").join("original-title.webm")).unwrap().len() > 0);
    assert!(fs::metadata(root.join("audio").join("original-title.mp3")).unwrap().len() > 0);
    assert!(!files.iter().any(|name| name.starts_with(".veo-") && name != ".veo-history"));
    let history: Value = serde_json::from_str(&String::from_utf8_lossy(&fixture.run(&["history", "--json", "--limit", "100"]).stdout)).unwrap();
    assert!(history["count"].as_u64().unwrap() > 0);
    assert!(String::from_utf8_lossy(&fixture.run(&["runs"]).stdout).contains("No veo runs are active"));
    let _ = fs::remove_dir_all(&root);
}
