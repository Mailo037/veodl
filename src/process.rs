use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as StdCommand, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::cancel::Cancel;
use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// A child process started without a shell. Output arrives through callbacks on
/// the calling thread; cancellation and timeouts stop the whole process tree.
pub struct Command {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Option<Vec<(OsString, OsString)>>,
    pub cwd: Option<PathBuf>,
    pub timeout: Option<Duration>,
    pub timeout_message: String,
    pub stdin: Option<Vec<u8>>,
    pub capture_stdout: bool,
    pub capture_stderr: bool,
    /// Hide a console window and detach from the terminal's process group.
    pub background: bool,
}

impl Command {
    pub fn new(program: impl AsRef<Path>) -> Self {
        Command {
            program: program.as_ref().to_path_buf(),
            args: Vec::new(),
            env: None,
            cwd: None,
            timeout: None,
            timeout_message: "timed out".into(),
            stdin: None,
            capture_stdout: true,
            capture_stderr: true,
            background: true,
        }
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn timeout(mut self, timeout: Duration, message: impl Into<String>) -> Self {
        self.timeout = Some(timeout);
        self.timeout_message = message.into();
        self
    }

    fn build(&self) -> StdCommand {
        let mut command = StdCommand::new(&self.program);
        command.args(&self.args);
        if let Some(env) = &self.env {
            command.env_clear();
            command.envs(env.iter().map(|(key, value)| (key, value)));
        }
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command.stdin(if self.stdin.is_some() { Stdio::piped() } else { Stdio::null() });
        command.stdout(if self.capture_stdout { Stdio::piped() } else { Stdio::null() });
        command.stderr(if self.capture_stderr { Stdio::piped() } else { Stdio::null() });
        if self.background {
            hide(&mut command);
        }
        command
    }
}

#[cfg(windows)]
fn hide(command: &mut StdCommand) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn hide(command: &mut StdCommand) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(any(windows, unix)))]
fn hide(_: &mut StdCommand) {}

/// Stop a child and everything it started (yt-dlp runs ffmpeg as a child).
pub fn kill_tree(child: &mut Child, grouped: bool) {
    #[cfg(windows)]
    {
        let _ = grouped;
        let mut killer = StdCommand::new("taskkill.exe");
        killer.args(["/PID", &child.id().to_string(), "/T", "/F"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        hide(&mut killer);
        match killer.status() {
            Ok(status) if status.success() => {}
            _ => {
                let _ = child.kill();
            }
        }
    }
    #[cfg(unix)]
    {
        let pid = child.id() as libc::pid_t;
        if grouped {
            // SAFETY: signalling our own child's process group.
            unsafe { libc::kill(-pid, libc::SIGTERM) };
            let end = Instant::now() + Duration::from_secs(3);
            while Instant::now() < end {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(Duration::from_millis(25));
            }
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        } else {
            let _ = child.kill();
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = grouped;
        let _ = child.kill();
    }
}

enum Message {
    Data(Stream, Vec<u8>),
    Eof,
}

fn pump(mut reader: impl Read + Send + 'static, stream: Stream, sender: mpsc::Sender<Message>) {
    thread::spawn(move || {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if sender.send(Message::Data(stream, buffer[..read].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = sender.send(Message::Eof);
    });
}

/// Run to completion, feeding raw output chunks to `on_data`. Returns the exit
/// code (None when ended by a signal). A callback error stops the process.
pub fn run(command: &Command, cancel: &Cancel, mut on_data: impl FnMut(Stream, &[u8]) -> Result<()>) -> Result<Option<i32>> {
    cancel.check()?;
    let mut child = command.build().spawn().map_err(|error| {
        let code = crate::error::io_code(&error);
        Error { code, ..Error::msg(format!("spawn {} {}", command.program.display(), error)) }
    })?;
    if let (Some(input), Some(mut stdin)) = (command.stdin.clone(), child.stdin.take()) {
        thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let (sender, receiver) = mpsc::channel();
    let mut open = 0;
    if let Some(stdout) = child.stdout.take() {
        pump(stdout, Stream::Stdout, sender.clone());
        open += 1;
    }
    if let Some(stderr) = child.stderr.take() {
        pump(stderr, Stream::Stderr, sender.clone());
        open += 1;
    }
    drop(sender);
    let started = Instant::now();
    let mut failure: Option<Error> = None;
    let mut status = None;
    let mut pause = Duration::from_micros(250);
    loop {
        if failure.is_none() {
            if cancel.is_cancelled() {
                kill_tree(&mut child, command.background);
                failure = Some(cancel.check().err().unwrap_or_else(Error::cancelled));
            } else if command.timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
                kill_tree(&mut child, command.background);
                failure = Some(Error::timeout(command.timeout_message.clone()));
            }
        }
        if open == 0 {
            if status.is_none() {
                match child.try_wait()? {
                    Some(exit) => status = Some(exit),
                    None => {
                        // Output is closed; the exit follows almost immediately.
                        thread::sleep(pause);
                        pause = (pause * 2).min(Duration::from_millis(20));
                        continue;
                    }
                }
            }
            break;
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(Message::Data(stream, data)) => {
                if failure.is_none() {
                    if let Err(error) = on_data(stream, &data) {
                        kill_tree(&mut child, command.background);
                        failure = Some(error);
                    }
                }
            }
            Ok(Message::Eof) => open -= 1,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => open = 0,
        }
    }
    let status = match status {
        Some(status) => status,
        None => child.wait()?,
    };
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(status.code())
}

/// Splits a byte stream into lines, tolerating CRLF and a missing final newline.
#[derive(Default)]
pub struct Lines {
    pending: Vec<u8>,
}

impl Lines {
    pub fn push(&mut self, data: &[u8], mut each: impl FnMut(&str) -> Result<()>) -> Result<()> {
        self.pending.extend_from_slice(data);
        let mut start = 0;
        while let Some(offset) = self.pending[start..].iter().position(|byte| *byte == b'\n') {
            let end = start + offset;
            let mut line = &self.pending[start..end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            each(&String::from_utf8_lossy(line))?;
            start = end + 1;
        }
        self.pending.drain(..start);
        Ok(())
    }

    pub fn finish(&mut self, mut each: impl FnMut(&str) -> Result<()>) -> Result<()> {
        if !self.pending.is_empty() {
            let mut line = std::mem::take(&mut self.pending);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            each(&String::from_utf8_lossy(&line))?;
        }
        Ok(())
    }
}

/// Collected output of a short-lived tool.
pub struct Captured {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Run and keep stdout/stderr up to `limit` bytes each; larger output fails.
pub fn capture(command: &Command, cancel: &Cancel, limit: usize, overflow: &str) -> Result<Captured> {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let code = run(command, cancel, |stream, data| {
        let target = if stream == Stream::Stdout { &mut stdout } else { &mut stderr };
        if target.len() + data.len() > limit {
            return Err(Error::msg(overflow));
        }
        target.extend_from_slice(data);
        Ok(())
    })?;
    Ok(Captured { code, stdout: String::from_utf8_lossy(&stdout).into_owned(), stderr: String::from_utf8_lossy(&stderr).into_owned() })
}
