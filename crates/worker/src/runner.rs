//! The `ibc` runner binaries and their long-lived `run-batch --jsonl` processes.
//!
//! Process model: one long-lived `ibc run-batch --jsonl --jobs 1` process per busy slot, reused between
//! jobs. Measured on the reference box (rusher vs holder): a match in a warm process costs ~25 ms, a match in
//! a fresh process ~300 ms (the first match pays wasmtime start-up), so one process per job would waste
//! about 90 % of the CPU. One process per *slot* rather than one shared `--jobs C` process means a job that
//! hits its wall-clock limit can be killed without taking its neighbours down, and a crash loses one job.

use crate::protocol::Patch;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::Duration;

/// A process is retired after this many matches, which bounds whatever a long-lived runner accumulates.
const MAX_JOBS_PER_PROCESS: u32 = 500;

pub const RUNNER_FILE: &str = if cfg!(windows) { "ibc.exe" } else { "ibc" };

/// `<os>-<arch>` of this worker, the key into a job's `runner_blobs`.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Ask a runner binary which patch it is (`ibc version --json`).
pub fn probe_version(runner: &Path) -> io::Result<Patch> {
    let out = Command::new(runner)
        .args(["version", "--json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "`{} version --json` exited with {}",
            runner.display(),
            out.status
        )));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| io::Error::other(format!("`{} version --json`: {e}", runner.display())))
}

/// Which binary serves which patch.
pub struct RunnerSet {
    primary: (Patch, PathBuf),
    dir: PathBuf,
    known: Mutex<HashMap<Patch, PathBuf>>,
    /// One install at a time: concurrent jobs of a new patch must not write the same binary together.
    installing: Mutex<()>,
}

impl RunnerSet {
    /// Fails if the primary runner cannot be executed: a worker that can run nothing should not register.
    pub fn new(primary: &Path, dir: &Path) -> io::Result<RunnerSet> {
        let patch = probe_version(primary)?;
        Ok(RunnerSet {
            primary: (patch, primary.to_path_buf()),
            dir: dir.to_path_buf(),
            known: Mutex::new(HashMap::new()),
            installing: Mutex::new(()),
        })
    }

    pub fn primary_patch(&self) -> &Patch {
        &self.primary.0
    }

    fn slot(&self, patch: &Patch) -> Option<PathBuf> {
        Some(self.dir.join(patch.dir_name()?).join(RUNNER_FILE))
    }

    /// The binary for `patch`, if this worker has one. A side-by-side runner is trusted only after its own
    /// `version --json` says it is exactly that patch.
    pub fn resolve(&self, patch: &Patch) -> Option<PathBuf> {
        if *patch == self.primary.0 {
            return Some(self.primary.1.clone());
        }
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = known.get(patch) {
            return Some(p.clone());
        }
        let path = self.slot(patch)?;
        if !path.is_file() {
            return None;
        }
        match probe_version(&path) {
            Ok(found) if found == *patch => {
                known.insert(patch.clone(), path.clone());
                Some(path)
            }
            Ok(found) => {
                crate::log!(
                    "warn",
                    "runner directory does not hold the patch it is named after",
                    path = path.display().to_string(),
                    found = found
                );
                None
            }
            Err(e) => {
                crate::log!(
                    "warn",
                    "cannot execute side-by-side runner",
                    path = path.display().to_string(),
                    error = e.to_string()
                );
                None
            }
        }
    }

    /// Install a (hash-verified) runner binary for `patch` from `blob`, then resolve it as usual.
    pub fn install(&self, patch: &Patch, blob: &Path) -> io::Result<Option<PathBuf>> {
        let _one = self.installing.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(done) = self.resolve(patch) {
            return Ok(Some(done));
        }
        let dest = self
            .slot(patch)
            .ok_or_else(|| io::Error::other("patch is not a valid directory name"))?;
        let dir = dest.parent().expect("slot has a parent");
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{RUNNER_FILE}.{}.tmp", std::process::id()));
        std::fs::copy(blob, &tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&tmp, &dest)?;
        let resolved = self.resolve(patch);
        if resolved.is_none() {
            let _ = std::fs::remove_file(&dest);
        }
        Ok(resolved)
    }
}

