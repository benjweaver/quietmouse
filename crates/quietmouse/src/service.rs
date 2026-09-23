//! Running quietmouse in the background, per user and without admin rights:
//! one running instance at a time, a stop request any process can make, a log
//! file for the windowless agent, and starting at log-in on every platform.

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
// On macOS this is a symlink into quietmoused.app/Contents/MacOS/quietmoused, not the
// binary itself. It has to stay a plain sibling file, not a nested path: Homebrew's
// `brew link` only symlinks individual files out of a keg's bin into the shared bin
// it puts on PATH, silently skipping whole directories, so a bundle sitting directly
// in bin never reaches it. The symlink is what gets a real bundle in front of System
// Settings -> Privacy & Security, for a real icon under Accessibility and Input
// Monitoring instead of a generic one, while resolve() below still finds the exact
// binary inside it.
#[cfg(not(target_os = "windows"))]
const AGENT: &str = "quietmoused";

/// Past this size the agent's log is kept as `quietmouse.old.log` and a new one started.
const LOG_LIMIT: u64 = 1024 * 1024;
/// How long `quietmouse stop` waits. The daemon notices a stop request within one
/// device scan (half a second), then hands buttons back to the devices.
const STOP_WAIT: Duration = Duration::from_secs(10);
const STOP_POLL: Duration = Duration::from_millis(200);

/// Per-user files: `%LOCALAPPDATA%\quietmouse` on Windows,
/// `~/Library/Application Support/quietmouse` on macOS, `~/.local/share/quietmouse` on Linux.
/// [`Config::default_path`] puts the config here too, except on Linux, where XDG
/// keeps configuration in `~/.config` and only data lands here.
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

/// Another process is already the running instance.
#[derive(Debug)]
pub struct AlreadyRunning;

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("quietmouse is already running; stop it with `quietmouse stop`")
    }
}

impl std::error::Error for AlreadyRunning {}

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
        Err(TryLockError::WouldBlock) => Err(AlreadyRunning.into()),
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
    // The agent outlives this command; nothing here waits for it. On Windows it
    // still inherits every inheritable handle this process holds, including a
    // pipe its output may be going to, so a caller reading that pipe waits for
    // the agent to exit. `inherit_handles(false)` fixes that without `unsafe`
    // once it's stable (rust-lang/rust#146407).
    Command::new(&agent)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("can't start {}", agent.display()))?;
    println!(
        "Started quietmouse in the background. It writes {} only if something goes wrong.",
        paths.log().display()
    );
    Ok(())
}

/// The agent's log: the file it writes problems to, opened when it has one.
///
/// The agent records warnings and errors only, and a run with nothing to report
/// writes nothing, so the file exists only when something has gone wrong. Watch
/// a healthy run with `quietmouse run -v` instead.
pub struct Log {
    path: PathBuf,
    file: Option<File>,
}

impl Log {
    fn file(&mut self) -> io::Result<&mut File> {
        match &mut self.file {
            Some(file) => Ok(file),
            slot => {
                // Rolling over here, rather than when the agent starts, leaves
                // the last run's log alone until there's something to add.
                if fs::metadata(&self.path).is_ok_and(|meta| meta.len() > LOG_LIMIT) {
                    fs::rename(&self.path, self.path.with_file_name("quietmouse.old.log"))?;
                }
                Ok(slot.insert(OpenOptions::new().create(true).append(true).open(&self.path)?))
            }
        }
    }
}

