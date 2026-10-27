//! Runs every enabled capture plugin for one recording: start them together, stop them together,
//! collect what they wrote.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::capture_protocol::{
    CaptureCommand, CaptureCommandKind, CaptureEvent, CaptureEventKind, CaptureTake, HostClock,
};
use crate::plugins::InstalledPlugin;
use crate::protocol::{JsonValue, PROTOCOL_VERSION};
use crate::spawn::{self, SpawnedProcess};

/// One file a capture plugin (or Lavdeck's own video capture) recorded for a take.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureOutput {
    pub plugin_id: String,
    pub path: String,
    pub source: String,
    pub kind: String,
    /// Host-clock seconds of the file's first frame.
    pub start_host_time: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CaptureProblemKind {
    /// The capture process couldn't be launched.
    Launch(String),
    /// It was still writing after `stop` and the timeout, and was terminated.
    Timeout { seconds: u64 },
    /// It exited non-zero without reporting an error; the last few log lines.
    Exited { status: i32, log_tail: Vec<String> },
    /// An `error` the plugin reported, or any other message.
    Reported(String),
}

/// A capture source that went wrong, for the deck and the take's notes.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureProblem {
    pub plugin_name: String,
    pub kind: CaptureProblemKind,
}

impl CaptureProblem {
    pub fn new(plugin_name: &str, message: &str) -> Self {
        Self {
            plugin_name: plugin_name.into(),
            kind: CaptureProblemKind::Reported(message.into()),
        }
    }

    /// The problem in words, without the plugin's name.
    pub fn message(&self) -> String {
        match &self.kind {
            CaptureProblemKind::Launch(m) => format!("Couldn’t start: {m}"),
            CaptureProblemKind::Timeout { seconds } => {
                format!("Didn’t finish writing within {seconds} seconds and was stopped.")
            }
            CaptureProblemKind::Exited { status, log_tail } => {
                format!("Exited with status {status}. {}", log_tail.join(" "))
            }
            CaptureProblemKind::Reported(m) => m.clone(),
        }
    }
}

impl fmt::Display for CaptureProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.plugin_name, self.message())
    }
}

/// What [`CaptureHost::stop`] returns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CaptureResult {
    pub outputs: Vec<CaptureOutput>,
    pub problems: Vec<CaptureProblem>,
}

type EventCallback = Arc<dyn Fn(&InstalledPlugin, &CaptureEvent) + Send + Sync>;

struct Running {
    plugin: InstalledPlugin,
    process: Arc<SpawnedProcess>,
    stdin: Option<File>,
    collector: Arc<Collector>,
    /// Set once the process has exited and its stdout is drained.
    finished: Arc<(Mutex<bool>, Condvar)>,
}

/// Capture processes for the current take.
pub struct CaptureHost {
    running: Mutex<Vec<Running>>,
    on_event: EventCallback,
}

impl CaptureHost {
    /// `on_event` sees every event as it arrives (for status and errors in the UI), on a background thread.
    pub fn new(on_event: impl Fn(&InstalledPlugin, &CaptureEvent) + Send + Sync + 'static) -> Self {
        Self {
            running: Mutex::new(Vec::new()),
            on_event: Arc::new(on_event),
        }
    }

    pub fn is_running(&self) -> bool {
        !self.running.lock().unwrap().is_empty()
    }

