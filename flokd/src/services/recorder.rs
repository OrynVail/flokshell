use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, Command as Process};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use zbus::Connection;

use crate::bus::{Bus, Request};
use crate::notify::{self, Notice};
use crate::services::capture;

const SERVICE: &str = "recorder";
const BIN: &str = "gpu-screen-recorder";
// NixOS can only grant KMS capture through this wrapper; other systems set the capability.
const NIXOS: &str = "/run/current-system/sw/bin/nixos-version";
const KMS_WRAPPER: &str = "/run/wrappers/bin/gsr-kms-server";
// Finishing the file takes a moment; a recorder that never does gets killed.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);
const CANCELLED: &str = "canceled by the user";
const STDERR_LINES: usize = 8;

#[derive(Serialize, Default, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    recording: bool,
    // Unix milliseconds; the shell ticks its own clock from this.
    started: u64,
    direct: bool,
    dir: String,
    error: String,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(tag = "cmd", content = "args", rename_all = "lowercase")]
enum Command {
    Start {
        mode: String,
        #[serde(default)]
        region: String,
        #[serde(default)]
        output: bool,
        #[serde(default)]
        input: bool,
    },
    Stop,
    Open,
}

impl TryFrom<Request> for Command {
    type Error = serde_json::Error;

    fn try_from(req: Request) -> Result<Self, Self::Error> {
        let mut v = serde_json::json!({ "cmd": req.cmd });
        if !req.args.is_null() {
            v["args"] = req.args;
        }
        serde_json::from_value(v)
    }
}

struct Active {
    child: Child,
    path: PathBuf,
    stderr: JoinHandle<String>,
    deadline: Option<Instant>,
}

pub struct Service {
    bus: Bus,
    state: State,
    active: Option<Active>,
    session: Option<Connection>,
}

impl Service {
    pub fn new(bus: Bus) -> Self {
        Self {
            bus,
            state: State {
                direct: direct(Path::new(NIXOS), Path::new(KMS_WRAPPER)),
                dir: capture::videos()
                    .join("Recordings")
                    .to_string_lossy()
                    .into_owned(),
                ..Default::default()
            },
            active: None,
            session: None,
        }
    }

    pub async fn run(mut self, mut rx: mpsc::Receiver<Request>) {
        self.session = Connection::session().await.ok();
        self.publish();

        loop {
            let deadline = self.active.as_ref().and_then(|a| a.deadline);
            tokio::select! {
                req = rx.recv() => {
                    let Some(req) = req else { return };
                    match Command::try_from(req) {
                        Ok(Command::Start { mode, region, output, input }) => {
                            if let Err(e) = self.start(&mode, &region, output, input).await {
                                self.failed(&format!("{e:#}")).await;
                            }
                        }
                        Ok(Command::Stop) => self.stop(),
                        Ok(Command::Open) => {
                            let _ = std::fs::create_dir_all(&self.state.dir);
                            if let Err(e) = Process::new("xdg-open").arg(&self.state.dir).spawn() {
                                tracing::warn!("xdg-open: {e}");
                            }
                        }
                        Err(e) => tracing::warn!("bad command: {e}"),
                    }
                }
                status = wait(&mut self.active) => self.finished(status).await,
                () = overdue(deadline) => self.kill(),
            }
        }
    }

    fn publish(&self) {
        match serde_json::to_value(&self.state) {
            Ok(v) => self.bus.publish(SERVICE, &v),
            Err(e) => tracing::error!("serialize: {e}"),
        }
    }

    async fn notify(&self, summary: &str, body: &str, urgency: u8) {
        if let Some(c) = &self.session {
            let notice = Notice {
                summary,
                body,
                urgency,
                ..Default::default()
            };
            if let Err(e) = notify::send(c, notice).await {
                tracing::warn!("notify: {e:#}");
            }
        }
    }

    async fn start(&mut self, mode: &str, region: &str, output: bool, input: bool) -> Result<()> {
        if self.active.is_some() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.state.dir)?;
        let name = format!("Recording_{}.mp4", capture::timestamp());
        let path = Path::new(&self.state.dir).join(name);
        let args = arguments(mode, region, output, input, &path)?;

        let mut child = Process::new(BIN)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("{BIN} not found"))?;
        let stderr = child.stderr.take().context("no stderr")?;

        self.active = Some(Active {
            child,
            path,
            stderr: tokio::spawn(tail(stderr)),
            deadline: None,
        });
        self.state.recording = true;
        self.state.started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        self.state.error.clear();
        self.publish();
        self.notify("Screen Recorder", "Recording started", 1).await;
        Ok(())
    }

    fn stop(&mut self) {
        let Some(active) = &mut self.active else {
            return;
        };
        if active.deadline.is_some() {
            return;
        }
        if let Some(pid) = active.child.id() {
            // SAFETY: a plain signal to our own child, which hasn't been reaped.
            unsafe { libc::kill(pid as i32, libc::SIGINT) };
        }
        active.deadline = Some(Instant::now() + STOP_TIMEOUT);
    }

    fn kill(&mut self) {
        if let Some(active) = &mut self.active {
            tracing::warn!("recorder ignored the stop, killing it");
            active.deadline = None;
            let _ = active.child.start_kill();
        }
    }

    async fn finished(&mut self, status: std::io::Result<ExitStatus>) {
        let Some(active) = self.active.take() else {
            return;
        };
        let log = active.stderr.await.unwrap_or_default();
        let size = std::fs::metadata(&active.path).map_or(0, |m| m.len());
        let asked = active.deadline.is_some();
        let clean = status.as_ref().is_ok_and(ExitStatus::success);

        self.state.recording = false;
        self.state.started = 0;
        self.publish();

        if size > 0 && (clean || asked) {
            let body = format!("Recording saved to {}", self.state.dir);
            self.notify("Screen Recorder", &body, 1).await;
            return;
        }
        // Whatever is left is a header at most.
        let _ = std::fs::remove_file(&active.path);
        if !log.contains(CANCELLED) {
            let reason = log.lines().last().unwrap_or("it stopped on its own");
            self.failed(reason).await;
        }
    }

    async fn failed(&mut self, reason: &str) {
        tracing::warn!("recorder: {reason}");
        self.state.error = reason.to_string();
        self.publish();
        self.notify("Screen Recorder Error", reason, 2).await;
    }
}