impl io::Write for Log {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file()?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// Where the agent will write, if it needs to. Nothing is created yet.
pub fn open_log() -> anyhow::Result<Log> {
    Ok(Log {
        path: Paths::user()?.log(),
        file: None,
    })
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
    Ok(resolve(agent))
}

/// Resolves a package manager's shortcut (Homebrew's `bin/quietmoused`, which
/// points into a versioned folder) to the real path.
///
/// macOS and Linux want the resolved path. macOS ties its privacy permissions to
/// the exact binary, and a shortcut that stays put across upgrades leaves a stale
/// entry behind that silently matches nothing: no prompt, no permission, no clue
/// why. A versioned path makes each upgrade a new entry that macOS asks about
/// properly.
#[cfg(not(target_os = "windows"))]
fn resolve(agent: PathBuf) -> PathBuf {
    agent.canonicalize().unwrap_or(agent)
}

/// Windows wants the path exactly as it was reached.
///
/// Nothing here ties permissions to a binary, so there's nothing to gain, and
/// two things to lose. Resolving turns a package manager's stable shortcut into
/// whatever it points at today, and the startup shortcut would then point into
/// a folder that the next upgrade replaces: no error, just no quietmouse at
/// sign-in. It also returns an extended-length `\\?\C:\...` path, which not
/// everything that starts programs copes with.
#[cfg(target_os = "windows")]
fn resolve(agent: PathBuf) -> PathBuf {
    agent
}

/// Starts the agent at sign-in from a shortcut in the per-user Startup folder,
/// which needs no admin rights and is listed in Task Manager's Startup apps.
///
/// Older versions used an entry in the per-user Run key. On one Windows 11 PC
/// Explorer skipped every entry added there, Windows' own `cmd.exe` included,
/// while it started a shortcut in the Startup folder at the same sign-in; so
/// the Run entry is removed wherever it's found.
#[cfg(target_os = "windows")]
pub fn autostart(enable: bool) -> anyhow::Result<()> {
    let shortcut = startup_folder()?.join(SHORTCUT);
    let switched_off = startup_switched_off(STARTUP_FOLDER_LIST, SHORTCUT);
    remove_run_entry()?;
    if enable {
        let agent = agent_path()?;
        // Windows makes the folder, but it can go missing, and then nothing in
        // it runs until it's back.
        if let Some(folder) = shortcut.parent() {
            fs::create_dir_all(folder).with_context(|| format!("can't create {}", folder.display()))?;
        }
        mslnk::ShellLink::new(&agent)
            .and_then(|link| link.create_lnk(&shortcut))
            .with_context(|| format!("can't create {}", shortcut.display()))?;
        // Asking for it on means on, so a switch left off in Task Manager or
        // Settings is cleared; a shortcut with none recorded is started.
        if switched_off == Some(true) {
            clear_startup_switch(STARTUP_FOLDER_LIST, SHORTCUT)?;
            println!("quietmouse had been switched off in Task Manager or Settings; it's switched back on");
        }
        println!("quietmouse will start whenever you sign in ({})", agent.display());
        if !held(&Paths::user()?.lock())? {
            start()?;
        }
    } else {
        match fs::remove_file(&shortcut) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                return Err(error).with_context(|| format!("can't remove {}", shortcut.display()));
            }
            _ => {}
        }
        // Left behind, the switch would apply to a later install.
        if switched_off.is_some() {
            clear_startup_switch(STARTUP_FOLDER_LIST, SHORTCUT)?;
        }
        println!("quietmouse won't start when you sign in");
    }
    Ok(())
}

/// The shortcut's name, which is also what its startup switch is recorded under.
#[cfg(target_os = "windows")]
const SHORTCUT: &str = "quietmouse.lnk";

/// The per-user Startup folder, where Explorer keeps it: this follows any
/// redirection, with the usual place as the fallback.
#[cfg(target_os = "windows")]
fn startup_folder() -> anyhow::Result<PathBuf> {
    windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders")
        .and_then(|key| key.get_string("Startup"))
        .ok()
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|dir| dir.join(r"Microsoft\Windows\Start Menu\Programs\Startup")))
        .context("can't find the Startup folder")
}

/// Removes the Run entry older versions started from, and its startup switch.
#[cfg(target_os = "windows")]
fn remove_run_entry() -> anyhow::Result<()> {
    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const RUN_VALUE: &str = "quietmouse";
    let key = windows_registry::CURRENT_USER
        .create(RUN_KEY)
        .context("can't open the per-user Run key")?;
    if key.get_string(RUN_VALUE).is_ok() {
        key.remove_value(RUN_VALUE)
            .context("can't remove the old entry in the Run key")?;
    }
    if startup_switched_off(RUN_LIST, RUN_VALUE).is_some() {
        clear_startup_switch(RUN_LIST, RUN_VALUE)?;
    }
    Ok(())
}

/// Where Task Manager's Startup apps and Settings → Apps → Startup record the
/// on/off switch for each startup item, one list per kind. Switching an item
/// off there leaves the item alone and marks it here instead.
#[cfg(target_os = "windows")]
const STARTUP_APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
#[cfg(target_os = "windows")]
const STARTUP_FOLDER_LIST: &str = "StartupFolder";
#[cfg(target_os = "windows")]
const RUN_LIST: &str = "Run";