    /// Starts each capture plugin's process and sends it `start`. Plugins that fail to launch are
    /// reported and skipped; the recording goes ahead without them. `granted` lists the
    /// permissions Lavdeck holds; each plugin is told which of its own are granted.
    pub fn start(
        &self,
        plugins: &[(InstalledPlugin, BTreeMap<String, JsonValue>)],
        take: &CaptureTake,
        granted: &BTreeSet<String>,
    ) -> Vec<CaptureProblem> {
        let mut problems = Vec::new();
        let host_time = HostClock::now();
        for (plugin, settings) in plugins {
            let Some(spec) = plugin
                .manifest
                .capture
                .as_ref()
                .filter(|_| plugin.manifest.is_capture())
            else {
                continue;
            };
            let extra = [
                (
                    "LAVDECK_PLUGIN_PROTOCOL".to_string(),
                    PROTOCOL_VERSION.to_string(),
                ),
                (
                    "LAVDECK_PLUGIN_DIR".to_string(),
                    plugin.folder.to_string_lossy().into_owned(),
                ),
            ];
            spawn::ignore_sigpipe();
            let executable = plugin.folder.join(
                spec.executable
                    .as_deref()
                    .unwrap_or(&plugin.manifest.executable),
            );
            let mut process = match SpawnedProcess::spawn(
                &executable,
                spec.arguments.as_deref().unwrap_or_default(),
                &spawn::environment(&extra),
                &plugin.folder,
            ) {
                Ok(process) => process,
                Err(error) => {
                    problems.push(CaptureProblem {
                        plugin_name: plugin.manifest.name.clone(),
                        kind: CaptureProblemKind::Launch(spawn::strerror(&error)),
                    });
                    continue;
                }
            };
            let (stdin, stdout, stderr) = (
                process.stdin.take(),
                process.stdout.take(),
                process.stderr.take(),
            );
            let process = Arc::new(process);
            let collector = Arc::new(Collector::new(plugin.clone(), self.on_event.clone()));
            let finished: Arc<(Mutex<bool>, Condvar)> = Arc::default();
            let (log_done, log_finished) = std::sync::mpsc::channel::<()>();
            if let Some(stderr) = stderr {
                let collector = collector.clone();
                thread::spawn(move || {
                    collector.read_log(stderr);
                    let _ = log_done.send(());
                });
            }
            {
                let (collector, process, finished) =
                    (collector.clone(), process.clone(), finished.clone());
                thread::spawn(move || {
                    if let Some(stdout) = stdout {
                        collector.read_events(stdout);
                    }
                    process.wait();
                    // Give stderr a moment to drain, without waiting on a child that inherited it.
                    let _ = log_finished.recv_timeout(Duration::from_millis(200));
                    *finished.0.lock().unwrap() = true;
                    finished.1.notify_all();
                });
            }
            let mut command = CaptureCommand::new(CaptureCommandKind::Start);
            command.take = Some(take.clone());
            command.settings = Some(settings.clone());
            command.plugin_directory = Some(plugin.folder.to_string_lossy().into_owned());
            command.host_time = Some(host_time);
            command.granted = Some(
                plugin
                    .manifest
                    .permissions
                    .iter()
                    .flatten()
                    .filter(|p| granted.contains(*p))
                    .cloned()
                    .collect(),
            );
            let mut stdin = stdin;
            write_line(&mut stdin, &command);
            self.running.lock().unwrap().push(Running {
                plugin: plugin.clone(),
                process,
                stdin,
                collector,
                finished,
            });
        }
        problems
    }

    /// Sends `stop` to every capture process and waits (up to `timeout` each) for them to finish
    /// writing. Returns the files they reported and any problems. Blocks.
    pub fn stop(&self, timeout: Duration) -> CaptureResult {
        let mut all = std::mem::take(&mut *self.running.lock().unwrap());
        for r in &mut all {
            write_line(&mut r.stdin, &CaptureCommand::stop());
            r.stdin = None;
        }
        let mut result = CaptureResult::default();
        for r in all {
            let deadline = Instant::now() + timeout;
            let (lock, wake) = &*r.finished;
            let mut done = lock.lock().unwrap();
            while !*done {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                done = wake.wait_timeout(done, left).unwrap().0;
            }
            let finished = *done;
            drop(done);
            let name = r.plugin.manifest.name.clone();
            if !finished {
                r.process.terminate();
                result.problems.push(CaptureProblem {
                    plugin_name: name.clone(),
                    kind: CaptureProblemKind::Timeout {
                        seconds: timeout.as_secs(),
                    },
                });
            }
            let (outputs, errors, log) = r.collector.snapshot();
            result.outputs.extend(outputs);
            let reported = !errors.is_empty();
            result
                .problems
                .extend(errors.into_iter().map(|m| CaptureProblem::new(&name, &m)));
            if finished
                && !reported
                && let Some(raw) = r.process.raw_status()
            {
                let status = spawn::shell_status(raw);
                if status != 0 {
                    let tail = log[log.len().saturating_sub(3)..].to_vec();
                    result.problems.push(CaptureProblem {
                        plugin_name: name,
                        kind: CaptureProblemKind::Exited {
                            status,
                            log_tail: tail,
                        },
                    });
                }
            }
        }
        result
    }
}