async fn wait(active: &mut Option<Active>) -> std::io::Result<ExitStatus> {
    match active {
        Some(a) => a.child.wait().await,
        None => std::future::pending().await,
    }
}

async fn overdue(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

// The pipe has to keep draining or the recorder blocks on a full one.
async fn tail(stderr: ChildStderr) -> String {
    let mut lines = BufReader::new(stderr).lines();
    let mut last = VecDeque::with_capacity(STDERR_LINES);
    while let Ok(Some(line)) = lines.next_line().await {
        if last.len() == STDERR_LINES {
            last.pop_front();
        }
        last.push_back(line);
    }
    Vec::from(last).join("\n")
}

fn direct(nixos: &Path, wrapper: &Path) -> bool {
    !nixos.exists() || wrapper.exists()
}

fn valid_region(region: &str) -> bool {
    let number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let offset = |s: &str| number(s.strip_prefix('-').unwrap_or(s));
    let Some((size, position)) = region.split_once('+') else {
        return false;
    };
    let Some(((w, h), (x, y))) = size.split_once('x').zip(position.split_once('+')) else {
        return false;
    };
    number(w) && number(h) && offset(x) && offset(y)
}

fn arguments(
    mode: &str,
    region: &str,
    output: bool,
    input: bool,
    path: &Path,
) -> Result<Vec<String>> {
    let mut args: Vec<String> = ["-f", "60", "-w", mode]
        .map(String::from)
        .into_iter()
        .collect();
    match mode {
        "portal" | "screen" => {}
        "region" if valid_region(region) => {
            args.extend(["-region".to_string(), region.to_string()]);
        }
        "region" => bail!("bad region: {region}"),
        _ => bail!("bad mode: {mode}"),
    }

    let sources: Vec<&str> = [
        output.then_some("default_output"),
        input.then_some("default_input"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !sources.is_empty() {
        args.extend(["-a".to_string(), sources.join("|")]);
    }

    // A slow recording beats none when the GPU's encoder isn't usable.
    args.extend(["-fallback-cpu-encoding".to_string(), "yes".to_string()]);
    args.extend(["-o".to_string(), path.to_string_lossy().into_owned()]);
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(mode: &str, region: &str, output: bool, input: bool) -> Result<String> {
        arguments(mode, region, output, input, Path::new("/v/a.mp4")).map(|a| a.join(" "))
    }

    #[test]
    fn a_plain_portal_recording() {
        assert_eq!(
            joined("portal", "", false, false).unwrap(),
            "-f 60 -w portal -fallback-cpu-encoding yes -o /v/a.mp4"
        );
    }

    #[test]
    fn a_region_carries_its_geometry() {
        assert_eq!(
            joined("region", "400x300+10+20", false, false).unwrap(),
            "-f 60 -w region -region 400x300+10+20 -fallback-cpu-encoding yes -o /v/a.mp4"
        );
    }

    #[test]
    fn audio_merges_into_one_track() {
        let args = joined("screen", "", true, true).unwrap();
        assert!(args.contains("-a default_output|default_input "));
        let args = joined("screen", "", false, true).unwrap();
        assert!(args.contains("-a default_input "));
    }

    #[test]
    fn regions_are_checked() {
        assert!(valid_region("1x1+0+0"));
        assert!(valid_region("400x300+-1920+0"));
        for bad in [
            "",
            "400x300",
            "400x300+1",
            "ax1+0+0",
            "1x1+0+0;rm",
            "1x1+0+0+0",
        ] {
            assert!(!valid_region(bad), "{bad}");
            assert!(joined("region", bad, false, false).is_err());
        }
    }

    #[test]
    fn unknown_modes_are_refused() {
        assert!(joined("focused", "", false, false).is_err());
        assert!(joined("-h", "", false, false).is_err());
    }

    #[test]
    fn direct_capture_on_nixos_needs_the_wrapper() {
        let here = Path::new("/");
        let nowhere = Path::new("/no/such/path");
        assert!(direct(nowhere, nowhere));
        assert!(direct(here, here));
        assert!(!direct(here, nowhere));
    }

    #[test]
    fn commands_parse() {
        let start: Command =
            serde_json::from_str(r#"{"cmd":"start","args":{"mode":"screen","output":true}}"#)
                .unwrap();
        assert_eq!(
            start,
            Command::Start {
                mode: "screen".to_string(),
                region: String::new(),
                output: true,
                input: false,
            }
        );
        let stop: Command = serde_json::from_str(r#"{"cmd":"stop"}"#).unwrap();
        assert_eq!(stop, Command::Stop);
    }

    #[tokio::test]
    async fn stderr_keeps_only_the_end() {
        let mut child = Process::new("sh")
            .args(["-c", "seq 1 500 >&2"])
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let log = tail(child.stderr.take().unwrap()).await;
        child.wait().await.unwrap();
        assert_eq!(log.lines().count(), STDERR_LINES);
        assert_eq!(log.lines().last(), Some("500"));
    }
}
