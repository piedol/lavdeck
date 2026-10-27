//! Runs one plugin job: launch the executable, send the request, stream its messages.

use std::fmt;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::plugins::InstalledPlugin;
use crate::protocol::{PROTOCOL_VERSION, PluginMessage, PluginMessageKind, PluginRequest};
use crate::spawn::{self, SpawnedProcess};

#[derive(Debug, PartialEq, Eq)]
pub enum PluginRunError {
    /// The executable couldn't be started, in Foundation's words for the failure.
    Launch(String),
    /// The plugin reported an `error` message (its text, if it gave one).
    Reported(Option<String>),
    /// The plugin exited non-zero; the last few log lines.
    Exited {
        status: i32,
        log_tail: Vec<String>,
    },
    Cancelled,
}

impl fmt::Display for PluginRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Launch(message) => write!(f, "Couldn’t start the plugin: {message}"),
            Self::Reported(Some(message)) => f.write_str(message),
            Self::Reported(None) => f.write_str("The plugin reported an error"),
            Self::Exited { status, log_tail } => {
                write!(f, "The plugin exited with status {status}.")?;
                if !log_tail.is_empty() {
                    write!(f, "\n{}", log_tail.join("\n"))?;
                }
                Ok(())
            }
            Self::Cancelled => f.write_str("Cancelled"),
        }
    }
}

impl std::error::Error for PluginRunError {}

/// What a finished job produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginRunResult {
    /// The plugin's `output` messages.
    pub outputs: Vec<PluginMessage>,
    /// Non-JSON stdout lines, stderr lines and nothing else.
    pub log: Vec<String>,
}

/// Cancels a running job: the plugin gets SIGTERM.
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<CancelInner>,
}

#[derive(Default)]
struct CancelInner {
    cancelled: AtomicBool,
    process: Mutex<Option<Arc<SpawnedProcess>>>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        if let Some(process) = self.inner.process.lock().unwrap().as_ref() {
            process.terminate();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }
}

/// A job started with [`spawn`].
pub struct PluginJob {
    cancel: CancelToken,
}

impl PluginJob {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn token(&self) -> CancelToken {
        self.cancel.clone()
    }
}

/// Runs `request` on a background thread. `on_message` sees each message as it arrives and
/// `on_finish` the outcome, both on background threads.
pub fn spawn(
    plugin: InstalledPlugin,
    request: PluginRequest,
    on_message: impl Fn(&PluginMessage) + Send + Sync + 'static,
    on_finish: impl FnOnce(Result<PluginRunResult, PluginRunError>) + Send + 'static,
) -> PluginJob {
    let cancel = CancelToken::new();
    let token = cancel.clone();
    thread::Builder::new()
        .name(format!("lavdeck-kit.plugin.{}", plugin.id()))
        .spawn(move || on_finish(run(&plugin, &request, on_message, &token)))
        .expect("spawn plugin thread");
    PluginJob { cancel }
}

