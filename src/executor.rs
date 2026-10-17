//! Linux process lifecycle. The protocol layer never starts an OS process.
use crate::{
    command::CommandSpec,
    config::Config,
    memory::{Completion, Status},
};
use std::{
    io,
    os::{
        fd::OwnedFd,
        unix::{
            net::UnixStream as StdUnixStream,
            process::{CommandExt, ExitStatusExt},
        },
    },
    process::Stdio,
};
use tokio::{
    io::AsyncReadExt,
    net::UnixStream,
    process::Command,
    sync::watch,
    time::{self, Duration, Instant},
};

struct ProcessGroup(i32);

impl ProcessGroup {
    fn kill(&mut self) {
        // SAFETY: spawn() created this process group via process_group(0).
        // The strictly positive PID avoids kill(0) and kill(-1).
        if self.0 > 1 {
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
        self.0 = 0;
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

struct Capture {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}
impl Capture {
    fn append(&mut self, chunk: &[u8]) {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        self.truncated |= chunk.len() > remaining;
    }
    fn text(&self) -> String {
        // An intentionally cut UTF-8 tail is omitted, not turned into a broken character.
        let mut bytes = self.bytes.as_slice();
        if self.truncated {
            if let Err(error) = std::str::from_utf8(bytes) {
                if error.error_len().is_none() {
                    bytes = &bytes[..error.valid_up_to()];
                }
            }
        }
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn output_pair() -> io::Result<(UnixStream, Stdio, Stdio)> {
    let (reader, writer) = StdUnixStream::pair()?;
    reader.set_nonblocking(true)?;
    let stdout = Stdio::from(OwnedFd::from(writer.try_clone()?));
    let stderr = Stdio::from(OwnedFd::from(writer));
    Ok((UnixStream::from_std(reader)?, stdout, stderr))
}

pub async fn execute(
    spec: &CommandSpec,
    config: &Config,
    mut shutdown: watch::Receiver<bool>,
) -> Completion {
    if *shutdown.borrow() {
        return Completion::failure(Status::Cancelled, "sm0sh: service stopping\n".into());
    }
    let (mut reader, stdout, stderr) = match output_pair() {
        Ok(pair) => pair,
        Err(error) => {
            return Completion::failure(Status::IoFailure, format!("sm0sh: output pipe: {error}\n"))
        }
    };
    let mut builder = std::process::Command::new(&spec.program);
    builder
        .args(&spec.args)
        .current_dir(&config.working_directory)
        .env_clear()
        .envs(&config.environment)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0);
    let mut command = Command::from(builder);
    command.kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Completion::failure(
                Status::SpawnFailure,
                format!("sm0sh: {}: {error}\n", spec.program),
            )
        }
    };
    // Drop the builder's output descriptors so EOF is visible after the child exits.
    drop(command);
    let mut group = ProcessGroup(child.id().expect("a newly spawned child has a PID") as i32);
    let deadline = Instant::now() + config.command_timeout();
    let mut capture = Capture {
        bytes: Vec::new(),
        limit: config.sd_words * 2 - 1,
        truncated: false,
    };
    let mut chunk = [0u8; 4096];
    let mut eof = false;
    let mut failure = None;
    let status = loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => { failure = Some((Status::Cancelled, "service stopping".to_string())); break None; }
            _ = time::sleep_until(deadline) => { failure = Some((Status::Timeout, "command timed out".to_string())); break None; }
            status = child.wait() => {
                match status {
                    Ok(value) => break Some(value),
                    Err(error) => { failure = Some((Status::IoFailure, format!("wait: {error}"))); break None; }
                }
            }
            read = reader.read(&mut chunk), if !eof => {
                match read {
                    Ok(0) => eof = true,
                    Ok(size) => capture.append(&chunk[..size]),
                    Err(error) => { failure = Some((Status::IoFailure, format!("read output: {error}"))); break None; }
                }
            }
        }
    };
    // Cleanup ordinary descendants even if their parent exits before they do.
    group.kill();
    if status.is_none() {
        let _ = time::timeout(Duration::from_secs(2), child.wait()).await;
    }
    let drain_deadline = Instant::now() + Duration::from_millis(500);
    while !eof {
        match time::timeout_at(drain_deadline, reader.read(&mut chunk)).await {
            Ok(Ok(0)) => eof = true,
            Ok(Ok(size)) => capture.append(&chunk[..size]),
            Ok(Err(error)) => {
                failure.get_or_insert((Status::IoFailure, format!("read output: {error}")));
                break;
            }
            Err(_) => {
                capture.truncated = true;
                break;
            }
        }
    }
    let mut text = capture.text();
    let final_status = if let Some((status, detail)) = failure {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("sm0sh: {detail}\n"));
        status
    } else if status.is_some_and(|value| value.success()) {
        Status::Success
    } else {
        Status::ExitFailure
    };
    Completion {
        status: final_status,
        exit_code: status.and_then(|v| v.code()),
        signal: status.and_then(|v| v.signal()),
        text,
        truncated: capture.truncated,
    }
}