/// Whether the item `name` in `list` has been switched off, or `None` if Windows
/// holds no switch for it. The first byte of the record is odd when it's off.
#[cfg(target_os = "windows")]
fn startup_switched_off(list: &str, name: &str) -> Option<bool> {
    let record = windows_registry::CURRENT_USER
        .open(format!(r"{STARTUP_APPROVED}\{list}"))
        .and_then(|key| key.get_bytes(name))
        .ok()?;
    Some(record.first().is_some_and(|flags| flags & 1 == 1))
}

/// Removes the switch for the item `name` in `list`.
#[cfg(target_os = "windows")]
fn clear_startup_switch(list: &str, name: &str) -> anyhow::Result<()> {
    windows_registry::CURRENT_USER
        .create(format!(r"{STARTUP_APPROVED}\{list}"))
        .and_then(|key| key.remove_value(name))
        .context("can't reset quietmouse's startup switch")
}

/// Label of the per-user LaunchAgent.
#[cfg(target_os = "macos")]
const LAUNCH_AGENT: &str = "local.quietmouse";

/// Installs or removes a LaunchAgent in `~/Library/LaunchAgents`, which needs no
/// admin rights, and loads it into this user's session straight away.
#[cfg(target_os = "macos")]
pub fn autostart(enable: bool) -> anyhow::Result<()> {
    let home = dirs::home_dir().context("can't find your home folder")?;
    let plist = home.join("Library/LaunchAgents").join(format!("{LAUNCH_AGENT}.plist"));
    let domain = format!("gui/{}", current_uid()?);
    let service = format!("{domain}/{LAUNCH_AGENT}");
    // Unload any earlier copy first; this fails harmlessly when none is loaded.
    if let Err(error) = run_tool("launchctl", &["bootout", &service]) {
        log::debug!("{error:#}");
    }
    // bootout returns as soon as it has asked the agent to stop, while the agent
    // is still handing buttons back to the mouse. launchd keeps it listed until
    // its process has exited, and loading the new agent before then fails with
    // "Bootstrap failed: 5"; worse, a new agent that did start would find the
    // old one still holding the instance lock and quietly exit.
    let deadline = Instant::now() + STOP_WAIT;
    while loaded(&service)? {
        ensure!(
            Instant::now() < deadline,
            "the old quietmouse was still running {} seconds after being asked to stop",
            STOP_WAIT.as_secs()
        );
        thread::sleep(STOP_POLL);
    }
    if enable {
        let agent = agent_path()?;
        if let Some(dir) = plist.parent() {
            fs::create_dir_all(dir).with_context(|| format!("can't create {}", dir.display()))?;
        }
        launch_agent(&agent)
            .to_file_xml(&plist)
            .with_context(|| format!("can't write {}", plist.display()))?;
        // Straight after an agent goes, launchd can still refuse the new one for a
        // moment with the same error, so that's retried too.
        let plist = plist.to_string_lossy();
        while let Err(error) = run_tool("launchctl", &["bootstrap", &domain, &plist]) {
            if Instant::now() >= deadline {
                return Err(error);
            }
            log::debug!("{error:#}");
            thread::sleep(STOP_POLL);
        }
        println!(
            "quietmouse is starting now and will start whenever you log in ({})",
            agent.display()
        );
    } else {
        if plist.exists() {
            fs::remove_file(&plist).with_context(|| format!("can't remove {}", plist.display()))?;
        }
        println!("quietmouse won't start when you log in");
    }
    Ok(())
}

/// Whether launchd still has `service` loaded.
#[cfg(target_os = "macos")]
fn loaded(service: &str) -> anyhow::Result<bool> {
    let status = Command::new("launchctl")
        .args(["print", service])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("can't run launchctl")?;
    Ok(status.success())
}

/// The LaunchAgent: start `agent` at log-in, and again if it crashes.
#[cfg(target_os = "macos")]
fn launch_agent(agent: &Path) -> plist::Value {
    let mut keep_alive = plist::Dictionary::new();
    keep_alive.insert("SuccessfulExit".to_owned(), plist::Value::Boolean(false));
    let mut launch_agent = plist::Dictionary::new();
    launch_agent.insert("Label".to_owned(), plist::Value::String(LAUNCH_AGENT.to_owned()));
    launch_agent.insert(
        "ProgramArguments".to_owned(),
        plist::Value::Array(vec![plist::Value::String(agent.to_string_lossy().into_owned())]),
    );
    launch_agent.insert("RunAtLoad".to_owned(), plist::Value::Boolean(true));
    launch_agent.insert("KeepAlive".to_owned(), plist::Value::Dictionary(keep_alive));
    launch_agent.insert("ProcessType".to_owned(), plist::Value::String("Interactive".to_owned()));
    plist::Value::Dictionary(launch_agent)
}