/// Runs `request` and blocks until the plugin exits.
pub fn run(
    plugin: &InstalledPlugin,
    request: &PluginRequest,
    on_message: impl Fn(&PluginMessage) + Send + Sync + 'static,
    cancel: &CancelToken,
) -> Result<PluginRunResult, PluginRunError> {
    let mut extra = vec![
        (
            "LAVDECK_PLUGIN_PROTOCOL".to_string(),
            PROTOCOL_VERSION.to_string(),
        ),
        (
            "LAVDECK_PLUGIN_DIR".to_string(),
            plugin.folder.to_string_lossy().into_owned(),
        ),
    ];
    for (key, value) in &request.settings {
        extra.push((
            format!("LAVDECK_SETTING_{}", key.to_uppercase().replace('-', "_")),
            value.to_string(),
        ));
    }
    // A plugin that exits without reading stdin must not take the host down with SIGPIPE.
    spawn::ignore_sigpipe();
    let executable = plugin.executable_path();
    let mut process = SpawnedProcess::spawn(
        &executable,
        plugin.manifest.arguments.as_deref().unwrap_or_default(),
        &spawn::environment(&extra),
        &plugin.folder,
    )
    .map_err(|e| PluginRunError::Launch(launch_message(&executable, &e)))?;
    let (stdin, stdout, stderr) = (
        process.stdin.take(),
        process.stdout.take(),
        process.stderr.take(),
    );
    let process = Arc::new(process);
    *cancel.inner.process.lock().unwrap() = Some(process.clone());
    if cancel.is_cancelled() {
        process.terminate();
    }

    let collector = Arc::new(Collector::new(on_message));
    let body = request.to_json();
    let writer = thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(body.as_bytes());
        }
    });
    let out = {
        let collector = collector.clone();
        thread::spawn(move || {
            if let Some(stdout) = stdout {
                collector.read_messages(stdout);
            }
        })
    };
    let err = {
        let collector = collector.clone();
        thread::spawn(move || {
            if let Some(stderr) = stderr {
                collector.read_log(stderr);
            }
        })
    };
    let raw = process.wait();
    let _ = out.join();
    let _ = err.join();
    let _ = writer.join();
    *cancel.inner.process.lock().unwrap() = None;

    if cancel.is_cancelled() {
        return Err(PluginRunError::Cancelled);
    }
    let (messages, log) = collector.snapshot();
    if let Some(error) = messages
        .iter()
        .rev()
        .find(|m| m.kind == PluginMessageKind::Error)
    {
        return Err(PluginRunError::Reported(error.message.clone()));
    }
    let status = spawn::process_status(raw);
    if status != 0 {
        let tail = log[log.len().saturating_sub(5)..].to_vec();
        return Err(PluginRunError::Exited {
            status,
            log_tail: tail,
        });
    }
    Ok(PluginRunResult {
        outputs: messages
            .into_iter()
            .filter(|m| m.kind == PluginMessageKind::Output)
            .collect(),
        log,
    })
}

/// How Foundation words a failed launch, e.g. `The file “run” doesn’t exist.`
pub(crate) fn launch_message(executable: &std::path::Path, error: &std::io::Error) -> String {
    let name = executable
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match error.kind() {
        std::io::ErrorKind::NotFound => format!("The file “{name}” doesn’t exist."),
        std::io::ErrorKind::PermissionDenied => {
            format!(
                "The file “{name}” couldn’t be opened because you don’t have permission to view it."
            )
        }
        _ => error.to_string(),
    }
}

/// Splits a plugin's stdout into messages and log lines, and keeps its stderr as log.
struct Collector {
    state: Mutex<(Vec<PluginMessage>, Vec<String>)>,
    on_message: Box<dyn Fn(&PluginMessage) + Send + Sync>,
}

impl Collector {
    fn new(on_message: impl Fn(&PluginMessage) + Send + Sync + 'static) -> Self {
        Self {
            state: Mutex::default(),
            on_message: Box::new(on_message),
        }
    }

    fn read_messages(&self, stdout: impl Read) {
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

    fn handle(&self, line: &[u8]) {
        if line.is_empty() {
            return;
        }
        match serde_json::from_slice::<PluginMessage>(line) {
            Ok(message) => {
                self.state.lock().unwrap().0.push(message.clone());
                (self.on_message)(&message);
            }
            Err(_) => self
                .state
                .lock()
                .unwrap()
                .1
                .push(String::from_utf8_lossy(line).into_owned()),
        }
    }

    fn read_log(&self, stderr: impl Read) {
        for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
            if !line.is_empty() {
                self.state
                    .lock()
                    .unwrap()
                    .1
                    .push(String::from_utf8_lossy(&line).into_owned());
            }
        }
    }

