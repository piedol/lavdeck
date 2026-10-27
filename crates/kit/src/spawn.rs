//! A child process started with posix_spawn, with pipes on stdin, stdout and stderr. It runs
//! under Lavdeck's privacy permissions (nothing disclaims responsibility), so plugins share
//! Lavdeck's grants instead of prompting on their own.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

#[derive(Default)]
struct ExitState {
    /// The child has exited (it may not be reaped yet). Signals are only sent before this.
    exited: bool,
    /// Raw `waitpid` status once reaped.
    raw: Option<i32>,
}

pub(crate) struct SpawnedProcess {
    pid: libc::pid_t,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
    exit: Arc<(Mutex<ExitState>, Condvar)>,
}

impl SpawnedProcess {
    pub(crate) fn spawn(
        executable: &Path,
        arguments: &[String],
        environment: &[(String, String)],
        directory: &Path,
    ) -> io::Result<Self> {
        let cstr = |bytes: &[u8]| {
            CString::new(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
        };
        let program = cstr(executable.as_os_str().as_bytes())?;
        let dir = cstr(directory.as_os_str().as_bytes())?;
        let mut argv = vec![program.clone()];
        for argument in arguments {
            argv.push(cstr(OsStr::new(argument).as_bytes())?);
        }
        let envp: Vec<CString> = environment
            .iter()
            .filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok())
            .collect();
        let mut argv_ptrs: Vec<*mut libc::c_char> =
            argv.iter().map(|a| a.as_ptr().cast_mut()).collect();
        argv_ptrs.push(std::ptr::null_mut());
        let mut envp_ptrs: Vec<*mut libc::c_char> =
            envp.iter().map(|e| e.as_ptr().cast_mut()).collect();
        envp_ptrs.push(std::ptr::null_mut());

        let (stdin_read, stdin_write) = pipe()?;
        let (stdout_read, stdout_write) = pipe()?;
        let (stderr_read, stderr_write) = pipe()?;
        let mut pid: libc::pid_t = 0;
        // SAFETY: posix_spawn with attributes and file actions we initialise and destroy here; the
        // argument and environment arrays are NUL-terminated and outlive the call.
        let result = unsafe {
            let mut attributes: libc::posix_spawnattr_t = std::ptr::null_mut();
            libc::posix_spawnattr_init(&mut attributes);
            // Only fds 0-2 reach the child, and it starts with default signal handling (Lavdeck
            // ignores SIGPIPE; a plugin shouldn't inherit that).
            libc::posix_spawnattr_setflags(
                &mut attributes,
                (libc::POSIX_SPAWN_CLOEXEC_DEFAULT | libc::POSIX_SPAWN_SETSIGDEF) as _,
            );
            let mut defaults: libc::sigset_t = 0;
            libc::sigemptyset(&mut defaults);
            libc::sigaddset(&mut defaults, libc::SIGPIPE);
            libc::posix_spawnattr_setsigdefault(&mut attributes, &defaults);
            let mut actions: libc::posix_spawn_file_actions_t = std::ptr::null_mut();
            libc::posix_spawn_file_actions_init(&mut actions);
            libc::posix_spawn_file_actions_adddup2(&mut actions, stdin_read, 0);
            libc::posix_spawn_file_actions_adddup2(&mut actions, stdout_write, 1);
            libc::posix_spawn_file_actions_adddup2(&mut actions, stderr_write, 2);
            libc::posix_spawn_file_actions_addchdir_np(&mut actions, dir.as_ptr());
            let result = libc::posix_spawn(
                &mut pid,
                program.as_ptr(),
                &actions,
                &attributes,
                argv_ptrs.as_ptr(),
                envp_ptrs.as_ptr(),
            );
            libc::posix_spawn_file_actions_destroy(&mut actions);
            libc::posix_spawnattr_destroy(&mut attributes);
            result
        };
        // The child has its own copies; close ours so EOF works both ways.
        for fd in [stdin_read, stdout_write, stderr_write] {
            // SAFETY: closing descriptors we own.
            unsafe { libc::close(fd) };
        }
        // SAFETY: wrapping descriptors we own exactly once.
        let (stdin, stdout, stderr) = unsafe {
            (
                File::from_raw_fd(stdin_write),
                File::from_raw_fd(stdout_read),
                File::from_raw_fd(stderr_read),
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result));
        }

