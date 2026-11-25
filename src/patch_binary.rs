use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

/// All writers (window, relay and standalone watchdog) share one transaction.
/// A second watchdog must re-read the winner's patched file, not kill the LS
/// to commit an obsolete copy it read before the first writer finished.
#[cfg(windows)]
struct PatchLock(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl PatchLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        use sha2::{Digest, Sha256};
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        let absolute = fs::canonicalize(path).map_err(|e| e.to_string())?;
        let id = hex::encode(Sha256::digest(
            absolute.to_string_lossy().to_lowercase().as_bytes(),
        ));
        let name: Vec<_> = format!("Global\\AGUnlocker.Patch.{id}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let waited = unsafe { WaitForSingleObject(handle, 15_000) };
        if waited == 0 || waited == 0x80 {
            return Ok(Self(handle));
        }
        let error = if waited == 258 {
            "another patch writer is still busy".into()
        } else {
            std::io::Error::last_os_error().to_string()
        };
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        Err(error)
    }
}

#[cfg(windows)]
impl Drop for PatchLock {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.0);
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

// The client patch preserves the file layout. Older versions also renamed
// the proxy environment variable. That rename is only used during rollback.
//
// The literals live inside obfstr! blocks so they don't show up as plain
// strings in this binary.

fn replace_all(data: &mut [u8], from: &[u8], to: &[u8]) -> usize {
    debug_assert_eq!(from.len(), to.len());
    if data.len() < from.len() {
        return 0;
    }
    let mut count = 0;
    let mut i = 0;
    while i + from.len() <= data.len() {
        if &data[i..i + from.len()] == from {
            data[i..i + to.len()].copy_from_slice(to);
            count += 1;
            i += from.len();
        } else {
            i += 1;
        }
    }
    count
}

fn count_occurrences(data: &[u8], needle: &[u8]) -> usize {
    if data.len() < needle.len() {
        return 0;
    }
    (0..=data.len() - needle.len())
        .filter(|&i| &data[i..i + needle.len()] == needle)
        .count()
}

