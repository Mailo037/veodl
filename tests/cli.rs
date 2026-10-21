//! End-to-end tests of the real `veo` binary against an offline fake backend
//! (tests/fake-backend), with isolated cache and config directories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use serde_json::Value;

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Build the fake backend once and return its path.
fn fake_backend() -> &'static PathBuf {
    static FAKE: OnceLock<PathBuf> = OnceLock::new();
    FAKE.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let target = root.join("target").join("fake-backend");
        let status = Command::new(env!("CARGO"))
            .args(["build", "--quiet", "--manifest-path"])
            .arg(root.join("tests").join("fake-backend").join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .status()
            .expect("cargo builds the fake backend");
        assert!(status.success(), "fake backend build failed");
        target.join("debug").join(exe("fake-backend"))
    })
}

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root = std::env::temp_dir().join(format!("veo-it-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let tools = root.join("tools");
        fs::create_dir_all(&tools).unwrap();
        for tool in ["yt-dlp", "ffmpeg", "ffprobe"] {
            let target = tools.join(exe(tool));
            fs::copy(fake_backend(), &target).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        Sandbox { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    fn arg(&self, relative: &str) -> String {
        self.path(relative).display().to_string()
    }

    fn run(&self, args: &[&str]) -> Output {
        let tools = self.path("tools");
        Command::new(env!("CARGO_BIN_EXE_veo"))
            .args(args)
            .current_dir(&self.root)
            .env("VEO_YT_DLP_PATH", tools.join(exe("yt-dlp")))
            .env("VEO_FFMPEG_PATH", &tools)
            .env("VEO_NO_UPDATE_CHECK", "1")
            .env("VEO_CONFIG", self.path("config.json"))
            .env("LOCALAPPDATA", self.path("cache"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env("HOME", &self.root)
            .env_remove("NO_COLOR")
            .output()
            .expect("veo runs")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn json_lines(output: &Output) -> Vec<Value> {
    stdout(output).lines().filter(|line| !line.trim().is_empty()).map(|line| serde_json::from_str(line).expect("JSON line")).collect()
}

#[test]
fn help_version_and_usage_errors() {
    let sandbox = Sandbox::new("help");
    let help = sandbox.run(&["--help"]);
    assert!(help.status.success());
    assert!(stdout(&help).starts_with("veo - simple video downloader\n"));
    let version = sandbox.run(&["--version"]);
    assert_eq!(stdout(&version).trim(), env!("CARGO_PKG_VERSION"));
    let invalid = sandbox.run(&["not-a-url"]);
    assert_eq!(invalid.status.code(), Some(1));
    assert!(stderr(&invalid).contains("Invalid URL"));
    let typo = sandbox.run(&["histroy"]);
    assert_eq!(stderr(&typo).trim(), "veo: Unknown command \"histroy\". Did you mean \"history\"?");
}

#[test]
fn downloads_skips_and_records_history() {
    let sandbox = Sandbox::new("download");
    let out = sandbox.arg("out");
    let first = sandbox.run(&["https://example.com/v", "--json", "-o", &out]);
    assert!(first.status.success(), "{}", stderr(&first));
    let result = &json_lines(&first)[0];
    assert_eq!(result["status"], "saved");
    assert!(result["runId"].as_str().is_some_and(|id| id.len() == 6));
    let file = PathBuf::from(result["files"][0].as_str().unwrap());
    assert_eq!(file.file_name().unwrap(), "Fixture Video.mp4");
    assert_eq!(fs::metadata(&file).unwrap().len(), 2048);

    let second = sandbox.run(&["https://example.com/v", "--json", "-o", &out]);
    assert_eq!(
        json_lines(&second)[0]["files"][0].as_str().map(|path| Path::new(path).file_name().unwrap().to_string_lossy().into_owned()).as_deref(),
        Some("Fixture Video (1).mp4")
    );

    let skipped = sandbox.run(&["https://example.com/v", "--json", "--skip-existing", "-o", &out]);
    assert_eq!(json_lines(&skipped)[0]["status"], "skipped");

    let history = sandbox.run(&["history", "--json", "--limit", "10"]);
    let history: Value = serde_json::from_str(&stdout(&history)).unwrap();
    assert_eq!(history["count"], 3);
    assert_eq!(history["entries"][0]["status"], "skipped");

    let stats = sandbox.run(&["stats", "--json"]);
    let stats: Value = serde_json::from_str(&stdout(&stats)).unwrap();
    assert_eq!(stats["videos"], 2);
    assert_eq!(stats["skipped"], 1);

    // Finished runs remove their record, but the archive stays inspectable.
    assert!(stdout(&sandbox.run(&["runs"])).contains("No veo runs are active"));
    let id = result["runId"].as_str().unwrap();
    let inspected = sandbox.run(&["inspect", "run", id, "--json"]);
    assert!(inspected.status.success(), "{}", stderr(&inspected));
    let inspected: Value = serde_json::from_str(&stdout(&inspected)).unwrap();
    assert_eq!(inspected["missingFiles"], 0);
    assert_eq!(inspected["items"][0]["files"][0]["metadata"]["hasVideoTrack"], true);
}

#[test]
fn failed_items_can_be_retried_by_run_id() {
    let sandbox = Sandbox::new("retry");
    let out = sandbox.arg("batch");
    let batch = sandbox.run(&["https://example.com/v", "https://example.com/missing", "--json", "-o", &out, "--concurrent-downloads", "1"]);
    assert_eq!(batch.status.code(), Some(1));
    let lines = json_lines(&batch);
    let statuses: Vec<&str> = lines.iter().map(|line| line["status"].as_str().unwrap()).collect();
    assert_eq!(statuses, ["saved", "failed"]);
    assert_eq!(lines[1]["error"], "The content was deleted, removed, or could not be found.");
    assert!(stderr(&batch).contains("Summary: 1 saved, 0 skipped, 1 failed."));
    let id = lines[1]["runId"].as_str().unwrap().to_string();
    let retried = sandbox.run(&["--retry-failed", &id, "--json"]);
    assert_eq!(retried.status.code(), Some(1));
    let retried = json_lines(&retried);
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0]["url"], "https://example.com/missing");
    let last = sandbox.run(&["retry", "--last", "--json"]);
    assert_eq!(json_lines(&last)[0]["url"], "https://example.com/missing");
}

#[test]
fn playlists_audio_subtitles_and_naming() {
    let sandbox = Sandbox::new("playlist");
    let list = sandbox.run(&["https://example.com/playlist", "--playlist", "--json", "-o", &sandbox.arg("list")]);
    assert!(list.status.success(), "{}", stderr(&list));
    let result = &json_lines(&list)[0];
    assert_eq!(result["saved"], 3);
    let names: Vec<String> =
        result["files"].as_array().unwrap().iter().map(|file| Path::new(file.as_str().unwrap()).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["First.mp4", "Second.mp4", "Third.mp4"]);

    let selected = sandbox.run(&["https://example.com/playlist", "--playlist-items", "2", "-r", "Sel", "--json", "-o", &sandbox.arg("sel")]);
    let selected = &json_lines(&selected)[0];
    assert!(selected["files"][0].as_str().unwrap().ends_with("Sel - 002.mp4"));

    let audio = sandbox.run(&["https://example.com/v", "--audio", "--format", "m4a", "--json", "-o", &sandbox.arg("audio")]);
    assert!(json_lines(&audio)[0]["files"][0].as_str().unwrap().ends_with("Fixture Video.m4a"));

    let subs = sandbox.run(&["https://example.com/v", "--subs", "--json", "-o", &sandbox.arg("subs")]);
    let files = json_lines(&subs)[0]["files"].clone();
    assert!(files[1].as_str().unwrap().ends_with("Fixture Video.en.vtt"), "{files}");

    let templated = sandbox.run(&[
        "https://example.com/v",
        "--filename-template",
        "{index} - {title}",
        "--folder-template",
        "{channel}/{year}",
        "--json",
        "-o",
        &sandbox.arg("tpl"),
    ]);
    let saved = PathBuf::from(json_lines(&templated)[0]["files"][0].as_str().unwrap().to_string());
    assert!(saved.ends_with(Path::new("Fixture Channel").join("2024").join("001 - Fixture Video.mp4")), "{}", saved.display());
}

#[test]
fn listings_and_dry_runs_write_nothing() {
    let sandbox = Sandbox::new("listing");
    let qualities = sandbox.run(&["https://example.com/v", "--list-qualities", "--json"]);
    assert_eq!(json_lines(&qualities)[0]["qualities"], serde_json::json!(["720p", "360p"]));
    let subs = sandbox.run(&["subs", "https://example.com/v", "--json"]);
    assert_eq!(json_lines(&subs)[0]["manual"][1]["formats"], serde_json::json!(["srt", "vtt"]));
    let dry = sandbox.run(&["https://example.com/v", "--dry-run", "--json", "-o", &sandbox.arg("dry")]);
    let plan = &json_lines(&dry)[0];
    assert_eq!(plan["status"], "planned");
    assert_eq!(plan["estimatedBytes"], 0);
    assert!(!sandbox.path("dry").exists());
    let capped = sandbox.run(&["https://example.com/v", "-q", "240p", "--json"]);
    assert_eq!(capped.status.code(), Some(1));
    assert!(json_lines(&capped)[0]["error"].as_str().unwrap().contains("offered: 360p, 720p"));
    let experimental = sandbox.run(&["https://example.com/v", "-N", "32", "--json"]);
    assert!(stderr(&experimental).contains("17-64 concurrent fragments are experimental"));
}

#[test]
fn incognito_leaves_no_records() {
    let sandbox = Sandbox::new("incognito");
    let run = sandbox.run(&["https://example.com/v", "--incognito", "--json", "-o", &sandbox.arg("inc")]);
    assert!(run.status.success(), "{}", stderr(&run));
    let result = &json_lines(&run)[0];
    assert!(result.get("runId").is_none());
    assert!(!sandbox.path("inc").join(".veo-history").exists());
    let history: Value = serde_json::from_str(&stdout(&sandbox.run(&["history", "--json"]))).unwrap();
    assert_eq!(history["count"], 0);
}

#[test]
fn config_profiles_and_changes() {
    let sandbox = Sandbox::new("config");
    fs::write(sandbox.path("config.json"), "{\n  // comment\n  \"quality\": \"720p\",\n  \"profiles\": { \"music\": { \"audio\": true } }\n}\n").unwrap();
    let show = sandbox.run(&["config", "show", "--profile", "music"]);
    let effective: Value = serde_json::from_str(&stdout(&show)).unwrap();
    assert_eq!(effective["audio"], true);
    assert_eq!(effective["quality"], "720p");
    assert!(stdout(&sandbox.run(&["config", "check"])).starts_with("Config OK:"));
    assert_eq!(stdout(&sandbox.run(&["profile", "music"])).trim(), "Active profile: music");
    assert!(fs::read_to_string(sandbox.path("config.json")).unwrap().contains("\"activeProfile\": \"music\""));
    fs::write(sandbox.path("config.json"), "{ \"quality\": 720 }").unwrap();
    let broken = sandbox.run(&["https://example.com/v", "--dry-run"]);
    assert!(stderr(&broken).contains("Config key \"quality\" must be a string, not a number"));
    let changes: Value = serde_json::from_str(&stdout(&sandbox.run(&["changes", "--json"]))).unwrap();
    assert_eq!(changes["current"], env!("CARGO_PKG_VERSION"));
    assert_eq!(changes["versions"][0]["installed"], true);
}

#[test]
fn transient_failures_are_retried() {
    let sandbox = Sandbox::new("busy");
    let _ = fs::remove_file(std::env::temp_dir().join("veo-fake-busy"));
    let run = sandbox.run(&["https://example.com/busy", "--json", "-o", &sandbox.arg("busy")]);
    assert!(run.status.success(), "{}", stderr(&run));
    assert!(stderr(&run).contains("retrying in 2s (1/2)"));
}