pub enum Recv {
    Event(Value),
    Timeout,
    /// The process closed its stdout: it exited or crashed.
    Exited,
}

/// One `ibc run-batch --jsonl --jobs 1` process. Dropping it kills the process.
pub struct RunnerProc {
    child: Child,
    stdin: Option<ChildStdin>,
    events: Receiver<Value>,
    runner: PathBuf,
    jobs_run: u32,
}

impl RunnerProc {
    fn spawn(runner: &Path) -> io::Result<RunnerProc> {
        let mut command = Command::new(runner);
        // Own process group: a terminal Ctrl-C must reach only the worker, which drains; the runners finish
        // their matches. A runner whose worker died sees end of input and exits by itself.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0000_0200); // CREATE_NEW_PROCESS_GROUP
        let mut child = command
            .args(["run-batch", "--jsonl", "--jobs", "1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let pid = child.id();
        let (tx, events) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<Value>(&line) {
                    Ok(v) => {
                        if tx.send(v).is_err() {
                            break;
                        }
                    }
                    Err(_) => crate::log!(
                        "warn",
                        "runner wrote a non-JSON line on stdout",
                        pid = pid,
                        line = line
                    ),
                }
            }
        });
        // The runner's free-form stderr becomes structured log lines instead of corrupting ours.
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                crate::log!("warn", "runner stderr", pid = pid, line = line);
            }
        });
        Ok(RunnerProc {
            stdin: child.stdin.take(),
            child,
            events,
            runner: runner.to_path_buf(),
            jobs_run: 0,
        })
    }

    /// Send one job line (see docs/JOBS.md part A).
    pub fn submit(&mut self, job: &Value) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("runner stdin is closed"))?;
        self.jobs_run += 1;
        stdin.write_all(format!("{job}\n").as_bytes())?;
        stdin.flush()
    }

    pub fn recv(&self, wait: Duration) -> Recv {
        match self.events.recv_timeout(wait) {
            Ok(v) => Recv::Event(v),
            Err(RecvTimeoutError::Timeout) => Recv::Timeout,
            Err(RecvTimeoutError::Disconnected) => Recv::Exited,
        }
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for RunnerProc {
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Idle runner processes, kept warm between jobs.
pub struct RunnerPool {
    idle: Mutex<Vec<RunnerProc>>,
    max_idle: usize,
}

impl RunnerPool {
    pub fn new(max_idle: usize) -> RunnerPool {
        RunnerPool {
            idle: Mutex::new(Vec::new()),
            max_idle,
        }
    }

    /// A warm process of `runner` if one is idle, else a new one.
    pub fn take(&self, runner: &Path) -> io::Result<RunnerProc> {
        loop {
            let found = {
                let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
                idle.iter()
                    .rposition(|p| p.runner == runner)
                    .map(|i| idle.remove(i))
            };
            let Some(mut p) = found else {
                return RunnerProc::spawn(runner);
            };
            if p.alive() {
                return Ok(p);
            }
        }
    }

    /// Return a process that finished its job cleanly. Anything else is simply dropped (and so killed).
    pub fn put(&self, proc_: RunnerProc) {
        if proc_.jobs_run >= MAX_JOBS_PER_PROCESS {
            return;
        }
        let evicted = {
            let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
            idle.push(proc_);
            (idle.len() > self.max_idle).then(|| idle.remove(0))
        };
        drop(evicted);
    }

    pub fn shutdown(&self) {
        let procs = std::mem::take(&mut *self.idle.lock().unwrap_or_else(|e| e.into_inner()));
        drop(procs);
    }
}