/// Writes the binary back atomically: a sibling temp on the same directory, then
/// a rename over the target. A crash or power loss mid-write leaves either the
/// old binary or the new one, never a truncated 135 MB executable that would fail
/// to launch. A running executable is locked on Windows, so the owning process is
/// killed and the write retried only if the first attempt actually fails.
fn write_binary(bin_path: &Path, data: &[u8]) -> Result<(), String> {
    let res = match write_atomic(bin_path, data) {
        Ok(()) => Ok(()),
        Err(e) if matches!(e.raw_os_error(), Some(32 | 33)) => {
            kill_holder(bin_path);
            thread::sleep(Duration::from_millis(500));
            write_atomic(bin_path, data).map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    };
    // macOS: a bundle changed under a quarantine flag is "damaged" to
    // Gatekeeper on its first launch, so the flag goes with the patch; a
    // refused write into a bundle says which switch macOS wants.
    #[cfg(target_os = "macos")]
    let res = match res {
        Ok(()) => {
            if let Some(app) = crate::macos::bundle_of(bin_path) {
                crate::macos::strip_quarantine(&app);
            }
            Ok(())
        }
        Err(e) => Err(crate::macos::app_management_hint(bin_path, &e)),
    };
    res
}

/// Kills whatever process holds `bin_path` open so the write can be retried.
/// Windows locks a running image; Linux does not (a rename-over succeeds while
/// the old inode keeps running), so this is a belt-and-braces retry helper there.
#[cfg(target_os = "windows")]
pub fn kill_holder(bin_path: &Path) {
    let file_name = bin_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut cmd = Command::new("taskkill");
    cmd.args(["/F", "/IM", &file_name])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::utils::no_window(&mut cmd).output().ok();
}

#[cfg(not(target_os = "windows"))]
pub fn kill_holder(bin_path: &Path) {
    let name = bin_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if !name.is_empty() {
        Command::new("pkill")
            .args(["-f", &name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok();
    }
}

/// The Antigravity app that owns a language server: the first directory above
/// `target` (at most six levels) holding the Desktop or IDE executable. `None`
/// for a CLI `agy` and anything else with no app around it.
///
/// Killing the server alone does not release the file for long: the app starts
/// it again within two seconds, the new process takes the lock, and after three
/// such restarts it stops trying and is left open with no server at all (seen
/// 2026-11-11 on the 2.31.1 update, G92). Closing the app is what makes the
/// release stick.
pub fn owner_app(target: &Path) -> Option<PathBuf> {
    const APPS: [&str; 2] = ["Antigravity.exe", "Antigravity IDE.exe"];
    target
        .ancestors()
        .skip(1)
        .take(6)
        .find_map(|dir| APPS.iter().map(|n| dir.join(n)).find(|p| p.is_file()))
}

/// Closes the app and everything it started. Windows only; elsewhere the
/// server kill is all there is.
#[cfg(target_os = "windows")]
pub fn quit_app(app: &Path) {
    let name = app
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut cmd = Command::new("taskkill");
    cmd.args(["/F", "/T", "/IM", &name])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::utils::no_window(&mut cmd).output().ok();
}

#[cfg(not(target_os = "windows"))]
pub fn quit_app(_app: &Path) {}

/// True when this process runs in Windows session 0 - the non-interactive one
/// the S4U tasks (watchdog, relay) live in. Anything started from there has no
/// desktop to draw on.
#[cfg(target_os = "windows")]
fn in_service_session() -> bool {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    let mut sid: u32 = u32::MAX;
    // SAFETY: plain out-parameter call with a valid pointer.
    let ok = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut sid) };
    ok != 0 && sid == 0
}

/// Starts the app in the user's own session through a one-shot Interactive
/// scheduled task (no password, no SeTcb privilege needed; the user must be
/// signed in, which they are when there is something to reopen). Succeeds only
/// when the app's process is seen in a session other than 0.
#[cfg(target_os = "windows")]
fn launch_in_user_session(app: &Path) -> std::io::Result<()> {
    let q = |p: &Path| p.to_string_lossy().replace('\'', "''");
    let stem = app
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace('\'', "''");
    let script = format!(
        "$ErrorActionPreference='Stop'; $n='AG Unlocker Reopen'; \
         $a=New-ScheduledTaskAction -Execute '{exe}' -WorkingDirectory '{dir}'; \
         $p=New-ScheduledTaskPrincipal -UserId \"$env:USERDOMAIN\\$env:USERNAME\" \
              -LogonType Interactive -RunLevel Limited; \
         $s=New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries \
              -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew \
              -ExecutionTimeLimit ([TimeSpan]::Zero); \
         Register-ScheduledTask -TaskName $n -Action $a -Principal $p -Settings $s \
              -Description 'Antigravity Unlocker: reopen the app in the user session' \
              -Force | Out-Null; \
         Start-ScheduledTask -TaskName $n; \
         for ($i=0; $i -lt 20; $i++) {{ \
           if (Get-Process -Name '{stem}' -ErrorAction SilentlyContinue \
                 | Where-Object {{ $_.SessionId -ne 0 }}) {{ exit 0 }}; \
           Start-Sleep -Milliseconds 500 }}; \
         [Console]::Error.WriteLine('process not seen in a user session'); exit 1",
        exe = q(app),
        dir = q(app.parent().unwrap_or(Path::new("."))),
        stem = stem,
    );
    let out = crate::utils::powershell_within(&script, std::time::Duration::from_secs(45))
        .ok_or_else(|| std::io::Error::other("powershell did not answer"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// Starts the app again, detached from whoever asks (the watchdog's task).
///
/// From session 0 (the watchdog is an S4U task) a plain spawn starts the app on
/// no desktop: no window, yet it holds Electron's single-instance lock, so every
/// later launch by the user is silently forwarded to it and nothing appears
/// (G96, seen after the 2.32.1 update). There the app is started through the
/// user's own session instead, and never by a plain spawn.
#[cfg(target_os = "windows")]
pub fn launch_app(app: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    // Keep the client at user integrity even when the patch window is elevated.
    // Otherwise its orphaned sidecars cannot be reaped by the Limited agent.
    if in_service_session() || crate::utils::is_admin() {
        return launch_in_user_session(app);
    }
    Command::new(app)
        .current_dir(app.parent().unwrap_or(Path::new(".")))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
}

#[cfg(not(target_os = "windows"))]
pub fn launch_app(_app: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Where a patched copy of `target` is written before it is renamed over it.
pub(crate) fn temp_path(target: &Path) -> PathBuf {
    let mut tmp = target.as_os_str().to_os_string();
    tmp.push(format!(".{}{}", std::process::id(), TEMP_SUFFIX));
    PathBuf::from(tmp)
}

/// The tail every patch temp carries, so a directory scan can tell one from the
/// file it is a copy of.
pub(crate) const TEMP_SUFFIX: &str = ".agtmp";

/// Removes the patch temps a writer killed mid-write left beside `target` -
/// `<target>.<pid>.agtmp`, and the single `<target>.agtmp` builds before D27
/// used. A shared name was overwritten by the next write; one per process is
/// not, and the relay is `taskkill`ed on every upgrade, possibly mid-patch.
///
/// Only temps older than any write takes: a younger one may be another
/// patcher's, being written this second.
pub(crate) fn sweep_stale_temps(target: &Path) {
    const STALE: Duration = Duration::from_secs(10 * 60);
    let (Some(dir), Some(name)) = (target.parent(), target.file_name().and_then(|n| n.to_str()))
    else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{}.", name);
    for entry in entries.flatten() {
        let file = entry.file_name();
        let Some(file) = file.to_str() else { continue };
        if !(file.starts_with(&prefix) && file.ends_with(TEMP_SUFFIX)) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Temp-on-same-dir + rename. The temp sits beside the target so the rename is a
/// same-volume move (atomic), not a cross-volume copy.
///
/// The temp is named per process. Up to three patchers can meet on one file -
/// the relay's watchdog thread, the standalone watchdog task and the window's
/// auto-patch (D27) - and with one shared name a second writer truncating the
/// temp while the first renamed it over the target could leave a torn 150 MB
/// binary in place. Separate temps make every rename a whole file.
fn write_atomic(bin_path: &Path, data: &[u8]) -> std::io::Result<()> {
    sweep_stale_temps(bin_path);
    let tmp = temp_path(bin_path);
    if let Err(e) = fs::write(&tmp, data) {
        // A full disk leaves a partial 150 MB copy; nothing else would remove it.
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // On Unix a fresh temp file is created 0644, and renaming it over the language
    // server would strip the execute bit - a non-executable binary the app then
    // cannot launch. Copy the original's mode onto the temp before the rename so
    // the patched file stays exactly as runnable as the one it replaces. No-op on
    // Windows, where executability is not a file-mode bit.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(bin_path)
            .map(|m| m.permissions().mode())
            .unwrap_or(0o755);
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(mode));
    }
    // macOS: the edited Mach-O no longer matches its signature and Apple
    // Silicon would kill it at exec. The temp is re-signed before the rename,
    // so the name never points at a binary the kernel refuses (S80). The
    // rename also gives it a fresh vnode - an in-place write would keep the
    // kernel's cached signature of the old one.
    #[cfg(target_os = "macos")]
    if crate::macos::is_macho(data) {
        if let Err(e) = crate::macos::resign(&tmp) {
            let _ = fs::remove_file(&tmp);
            return Err(std::io::Error::other(e));
        }
        crate::macos::strip_quarantine(&tmp);
    }
    match fs::rename(&tmp, bin_path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// The field name rewrites stored in obfuscated literals.
pub(crate) fn field_signatures() -> ([u8; 10], [u8; 10]) {
    obfstr::obfstr! {
        let from = "ineligible";
        let to = "inexigible";
    }
    let mut f = [0u8; 10];
    let mut t = [0u8; 10];
    f.copy_from_slice(from.as_bytes());
    t.copy_from_slice(to.as_bytes());
    (f, t)
}

/// The original proxy variable name stored in an obfuscated literal.
pub(crate) fn proxy_var_original() -> [u8; 11] {
    obfstr::obfstr! {
        let proxy_from = "https_proxy";
    }
    let mut p = [0u8; 11];
    p.copy_from_slice(proxy_from.as_bytes());
    p
}

fn rewrite(bin_path: &Path, from: &[u8], to: &[u8]) -> Result<usize, String> {
    #[cfg(windows)]
    let _transaction = PatchLock::acquire(bin_path)?;
    let mut data = fs::read(bin_path).map_err(|e| e.to_string())?;
    let replaced = replace_all(&mut data, from, to);
    if replaced == 0 {
        // Nothing to do - report whether the target state is already in place.
        return if count_occurrences(&data, to) > 0 {
            Ok(0)
        } else {
            Err("Сигнатура не найдена".to_string())
        };
    }
    write_binary(bin_path, &data)?;
    Ok(replaced)
}

/// What each patch target of an install is, for the line that says a build is
/// not known: its kind (ELF / script / PE / other), size, and where a symlink
/// really points. A report of "не знакома" without this cannot tell a changed
/// binary from a launcher stub or a symlink to the real file elsewhere.
pub fn describe_targets(inst: &Path) -> String {
    let mut parts = Vec::new();
    for bin in binary_targets(inst) {
        let Ok(meta) = fs::symlink_metadata(&bin) else {
            continue;
        };
        let mut head = [0u8; 4];
        let n = fs::File::open(&bin)
            .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
            .unwrap_or(0);
        let kind = match &head[..n] {
            [0x7f, b'E', b'L', b'F'] => "ELF",
            [b'#', b'!', ..] => "скрипт",
            [b'M', b'Z', ..] => "PE",
            [0xcf | 0xca, 0xfa | 0xfe, ..] => "Mach-O",
            _ => "другое",
        };
        let name = bin.file_name().unwrap_or_default().to_string_lossy();
        let link = if meta.file_type().is_symlink() {
            match fs::canonicalize(&bin) {
                Ok(t) => format!(
                    ", ссылка → {}",
                    crate::utils::mask_path(&t.display().to_string())
                ),
                Err(_) => ", битая ссылка".to_string(),
            }
        } else {
            String::new()
        };
        let size = fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
        parts.push(format!("{name}: {kind}, {} КБ{link}", size / 1024));
    }
    parts.join("; ")
}

pub fn patch_binary(_inst: &Path, bin_path: &Path) -> Result<usize, String> {
    let (old_bytes, new_bytes) = field_signatures();
    rewrite(bin_path, &old_bytes, &new_bytes)
}

pub fn unpatch_binary(bin_path: &Path) -> Result<usize, String> {
    let (old_bytes, new_bytes) = field_signatures();
    rewrite(bin_path, &new_bytes, &old_bytes)
}

/// Private environment name written by older releases, retained for cleanup.
pub const PROXY_VAR_NEW: &str = "AG_LS_PROXY";

/// Puts the standard name back.
pub fn unpatch_proxy_var(bin_path: &Path) -> Result<usize, String> {
    let old_bytes = proxy_var_original();
    rewrite(bin_path, PROXY_VAR_NEW.as_bytes(), &old_bytes)
}

/// What re-patching one binary found. The background watchdog needs to tell
/// these outcomes apart where menu 1 only needs success/failure:
///
/// - `SignatureMissing` is deliberately distinct from `Failed`. A signature
///   that is simply gone means a new or broken build this patcher does not
///   understand, and the right thing is to **leave it alone** so Antigravity
///   launches and shows its own error - that is the user's cue to fetch a newer
///   patcher. A `Failed` is transient (the file was locked mid-update) and is
///   worth retrying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepatchOutcome {
    /// Already patched; nothing was done.
    AlreadyPatched,
    /// Was reverted (typically by an app auto-update) and is now patched again.
    Repatched(usize),
    /// The signature is absent - do not touch it, let the app show its error.
    SignatureMissing,
    /// A transient failure, usually a file locked mid-update. Retry later.
    Failed(String),
    /// An additive edit is locked; retry without ever killing a working LS.
    Deferred(String),
}

/// Re-applies the rename to a binary that may have been reverted by an update.
///
/// Unlike `patch_binary`, this never conflates "nothing to patch because the
/// signature is gone" with "already patched": that distinction is the whole
/// point of the watchdog. `write_binary` still kills a process holding the file
/// open and retries, so a running-but-unpatched Language Server is replaced in
/// the same step - which is what enforces "no unpatched server keeps running"
/// without ever touching the editor shell.
pub fn repatch_if_needed(bin_path: &Path) -> RepatchOutcome {
    repatch_with(bin_path, true)
}

#[cfg(windows)]
pub(crate) fn repatch_before_launch(bin_path: &Path) -> RepatchOutcome {
    repatch_with(bin_path, false)
}

fn repatch_with(bin_path: &Path, kill_locked: bool) -> RepatchOutcome {
    #[cfg(windows)]
    if crate::watchdog::updater_busy(&crate::watchdog::updater_dirs()) {
        return RepatchOutcome::Deferred("Antigravity update is still running".into());
    }
    #[cfg(windows)]
    let _transaction = match PatchLock::acquire(bin_path) {
        Ok(lock) => lock,
        Err(e) => return RepatchOutcome::Failed(e),
    };
    let (from, to) = field_signatures();
    let mut data = match fs::read(bin_path) {
        Ok(d) => d,
        Err(e) => return RepatchOutcome::Failed(e.to_string()),
    };
    let replaced = replace_all(&mut data, &from, &to);
    if replaced == 0 {
        if count_occurrences(&data, &to) == 0 {
            return RepatchOutcome::SignatureMissing;
        }
        return RepatchOutcome::AlreadyPatched;
    }
    let written = if kill_locked {
        write_binary(bin_path, &data)
    } else {
        write_atomic(bin_path, &data).map_err(|e| e.to_string())
    };
    match written {
        Ok(()) => RepatchOutcome::Repatched(replaced),
        Err(e) => RepatchOutcome::Failed(e),
    }
}

/// Native binaries that carry the eligibility check, for a given install root.
///
/// The list is cross-platform on purpose: a Windows install never has the Linux
/// names and vice versa, and everything is filtered by `exists()`, so one list
/// serves both. On Linux the language-server filename carries a
/// `_linux_x64`-style platform suffix that can drift between builds, so instead
/// of hardcoding it the two `bin` directories are globbed for any
/// `language_server*` file - whatever the exact suffix, the signature scan then
/// decides whether it is really a target.
pub fn binary_targets(inst: &Path) -> Vec<PathBuf> {
    // A macOS bundle spells it `Contents/Resources`.
    #[cfg(target_os = "macos")]
    let resources = crate::macos::resources_dir(inst);
    #[cfg(not(target_os = "macos"))]
    let resources = inst.join("resources");
    let resources_bin = resources.join("bin");
    let ext_bin = resources
        .join("app")
        .join("extensions")
        .join("antigravity")
        .join("bin");

    let mut targets: Vec<PathBuf> = vec![
        // CLI: `agy.exe` on Windows, bare `agy` on Linux/macOS.
        inst.join("agy.exe"),
        inst.join("agy"),
        // Desktop's own language server.
        resources_bin.join("language_server.exe"),
        resources_bin.join("language_server"),
        // IDE's bundled language server, Windows-named.
        ext_bin.join("language_server_windows_x64.exe"),
        ext_bin.join("language_server.exe"),
    ];

    // Any other `language_server*` in the two bin dirs - catches the Linux/macOS
    // platform-suffixed names (`language_server_linux_x64`, `..._darwin_arm64`, …)
    // without pinning the exact spelling.
    for dir in [&ext_bin, &resources_bin] {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                // Never a patch temp: `language_server_linux_x64.<pid>.agtmp`
                // matches the prefix for the second or so it exists.
                let is_ls = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("language_server") && !n.ends_with(TEMP_SUFFIX));
                if is_ls && path.is_file() && !targets.contains(&path) {
                    targets.push(path);
                }
            }
        }
    }

    // IDE 2.6.0 carries no language server of its own: its `agy` extension
    // downloads the backend into the user's `~/.gemini/bin`. That file is the
    // IDE's, so the IDE's row reads and patches it (G85).
    if resources
        .join("app")
        .join("extensions")
        .join("agy")
        .is_dir()
    {
        let home = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from);
        if let Some(bin) = home.map(|h| h.join(".gemini").join("bin")) {
            for name in ["agy.exe", "agy"] {
                let p = bin.join(name);
                if !targets.contains(&p) {
                    targets.push(p);
                }
            }
        }
    }

    targets.retain(|p| p.exists());

    // On Unix a CLI like `agy` is frequently a symlink in a PATH dir pointing at
    // the real binary; resolve it so the patch edits the actual file, and dedup by
    // the resolved path so the same file reached two ways is not patched twice.
    #[cfg(unix)]
    {
        let mut seen = std::collections::HashSet::new();
        targets = targets
            .into_iter()
            .map(|p| fs::canonicalize(&p).unwrap_or(p))
            .filter(|p| seen.insert(p.clone()))
            .collect();
    }

    targets.dedup();
    targets
}

/// Closes Antigravity before a patch run.
///
/// Silent: the only caller left is the window's worker thread, which has no
/// console to print to and reports the step in the log pane itself. The one
/// second afterwards is functional, not cosmetic — `taskkill` returns before the
/// image handle is actually released.
pub fn kill_affected_processes() {
    kill_platform_processes();
    thread::sleep(Duration::from_millis(1000));
}

/// Stops the language server / CLI so their files can be replaced. Never the
/// editor shell itself - that would lose the user's unsaved work (D9).
#[cfg(target_os = "windows")]
fn kill_platform_processes() {
    let processes = [
        "Antigravity.exe",
        "Antigravity CLI.exe",
        "Antigravity IDE.exe",
        "agy.exe",
        "language_server.exe",
        "language_server_windows_x64.exe",
    ];
    for p in processes.iter() {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/F", "/IM", p])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::utils::no_window(&mut cmd).output().ok();
    }
}

/// The Linux/macOS side. The language server is the process holding the file
/// open; it is matched by the binary's basename via `pkill -f` so whatever the
/// platform suffix is (`language_server_linux_x64`), it is caught. The `agy` CLI
/// and the two language-server basenames are killed; the editor shell is left
/// running, same rule as Windows.
#[cfg(not(target_os = "windows"))]
fn kill_platform_processes() {
    // -f matches against the whole command line, so a full install path still
    // matches; the patterns are the binary basenames the patcher targets.
    let patterns = ["language_server", "/agy"];
    for pat in patterns.iter() {
        Command::new("pkill")
            .args(["-f", pat])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok();
    }
}

/// Outcome of patching the native binaries of one install.
pub struct BinarySummary {
    /// Binaries that are now in the patched state (freshly patched or already so).
    pub ok: usize,
    /// Binaries where the signature could not be found or written.
    pub failed: usize,
    /// The last failure, so the caller can name it without this module printing.
    pub last_error: Option<String>,
}

impl BinarySummary {
    pub fn total(&self) -> usize {
        self.ok + self.failed
    }
}

pub fn patch_all_binaries(inst: &Path) -> BinarySummary {
    let mut summary = BinarySummary {
        ok: 0,
        failed: 0,
        last_error: None,
    };
    for bin in binary_targets(inst) {
        let label = bin
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        // Deliberately silent. The caller prints one progress line per install
        // and then its result on the same row; a line from here lands in the
        // middle of it and pushes the result onto its own. Failures are not
        // lost - they are counted here and named by `binary_failure_message`.
        match patch_binary(inst, &bin) {
            Ok(_) => summary.ok += 1,
            Err(e) => {
                summary.failed += 1;
                summary.last_error = Some(format!("{}: {}", label, e));
            }
        }
    }
    summary
}

/// What a read-only look at one install found. Nothing here writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileState {
    /// The patched name is present - this file is done.
    Patched,
    /// The original name is present and the patched one is not.
    Unpatched,
    /// Neither name is there: a build we do not know how to patch.
    SignatureMissing,
    /// Could not be read at all (missing file, permissions).
    Unreadable(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallState {
    pub files: Vec<(std::path::PathBuf, FileState)>,
}

#[allow(dead_code)]
impl InstallState {
    /// True only when every native binary we know about is patched.
    pub fn fully_patched(&self) -> bool {
        !self.files.is_empty()
            && self
                .files
                .iter()
                .all(|(_, state)| *state == FileState::Patched)
    }

    /// True when some are patched and some are not - what an interrupted run
    /// or a half-finished auto-update leaves behind.
    pub fn partially_patched(&self) -> bool {
        let has_patched = self
            .files
            .iter()
            .any(|(_, state)| *state == FileState::Patched);
        let has_not_patched = self
            .files
            .iter()
            .any(|(_, state)| *state != FileState::Patched);
        has_patched && has_not_patched
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

const INSPECT_CHUNK_SIZE: usize = 1024 * 1024; // 1 MiB stream buffer

fn contains_subslice(data: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if data.len() < needle.len() {
        return false;
    }
    // First-byte skip rather than `windows(k).any(..)`. The Language Server is
    // ~150 MB and this runs over the whole of it twice per install, so the naive
    // form costs a comparison per byte per needle; anchoring on the first byte
    // turns almost all of them into one.
    let first = needle[0];
    let last = data.len() - needle.len();
    let mut i = 0;
    while i <= last {
        match data[i..=last].iter().position(|b| *b == first) {
            Some(off) => {
                let at = i + off;
                if &data[at..at + needle.len()] == needle {
                    return true;
                }
                i = at + 1;
            }
            None => return false,
        }
    }
    false
}

/// Inspects one binary file on disk in a streaming manner using a 1 MiB buffer
/// with overlap to prevent missing matches at buffer boundaries.
#[allow(dead_code)]
pub fn inspect_file(bin_path: &Path) -> FileState {
    inspect_file_stream(bin_path, INSPECT_CHUNK_SIZE)
}

fn inspect_file_stream(bin_path: &Path, chunk_size: usize) -> FileState {
    let (unpatched, patched) = field_signatures();
    let max_needle = unpatched.len().max(patched.len());
    let overlap_len = max_needle.saturating_sub(1);

    let mut file = match fs::File::open(bin_path) {
        Ok(f) => f,
        Err(e) => return FileState::Unreadable(e.to_string()),
    };

    use std::io::Read;
    let mut buf = vec![0u8; chunk_size.max(max_needle)];
    let mut overlap = 0;
    let mut found_unpatched = false;

    loop {
        let mut bytes_read = 0;
        while overlap + bytes_read < buf.len() {
            match file.read(&mut buf[overlap + bytes_read..]) {
                Ok(0) => break,
                Ok(n) => bytes_read += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return FileState::Unreadable(e.to_string()),
            }
        }

        if bytes_read == 0 {
            break;
        }

        let valid_len = overlap + bytes_read;
        let slice = &buf[..valid_len];
        if contains_subslice(slice, &patched) {
            return FileState::Patched;
        }
        if !found_unpatched && contains_subslice(slice, &unpatched) {
            found_unpatched = true;
        }

        if overlap + bytes_read < buf.len() {
            break;
        }

        overlap = overlap_len.min(valid_len);
        buf.copy_within(valid_len - overlap..valid_len, 0);
    }

    if found_unpatched {
        FileState::Unpatched
    } else {
        FileState::SignatureMissing
    }
}

/// Reads every native binary of one install and reports whether each carries the
/// patched name. Read-only: opens files for reading, writes nothing, spawns no
/// child process, kills no process.
#[allow(dead_code)]
pub fn inspect_install(install: &std::path::Path) -> InstallState {
    let targets = binary_targets(install);
    let mut files = Vec::with_capacity(targets.len());
    for target in targets {
        let state = inspect_file(&target);
        files.push((target, state));
    }
    InstallState { files }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn competing_writers_wait_then_read_the_committed_patch() {
        let path =
            std::env::temp_dir().join(format!("ag_patch_transaction_{}.bin", std::process::id()));
        fs::write(&path, b"..ineligible..https_proxy..").unwrap();
        let lock = PatchLock::acquire(&path).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let other = path.clone();
        let worker = std::thread::spawn(move || {
            tx.send(repatch_before_launch(&other)).unwrap();
        });
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(150)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"..ineligible..https_proxy..");
        drop(lock);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            RepatchOutcome::Repatched(1)
        );
        worker.join().unwrap();
        assert_eq!(repatch_before_launch(&path), RepatchOutcome::AlreadyPatched);
        assert_eq!(fs::read(&path).unwrap(), b"..inexigible..https_proxy..");
        fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_locked_patched_client_keeps_its_standard_proxy_name() {
        use std::os::windows::fs::OpenOptionsExt;
        let path =
            std::env::temp_dir().join(format!("ag_proxy_deferred_{}.bin", std::process::id()));
        fs::write(&path, b"..inexigible..https_proxy..").unwrap();
        let reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert_eq!(repatch_if_needed(&path), RepatchOutcome::AlreadyPatched);
        assert_eq!(fs::read(&path).unwrap(), b"..inexigible..https_proxy..");
        drop(reader);
        assert_eq!(repatch_if_needed(&path), RepatchOutcome::AlreadyPatched);
        assert_eq!(fs::read(&path).unwrap(), b"..inexigible..https_proxy..");
        fs::remove_file(path).unwrap();
    }

    /// What this machine's installs look like right now, read-only.
    ///
    /// The one question a new Antigravity release asks: is the client signature still
    /// present? `SignatureMissing` on a fresh install means the proto
    /// field moved and the patcher needs a new signature before the version is
    /// shipped (kb/patch.md); `Unpatched` means it is simply not applied yet.
    /// Reads and prints, asserts nothing about the answer - all four states are
    /// legitimate depending on what the user has done.
    ///
    ///     cargo test inspects_the_installs_on_this_machine -- --ignored --nocapture
    #[test]
    #[ignore = "reads the real installs on this machine; run with --ignored"]
    fn inspects_the_installs_on_this_machine() {
        let installs = crate::installs::discover_installs_fast();
        println!("установок найдено: {}", installs.len());
        for install in &installs {
            println!(
                "\n{} — {}",
                crate::installs::install_label(install),
                install.display()
            );
            for (path, state) in inspect_install(install).files {
                println!(
                    "  {:?} — {}",
                    state,
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
        }
    }

    #[test]
    fn replaces_every_occurrence() {
        let mut data = b"xxineligibleyyineligible".to_vec();
        assert_eq!(replace_all(&mut data, b"ineligible", b"inexigible"), 2);
        assert_eq!(&data, b"xxinexigibleyyinexigible");
    }

    #[test]
    fn matches_at_the_very_end_of_the_buffer() {
        // The previous implementation used `0..len - needle.len()`, which
        // silently skipped a match sitting at the last possible offset.
        let mut data = b"padineligible".to_vec();
        assert_eq!(replace_all(&mut data, b"ineligible", b"inexigible"), 1);
        assert_eq!(&data, b"padinexigible");
    }

    #[test]
    fn shorter_than_needle_is_not_a_panic() {
        let mut data = b"abc".to_vec();
        assert_eq!(replace_all(&mut data, b"ineligible", b"inexigible"), 0);
        assert_eq!(count_occurrences(&data, b"ineligible"), 0);
    }

    /// The watchdog's four outcomes, on real files. The signature strings are
    /// obfuscated in the binary but plain here in the test fixtures - the point
    /// is the classification, not the literals.
    #[test]
    fn repatch_classifies_each_case() {
        let dir = std::env::temp_dir().join("ag_repatch_test");
        fs::create_dir_all(&dir).expect("temp dir");

        // Reverted by an update: contains the original field name → re-patched.
        let reverted = dir.join("reverted.bin");
        fs::write(&reverted, b"..ineligible..ineligible..").unwrap();
        assert_eq!(repatch_if_needed(&reverted), RepatchOutcome::Repatched(2));
        // And now it reads back as patched, so a second pass is a no-op.
        assert_eq!(repatch_if_needed(&reverted), RepatchOutcome::AlreadyPatched);

        // A build whose signature is gone entirely: left untouched on purpose.
        let unknown = dir.join("unknown.bin");
        fs::write(&unknown, b"a totally different binary layout").unwrap();
        assert_eq!(
            repatch_if_needed(&unknown),
            RepatchOutcome::SignatureMissing
        );
        // Crucially it was NOT modified - the app must run and show its error.
        assert_eq!(
            fs::read(&unknown).unwrap(),
            b"a totally different binary layout"
        );

        // A path that does not exist is a failure, not a missing signature:
        // "retry", never "give up and hide the error".
        match repatch_if_needed(&dir.join("nope.bin")) {
            RepatchOutcome::Failed(_) => {}
            other => panic!("expected Failed, got {:?}", other),
        }

        fs::remove_dir_all(&dir).ok();
    }

    /// End-to-end check against the installed Language Server: patch a copy,
    /// confirm the signature is found, the file size is untouched and the
    /// revert is byte-exact. Heavy (copies ~140 MB), so run it explicitly:
    ///   cargo test --bin ag_unlocker -- --ignored
    ///
    /// The copy is reverted to stock first. Any machine that has actually run
    /// this tool has an already-patched Language Server, and without that step
    /// the test only ever passed on a pristine install - which is the machine
    /// least likely to be running it.
    #[test]
    #[ignore]
    fn patches_and_reverts_the_installed_language_server() {
        let Ok(local) = std::env::var("LOCALAPPDATA") else {
            return;
        };
        let src = Path::new(&local)
            .join("Programs")
            .join("Antigravity")
            .join("resources")
            .join("bin")
            .join("language_server.exe");
        if !src.exists() {
            return;
        }

        let tmp = std::env::temp_dir().join("ag_unlocker_ls_patch_test.exe");
        fs::copy(&src, &tmp).expect("copy language server");
        unpatch_proxy_var(&tmp).ok();
        unpatch_binary(&tmp).ok();
        let original = fs::read(&tmp).expect("read stock copy");
        assert!(
            count_occurrences(&original, b"ineligible") > 0,
            "could not get the copy back to stock"
        );

        let patched = patch_binary(Path::new(""), &tmp).expect("signature found");
        assert!(patched > 0, "no occurrences replaced");
        let after = fs::read(&tmp).expect("read patched");
        assert_eq!(after.len(), original.len(), "patch changed the file size");
        assert_eq!(count_occurrences(&after, b"ineligible"), 0);
        assert_eq!(count_occurrences(&after, b"inexigible"), patched);

        // Re-running must be a no-op, not an error.
        assert_eq!(patch_binary(Path::new(""), &tmp).expect("idempotent"), 0);

        assert_eq!(unpatch_binary(&tmp).expect("revert"), patched);
        assert_eq!(fs::read(&tmp).expect("read reverted"), original);

        let _ = fs::remove_file(&tmp);
    }

    #[test]
    fn patch_and_watchdog_leave_proxy_variables_alone() {
        let root = std::env::temp_dir().join(format!("ag_keep_proxy_names_{}", std::process::id()));
        let dir = root.join("resources/bin");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(if cfg!(windows) { "language_server.exe" } else { "language_server" });
        let (original, patched) = field_signatures();
        for names in [b"https_proxy HTTPS_PROXY".as_slice(), PROXY_VAR_NEW.as_bytes()] {
            let source = [original.as_slice(), names].concat();
            let expected = [patched.as_slice(), names].concat();
            fs::write(&path, &source).unwrap();
            assert_eq!(patch_all_binaries(&root).ok, 1);
            assert_eq!(fs::read(&path).unwrap(), expected);
            fs::write(&path, &source).unwrap();
            assert!(matches!(repatch_if_needed(&path), RepatchOutcome::Repatched(_)));
            assert_eq!(repatch_if_needed(&path), RepatchOutcome::AlreadyPatched);
            assert_eq!(fs::read(&path).unwrap(), expected);
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// A temp a killed writer left behind goes on the next write; one young
    /// enough to be another patcher's, and anything that is not a temp of this
    /// file, stay.
    #[test]
    fn a_stale_temp_is_swept_and_a_fresh_one_is_not() {
        let dir = std::env::temp_dir().join("ag_sweep_temps");
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("language_server.exe");
        fs::write(&target, b"x").unwrap();
        let stale = dir.join("language_server.exe.1234.agtmp");
        let fresh = dir.join("language_server.exe.5678.agtmp");
        let other = dir.join("webm_encoder.exe.1234.agtmp");
        for f in [&stale, &fresh, &other] {
            fs::write(f, b"partial").unwrap();
        }
        let old = std::time::SystemTime::now() - Duration::from_secs(3600);
        for f in [&stale, &other] {
            fs::File::options()
                .write(true)
                .open(f)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        sweep_stale_temps(&target);
        assert!(!stale.exists(), "an hour-old temp of this file is swept");
        assert!(fresh.exists(), "a young one may be another writer's");
        assert!(other.exists(), "another file's temp is not ours to judge");
        assert!(target.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn round_trip_restores_the_original_bytes() {
        let original = b"a ineligible b ineligible_tiers c".to_vec();
        let mut data = original.clone();
        replace_all(&mut data, b"ineligible", b"inexigible");
        assert_ne!(data, original);
        replace_all(&mut data, b"inexigible", b"ineligible");
        assert_eq!(data, original);
    }

    #[test]
    fn inspect_matches_across_buffer_boundary() {
        let dir = std::env::temp_dir().join("ag_inspect_boundary_test");
        fs::create_dir_all(&dir).expect("temp dir");

        let chunk_size = 1024 * 1024;
        let file_len = chunk_size + 100;
        let mut data = vec![b'x'; file_len];

        let offset = chunk_size - 5;
        data[offset..offset + 10].copy_from_slice(b"inexigible");

        let bin = dir.join("boundary_patched.bin");
        fs::write(&bin, &data).expect("write file");
        assert_eq!(inspect_file(&bin), FileState::Patched);

        data[offset..offset + 10].copy_from_slice(b"ineligible");
        let bin_unpatched = dir.join("boundary_unpatched.bin");
        fs::write(&bin_unpatched, &data).expect("write file");
        assert_eq!(inspect_file(&bin_unpatched), FileState::Unpatched);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn inspect_missing_signature_when_neither_present() {
        let dir = std::env::temp_dir().join("ag_inspect_missing_test");
        fs::create_dir_all(&dir).expect("temp dir");
        let bin = dir.join("neither.bin");
        fs::write(&bin, b"some content without any known signatures").expect("write");

        assert_eq!(inspect_file(&bin), FileState::SignatureMissing);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn inspect_unreadable_on_nonexistent_path() {
        let missing = PathBuf::from("this_file_definitely_does_not_exist_xyz_12345.exe");
        match inspect_file(&missing) {
            FileState::Unreadable(reason) => {
                assert!(!reason.is_empty(), "reason should not be empty");
            }
            other => panic!("expected FileState::Unreadable, got {:?}", other),
        }
    }

    #[test]
    fn install_state_evaluates_fully_and_partially_patched() {
        // Empty
        let empty = InstallState { files: vec![] };
        assert!(empty.is_empty());
        assert!(!empty.fully_patched());
        assert!(!empty.partially_patched());

        // All patched
        let all_patched = InstallState {
            files: vec![
                (PathBuf::from("a.exe"), FileState::Patched),
                (PathBuf::from("b.exe"), FileState::Patched),
            ],
        };
        assert!(!all_patched.is_empty());
        assert!(all_patched.fully_patched());
        assert!(!all_patched.partially_patched());

        // None patched (all unpatched)
        let all_unpatched = InstallState {
            files: vec![
                (PathBuf::from("a.exe"), FileState::Unpatched),
                (PathBuf::from("b.exe"), FileState::Unpatched),
            ],
        };
        assert!(!all_unpatched.fully_patched());
        assert!(!all_unpatched.partially_patched());

        // Some patched, some unpatched
        let mixed = InstallState {
            files: vec![
                (PathBuf::from("a.exe"), FileState::Patched),
                (PathBuf::from("b.exe"), FileState::Unpatched),
            ],
        };
        assert!(!mixed.fully_patched());
        assert!(mixed.partially_patched());

        // Some patched, some signature missing
        let mixed_missing = InstallState {
            files: vec![
                (PathBuf::from("a.exe"), FileState::Patched),
                (PathBuf::from("b.exe"), FileState::SignatureMissing),
            ],
        };
        assert!(!mixed_missing.fully_patched());
        assert!(mixed_missing.partially_patched());

        // Some patched, some unreadable
        let mixed_unreadable = InstallState {
            files: vec![
                (PathBuf::from("a.exe"), FileState::Patched),
                (
                    PathBuf::from("b.exe"),
                    FileState::Unreadable("locked".to_string()),
                ),
            ],
        };
        assert!(!mixed_unreadable.fully_patched());
        assert!(mixed_unreadable.partially_patched());
    }
}

/// Reverses the binary patch so an install can be returned to stock without
/// reinstalling.
/// Reverts every native binary of one install and reports each outcome.
///
/// Returns results rather than a count, and prints nothing: a count cannot say
/// that one file failed, and the caller used to read "reverted 1 of 2" as a
/// clean revert. The window has no console for the difference to appear on
/// either.
pub fn unpatch_all_binaries(inst: &Path) -> Vec<(String, Result<usize, String>)> {
    let mut results = Vec::new();
    for bin in binary_targets(inst) {
        let label = bin
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        // The proxy rename first: it is the one that stops the binary reading
        // our variable, and a revert that failed halfway should at least leave
        // the language server on the stock proxy channel. Silent - a build that
        // never carried it is not an error to report.
        unpatch_proxy_var(&bin).ok();
        results.push((label, unpatch_binary(&bin)));
    }
    results
}

#[cfg(test)]
mod owner_app_tests {
    use super::*;

    #[test]
    fn a_server_inside_an_install_has_the_install_s_app_and_a_lone_agy_has_none() {
        let root = std::env::temp_dir().join(format!("ag_owner_{}", std::process::id()));
        let bin = root.join("resources").join("bin");
        fs::create_dir_all(&bin).unwrap();
        let ls = bin.join("language_server.exe");
        fs::write(&ls, b"x").unwrap();
        assert_eq!(owner_app(&ls), None, "no app beside it yet");
        let app = root.join("Antigravity.exe");
        fs::write(&app, b"x").unwrap();
        assert_eq!(owner_app(&ls), Some(app));

        let lone = std::env::temp_dir().join(format!("ag_lone_{}", std::process::id()));
        fs::create_dir_all(&lone).unwrap();
        let agy = lone.join("agy.exe");
        fs::write(&agy, b"x").unwrap();
        assert_eq!(owner_app(&agy), None);
        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&lone).ok();
    }
}