#[cfg(target_os = "macos")]
fn current_uid() -> anyhow::Result<String> {
    let output = Command::new("id").arg("-u").output().context("can't run `id -u`")?;
    ensure!(output.status.success(), "`id -u` failed");
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Name of the systemd user service.
#[cfg(target_os = "linux")]
const SYSTEMD_UNIT: &str = "quietmouse.service";

/// Installs or removes a systemd user service in `~/.config/systemd/user`, which
/// needs no root, and starts or stops it straight away.
#[cfg(target_os = "linux")]
pub fn autostart(enable: bool) -> anyhow::Result<()> {
    let dir = dirs::config_dir()
        .context("this system has no config directory")?
        .join("systemd/user");
    let unit = dir.join(SYSTEMD_UNIT);
    if enable {
        let agent = agent_path()?;
        fs::create_dir_all(&dir).with_context(|| format!("can't create {}", dir.display()))?;
        fs::write(&unit, systemd_unit(&agent)).with_context(|| format!("can't write {}", unit.display()))?;
        run_tool("systemctl", &["--user", "daemon-reload"])?;
        run_tool("systemctl", &["--user", "enable", "--now", SYSTEMD_UNIT])?;
        println!(
            "quietmouse is starting now and will start whenever you log in ({})",
            agent.display()
        );
    } else {
        if unit.exists() {
            run_tool("systemctl", &["--user", "disable", "--now", SYSTEMD_UNIT])?;
            fs::remove_file(&unit).with_context(|| format!("can't remove {}", unit.display()))?;
            run_tool("systemctl", &["--user", "daemon-reload"])?;
        }
        println!("quietmouse won't start when you log in");
    }
    Ok(())
}

/// The systemd user unit: run `agent` for the graphical session, restarting it if it crashes.
#[cfg(any(target_os = "linux", test))]
fn systemd_unit(agent: &Path) -> String {
    // In unit files `%` starts a specifier, and quoted values use C-style escapes.
    let quoted = agent
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!(
        "[Unit]\n\
         Description=quietmouse: Logitech mouse settings, buttons and gestures\n\
         After=graphical-session.target\n\
         PartOf=graphical-session.target\n\
         \n\
         [Service]\n\
         ExecStart=\"{quoted}\"\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy=graphical-session.target\n"
    )
}

/// Runs a system tool, turning a failure into an error carrying its output.
#[cfg(not(target_os = "windows"))]
fn run_tool(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("can't run {program}"))?;
    ensure!(
        output.status.success(),
        "`{program} {}` failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
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
    fn second_instance_is_recognisable() {
        let lock = scratch("recognise").join("quietmouse.lock");
        let _instance = acquire(&lock).unwrap();
        assert!(acquire(&lock).err().unwrap().is::<AlreadyRunning>());
    }

    #[test]
    fn systemd_unit_quotes_the_agent_path() {
        let unit = systemd_unit(Path::new("/home/ben/100% \"mice\"/quietmoused"));
        assert!(unit.contains(r#"ExecStart="/home/ben/100%% \"mice\"/quietmoused""#));
        assert!(unit.contains("WantedBy=graphical-session.target"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn launch_agent_runs_the_agent_at_login() {
        let mut xml = Vec::new();
        launch_agent(Path::new("/Users/ben/.local/bin/quietmoused"))
            .to_writer_xml(&mut xml)
            .unwrap();
        let parsed = plist::Value::from_reader_xml(xml.as_slice()).unwrap();
        let agent = parsed.as_dictionary().unwrap();
        assert_eq!(agent.get("Label").and_then(plist::Value::as_string), Some(LAUNCH_AGENT));
        assert_eq!(agent.get("RunAtLoad").and_then(plist::Value::as_boolean), Some(true));
        let program = agent.get("ProgramArguments").and_then(plist::Value::as_array).unwrap();
        assert_eq!(program[0].as_string(), Some("/Users/ben/.local/bin/quietmoused"));
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