        let exit: Arc<(Mutex<ExitState>, Condvar)> = Arc::default();
        let waiter = exit.clone();
        thread::Builder::new()
            .name("lavdeck-kit.spawn.wait".into())
            .spawn(move || {
                // Wait without reaping, so `terminate` can never signal a recycled pid.
                loop {
                    // SAFETY: waitid fills the siginfo we pass.
                    let r = unsafe {
                        let mut info: libc::siginfo_t = std::mem::zeroed();
                        libc::waitid(
                            libc::P_PID,
                            pid as libc::id_t,
                            &mut info,
                            libc::WEXITED | libc::WNOWAIT,
                        )
                    };
                    if r == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                        break;
                    }
                }
                let (lock, wake) = &*waiter;
                let mut state = lock.lock().unwrap();
                state.exited = true;
                let mut raw = 0;
                // SAFETY: reaping our own child.
                while unsafe { libc::waitpid(pid, &mut raw, 0) } == -1
                    && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
                {
                }
                state.raw = Some(raw);
                wake.notify_all();
            })?;
        Ok(Self {
            pid,
            stdin: Some(stdin),
            stdout: Some(stdout),
            stderr: Some(stderr),
            exit,
        })
    }

    /// Sends SIGTERM if the process is still running.
    pub(crate) fn terminate(&self) {
        let state = self.exit.0.lock().unwrap();
        if !state.exited {
            // SAFETY: the pid is ours and not yet reaped (checked under the lock the reaper holds).
            unsafe { libc::kill(self.pid, libc::SIGTERM) };
        }
    }

    /// Raw wait status once the process has exited, waiting at most `timeout`.
    #[cfg(test)]
    pub(crate) fn wait_timeout(&self, timeout: std::time::Duration) -> Option<i32> {
        let deadline = std::time::Instant::now() + timeout;
        let (lock, wake) = &*self.exit;
        let mut state = lock.lock().unwrap();
        while state.raw.is_none() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            state = wake.wait_timeout(state, left).unwrap().0;
        }
        state.raw
    }

    pub(crate) fn wait(&self) -> i32 {
        let (lock, wake) = &*self.exit;
        let mut state = lock.lock().unwrap();
        loop {
            if let Some(raw) = state.raw {
                return raw;
            }
            state = wake.wait(state).unwrap();
        }
    }

    /// The raw status once exited, without waiting.
    pub(crate) fn raw_status(&self) -> Option<i32> {
        self.exit.0.lock().unwrap().raw
    }
}

/// Exit code, or 128 + signal for a process killed by a signal.
pub(crate) fn shell_status(raw: i32) -> i32 {
    if raw & 0x7f == 0 {
        (raw >> 8) & 0xff
    } else {
        128 + (raw & 0x7f)
    }
}

/// Exit code, or the signal number for a process killed by a signal (as `Process` reports it).
pub(crate) fn process_status(raw: i32) -> i32 {
    if raw & 0x7f == 0 {
        (raw >> 8) & 0xff
    } else {
        raw & 0x7f
    }
}

fn pipe() -> io::Result<(RawFd, RawFd)> {
    let mut fds = [0; 2];
    // SAFETY: pipe fills two descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for fd in fds {
        // SAFETY: setting close-on-exec on descriptors we just made.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    Ok((fds[0], fds[1]))
}

/// The C library's words for an error, without Rust's `(os error N)`.
pub(crate) fn strerror(error: &io::Error) -> String {
    match error.raw_os_error() {
        // SAFETY: strerror returns a static C string.
        Some(code) => unsafe { std::ffi::CStr::from_ptr(libc::strerror(code)) }
            .to_string_lossy()
            .into_owned(),
        None => error.to_string(),
    }
}

/// The current environment plus `extra`, which wins.
pub(crate) fn environment(extra: &[(String, String)]) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect();
    env.retain(|(k, _)| !extra.iter().any(|(e, _)| e == k));
    env.extend(extra.iter().cloned());
    env
}

/// Ignores SIGPIPE once, so a plugin that exits without reading its input can't take Lavdeck down.
pub(crate) fn ignore_sigpipe() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: setting a signal disposition.
    ONCE.call_once(|| unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::time::Duration;

    #[test]
    fn pipes_reach_the_child_and_status_comes_back() {
        let mut process = SpawnedProcess::spawn(
            Path::new("/bin/sh"),
            &[
                "-c".into(),
                "read line; echo \"got $line in $(pwd)\"; echo oops >&2; exit 3".into(),
            ],
            &environment(&[]),
            Path::new("/usr"),
        )
        .unwrap();
        let mut stdin = process.stdin.take().unwrap();
        stdin.write_all(b"hello\n").unwrap();
        drop(stdin);
        let mut out = String::new();
        process
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        let mut err = String::new();
        process
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        assert_eq!(out, "got hello in /usr\n");
        assert_eq!(err, "oops\n");
        assert_eq!(shell_status(process.wait()), 3);
    }

    #[test]
    fn terminate_stops_a_running_child() {
        let process = SpawnedProcess::spawn(
            Path::new("/bin/sleep"),
            &["30".into()],
            &environment(&[]),
            Path::new("/"),
        )
        .unwrap();
        assert_eq!(process.wait_timeout(Duration::from_millis(100)), None);
        process.terminate();
        let raw = process.wait_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            (shell_status(raw), process_status(raw)),
            (128 + libc::SIGTERM, libc::SIGTERM)
        );
        // After exit, terminating again is harmless.
        process.terminate();
    }

    #[test]
    fn a_missing_executable_fails_to_spawn() {
        let error = SpawnedProcess::spawn(Path::new("/no/such/plugin"), &[], &[], Path::new("/"))
            .err()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }
}