fn write_line(stdin: &mut Option<File>, command: &CaptureCommand) {
    if let Some(stdin) = stdin {
        let _ = stdin.write_all(format!("{}\n", command.to_json()).as_bytes());
    }
}

#[derive(Default)]
struct Collected {
    outputs: Vec<CaptureOutput>,
    errors: Vec<String>,
    log: Vec<String>,
}

/// Splits a capture process's stdout into events.
struct Collector {
    plugin: InstalledPlugin,
    on_event: EventCallback,
    state: Mutex<Collected>,
}

impl Collector {
    fn new(plugin: InstalledPlugin, on_event: EventCallback) -> Self {
        Self {
            plugin,
            on_event,
            state: Mutex::default(),
        }
    }

    fn read_events(&self, stdout: File) {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            self.handle(&line);
            line.clear();
        }
    }

    fn read_log(&self, stderr: File) {
        for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
            if !line.is_empty() {
                self.state
                    .lock()
                    .unwrap()
                    .log
                    .push(String::from_utf8_lossy(&line).into_owned());
            }
        }
    }

    fn handle(&self, line: &[u8]) {
        if line.is_empty() {
            return;
        }
        let Ok(event) = serde_json::from_slice::<CaptureEvent>(line) else {
            self.state
                .lock()
                .unwrap()
                .log
                .push(String::from_utf8_lossy(line).into_owned());
            return;
        };
        {
            let mut state = self.state.lock().unwrap();
            match event.kind {
                CaptureEventKind::Output => {
                    if let Some(path) = &event.path {
                        state.outputs.push(CaptureOutput {
                            plugin_id: self.plugin.id().into(),
                            path: path.clone(),
                            source: event.source.clone().unwrap_or_else(|| "video".into()),
                            kind: event.media_kind.clone().unwrap_or_else(|| "video".into()),
                            start_host_time: event.start_host_time,
                        });
                    }
                }
                CaptureEventKind::Error => state.errors.push(
                    event
                        .message
                        .clone()
                        .unwrap_or_else(|| "Unknown error".into()),
                ),
                CaptureEventKind::Log => state.log.push(event.message.clone().unwrap_or_default()),
                CaptureEventKind::Started | CaptureEventKind::Stopped => {}
            }
        }
        (self.on_event)(&self.plugin, &event);
    }

    fn snapshot(&self) -> (Vec<CaptureOutput>, Vec<String>, Vec<String>) {
        let state = self.state.lock().unwrap();
        (
            state.outputs.clone(),
            state.errors.clone(),
            state.log.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_protocol::CaptureSpec;
    use crate::protocol::PluginManifest;
    use crate::testing::{make_executable, scratch_folder};

    fn capture_plugin(script: &str) -> InstalledPlugin {
        let folder = scratch_folder();
        make_executable(&folder.join("capture"), &format!("#!/bin/sh\n{script}"));
        let mut manifest = PluginManifest::new("test.capture", "Capture", "1", 2, "capture");
        manifest.kind = Some("capture".into());
        manifest.capture = Some(CaptureSpec {
            arguments: Some(vec![]),
            ..CaptureSpec::default()
        });
        manifest.permissions = Some(vec!["camera".into(), "screen-recording".into()]);
        InstalledPlugin { folder, manifest }
    }

    #[test]
    fn capture_host_starts_stops_and_collects_outputs() {
        let plugin = capture_plugin(
            r#"read -r start
case "$start" in *'"type":"start"'*'"granted":["camera"]'*) ;; *) echo '{"type":"error","message":"expected start"}'; exit 1;; esac
echo '{"type":"started","source":"screen","startHostTime":100.5}'
read -r stop
echo '{"type":"output","source":"screen","kind":"video","path":"/tmp/x Screen.mov","startHostTime":100.5}'
echo '{"type":"stopped"}'
"#,
        );
        assert!(plugin.manifest.is_capture());
        let started = Arc::new(Mutex::new(Vec::new()));
        let sink = started.clone();
        let host = CaptureHost::new(move |_, event| {
            if event.kind == CaptureEventKind::Started {
                sink.lock()
                    .unwrap()
                    .push(event.source.clone().unwrap_or_default());
            }
        });
        let take = CaptureTake {
            folder: "/tmp".into(),
            stem: "x".into(),
        };
        let problems = host.start(
            &[(plugin.clone(), BTreeMap::new())],
            &take,
            &BTreeSet::from(["camera".to_string()]),
        );
        assert!(problems.is_empty());
        assert!(host.is_running());
        thread::sleep(Duration::from_millis(300));
        let result = host.stop(Duration::from_secs(5));
        assert!(result.problems.is_empty(), "{:?}", result.problems);
        assert_eq!(
            result.outputs,
            [CaptureOutput {
                plugin_id: "test.capture".into(),
                path: "/tmp/x Screen.mov".into(),
                source: "screen".into(),
                kind: "video".into(),
                start_host_time: Some(100.5),
            }]
        );
        assert_eq!(*started.lock().unwrap(), ["screen"]);
        assert!(!host.is_running());
        let _ = std::fs::remove_dir_all(&plugin.folder);
    }

    #[test]
    fn slow_and_failing_capture_plugins_are_reported() {
        let slow = capture_plugin("read -r start\nread -r stop\nexec sleep 30\n");
        let failing = capture_plugin("read -r start\necho 'camera gone' >&2\nexit 2\n");
        let host = CaptureHost::new(|_, _| {});
        let take = CaptureTake {
            folder: "/tmp".into(),
            stem: "x".into(),
        };
        host.start(
            &[
                (slow.clone(), BTreeMap::new()),
                (failing.clone(), BTreeMap::new()),
            ],
            &take,
            &BTreeSet::new(),
        );
        thread::sleep(Duration::from_millis(200));
        let result = host.stop(Duration::from_secs(1));
        let messages: Vec<String> = result.problems.iter().map(ToString::to_string).collect();
        assert_eq!(
            messages,
            [
                "Capture: Didn’t finish writing within 1 seconds and was stopped.",
                "Capture: Exited with status 2. camera gone"
            ]
        );
        for plugin in [slow, failing] {
            let _ = std::fs::remove_dir_all(&plugin.folder);
        }
    }

    #[test]
    fn a_capture_plugin_that_cannot_launch_is_skipped() {
        let mut plugin = capture_plugin("exit 0\n");
        plugin.manifest.capture = Some(CaptureSpec {
            executable: Some("missing".into()),
            ..CaptureSpec::default()
        });
        let host = CaptureHost::new(|_, _| {});
        let take = CaptureTake {
            folder: "/tmp".into(),
            stem: "x".into(),
        };
        let problems = host.start(
            &[(plugin.clone(), BTreeMap::new())],
            &take,
            &BTreeSet::new(),
        );
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message().starts_with("Couldn’t start: "));
        assert!(!host.is_running());
        let _ = std::fs::remove_dir_all(&plugin.folder);
    }
}
