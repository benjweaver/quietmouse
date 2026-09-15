//! Running quietmouse in the background, per user and without admin rights:
//! one running instance at a time, a stop request any process can make, a log
//! file for the windowless agent, and starting at sign-in on Windows.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail, ensure};

use crate::config::Config;

#[cfg(target_os = "windows")]
const AGENT: &str = "quietmoused.exe";
#[cfg(not(target_os = "windows"))]
const AGENT: &str = "quietmoused";

/// Past this size the agent's log is kept as `quietmouse.old.log` and a new one started.
const LOG_LIMIT: u64 = 1024 * 1024;
/// How long `quietmouse stop` waits. The daemon notices a stop request within one
/// device scan (two seconds), then hands buttons back to the devices.
const STOP_WAIT: Duration = Duration::from_secs(10);
const STOP_POLL: Duration = Duration::from_millis(200);

/// Per-user files: `%LOCALAPPDATA%\quietmouse` on Windows,
/// `~/Library/Application Support/quietmouse` on macOS, `~/.local/share/quietmouse` on Linux.
struct Paths {
    dir: PathBuf,
}

impl Paths {
    fn user() -> anyhow::Result<Self> {
        let dir = dirs::data_local_dir()
            .context("this system has no local data directory")?
            .join("quietmouse");
        fs::create_dir_all(&dir).with_context(|| format!("can't create {}", dir.display()))?;
        Ok(Self { dir })
    }

    fn lock(&self) -> PathBuf {
        self.dir.join("quietmouse.lock")
    }

    fn stop(&self) -> PathBuf {
        self.dir.join("quietmouse.stop")
    }

    fn log(&self) -> PathBuf {
        self.dir.join("quietmouse.log")
    }
}

/// Proof that this process is the running instance; released when dropped.
pub struct Instance {
    _lock: File,
}

/// Claims the single running instance, failing if another process holds it.
pub fn lock_instance() -> anyhow::Result<Instance> {
    acquire(&Paths::user()?.lock())
}

fn acquire(path: &Path) -> anyhow::Result<Instance> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("can't open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Instance { _lock: file }),
        Err(TryLockError::WouldBlock) => bail!("quietmouse is already running; stop it with `quietmouse stop`"),
        Err(TryLockError::Error(error)) => Err(error).with_context(|| format!("can't lock {}", path.display())),
    }
}

/// Whether some process holds the instance lock at `path`.
fn held(path: &Path) -> anyhow::Result<bool> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("can't open {}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(error)) => Err(error).with_context(|| format!("can't check {}", path.display())),
    }
}

pub fn stop_requested() -> bool {
    Paths::user().is_ok_and(|paths| paths.stop().exists())
}

pub fn clear_stop_request() {
    if let Ok(paths) = Paths::user() {
        remove_stop_file(&paths.stop());
    }
}

fn remove_stop_file(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        log::warn!("can't remove {}: {error}", path.display());
    }
}

/// Asks the running instance to hand its buttons back and exit, and waits for it.
pub fn stop() -> anyhow::Result<()> {
    let paths = Paths::user()?;
    if !held(&paths.lock())? {
        println!("quietmouse isn't running");
        return Ok(());
    }
    let request = paths.stop();
    File::create(&request).with_context(|| format!("can't create {}", request.display()))?;
    let deadline = Instant::now() + STOP_WAIT;
    while Instant::now() < deadline {
        thread::sleep(STOP_POLL);
        if !held(&paths.lock())? {
            remove_stop_file(&request);
            println!("quietmouse stopped");
            return Ok(());
        }
    }
    bail!("quietmouse didn't stop within {} seconds", STOP_WAIT.as_secs())
}

/// Starts the agent in the background, after checking the config here, where
/// mistakes can be shown (the agent can only log them).
pub fn start() -> anyhow::Result<()> {
    let paths = Paths::user()?;
    ensure!(!held(&paths.lock())?, "quietmouse is already running");
    let config = Config::resolve_path(None)?;
    ensure!(
        config.exists(),
        "no config at {}; create one with `quietmouse config --init`",
        config.display()
    );
    Config::load(&config)?;
    let agent = agent_path()?;
    // The agent outlives this command; nothing here waits for it.
    Command::new(&agent)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("can't start {}", agent.display()))?;
    println!("Started quietmouse in the background. Log: {}", paths.log().display());
    Ok(())
}

/// The agent's log, rolling the previous one over once it passes [`LOG_LIMIT`].
pub fn open_log() -> anyhow::Result<File> {
    let path = Paths::user()?.log();
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
        fs::rename(&path, path.with_file_name("quietmouse.old.log"))
            .with_context(|| format!("can't roll over {}", path.display()))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("can't open {}", path.display()))
}

/// The agent binary, installed next to this one.
fn agent_path() -> anyhow::Result<PathBuf> {
    let agent = std::env::current_exe()
        .context("can't find this executable")?
        .with_file_name(AGENT);
    ensure!(
        agent.exists(),
        "{AGENT} should be next to this executable, at {}",
        agent.display()
    );
    Ok(agent)
}

/// Adds or removes the agent in the per-user Run key, which needs no admin rights.
#[cfg(target_os = "windows")]
pub fn autostart(enable: bool) -> anyhow::Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_VALUE: &str = "quietmouse";
    let key = windows_registry::CURRENT_USER
        .create(RUN_KEY)
        .context("can't open the per-user Run key")?;
    if enable {
        let agent = agent_path()?;
        key.set_string(RUN_VALUE, format!("\"{}\"", agent.display()))
            .context("can't add quietmouse to the Run key")?;
        println!("quietmouse will start when you sign in ({})", agent.display());
    } else {
        if key.get_string(RUN_VALUE).is_ok() {
            key.remove_value(RUN_VALUE)
                .context("can't remove quietmouse from the Run key")?;
        }
        println!("quietmouse won't start when you sign in");
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub fn autostart(_enable: bool) -> anyhow::Result<()> {
    bail!("use packaging/macos/local.quietmouse.plist on macOS or packaging/linux/quietmouse.service on Linux")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("quietmouse-test-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_one_instance_holds_the_lock() {
        let lock = scratch("lock").join("quietmouse.lock");
        assert!(!held(&lock).unwrap());
        let instance = acquire(&lock).unwrap();
        assert!(held(&lock).unwrap());
        let second = acquire(&lock).err().unwrap();
        assert!(second.to_string().contains("already running"));
        drop(instance);
        assert!(!held(&lock).unwrap());
    }

    #[test]
    fn removing_a_missing_stop_request_is_quiet() {
        let request = scratch("stop").join("quietmouse.stop");
        File::create(&request).unwrap();
        remove_stop_file(&request);
        assert!(!request.exists());
        remove_stop_file(&request);
    }
}