    fn snapshot(&self) -> (Vec<PluginMessage>, Vec<String>) {
        self.state.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{JsonValue, PluginManifest, TakeDescriptor};
    use crate::testing::{make_executable, scratch_folder};
    use std::collections::BTreeMap;
    use std::time::{Duration, Instant};

    fn make_plugin(script: &str) -> InstalledPlugin {
        let folder = scratch_folder();
        make_executable(&folder.join("run"), &format!("#!/bin/sh\n{script}"));
        InstalledPlugin {
            folder,
            manifest: PluginManifest::new("test.plugin", "Test", "1", 1, "run"),
        }
    }

    fn sample_request() -> PluginRequest {
        PluginRequest::new(
            "action",
            "go",
            TakeDescriptor::new("d/s", "/tmp", "s"),
            BTreeMap::from([("model".into(), JsonValue::String("small".into()))]),
            "/tmp",
            "/tmp",
        )
    }

    #[test]
    fn plugin_runner_streams_messages() {
        let plugin = make_plugin(
            r#"read -r request
echo '{"type":"progress","fraction":0.5,"message":"half"}'
echo "not json goes to the log"
echo "{\"type\":\"output\",\"path\":\"/tmp/$LAVDECK_SETTING_MODEL.txt\",\"kind\":\"text\"}"
echo "$LAVDECK_PLUGIN_PROTOCOL $(pwd)" >&2
echo '{"type":"done"}'
"#,
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let result = run(
            &plugin,
            &sample_request(),
            move |m| sink.lock().unwrap().push(m.kind),
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(
            result
                .outputs
                .iter()
                .map(|m| m.path.clone().unwrap())
                .collect::<Vec<_>>(),
            ["/tmp/small.txt"]
        );
        assert!(result.log.contains(&"not json goes to the log".to_string()));
        let folder = std::fs::canonicalize(&plugin.folder).unwrap();
        assert!(
            result.log.contains(&format!("2 {}", folder.display())),
            "{:?}",
            result.log
        );
        assert_eq!(
            *seen.lock().unwrap(),
            [
                PluginMessageKind::Progress,
                PluginMessageKind::Output,
                PluginMessageKind::Done
            ]
        );
        let _ = std::fs::remove_dir_all(&plugin.folder);
    }

    #[test]
    fn plugin_runner_reports_errors_and_survives_unread_stdin() {
        let plugin =
            make_plugin("echo '{\"type\":\"error\",\"message\":\"model missing\"}'\nexit 1\n");
        let error = run(&plugin, &sample_request(), |_| {}, &CancelToken::new()).unwrap_err();
        assert_eq!(error.to_string(), "model missing");
        let _ = std::fs::remove_dir_all(&plugin.folder);
    }

    #[test]
    fn plugin_runner_reports_the_exit_status_with_the_log_tail() {
        let plugin = make_plugin("for i in 1 2 3 4 5 6; do echo line $i >&2; done\nexit 4\n");
        let error = run(&plugin, &sample_request(), |_| {}, &CancelToken::new()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "The plugin exited with status 4.\nline 2\nline 3\nline 4\nline 5\nline 6"
        );
        let _ = std::fs::remove_dir_all(&plugin.folder);
    }

    #[test]
    fn cancelling_terminates_the_plugin() {
        let plugin =
            make_plugin("echo '{\"type\":\"progress\",\"fraction\":0.1}'\nexec sleep 30\n");
        let (sender, receiver) = std::sync::mpsc::channel();
        let job = spawn(
            plugin.clone(),
            sample_request(),
            |_| {},
            move |result| sender.send(result).unwrap(),
        );
        std::thread::sleep(Duration::from_millis(200));
        let started = Instant::now();
        job.cancel();
        let result = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(result, Err(PluginRunError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(&plugin.folder);
    }

    #[test]
    fn a_missing_executable_is_a_launch_error() {
        let folder = scratch_folder();
        let plugin = InstalledPlugin {
            folder: folder.clone(),
            manifest: PluginManifest::new("x", "X", "1", 1, "run"),
        };
        let error = run(&plugin, &sample_request(), |_| {}, &CancelToken::new()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Couldn’t start the plugin: The file “run” doesn’t exist."
        );
        let _ = std::fs::remove_dir_all(&folder);
    }
}
