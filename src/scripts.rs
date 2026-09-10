//! Contained script discovery and replacement-process terminal handoff.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use crate::error::{DegaussError, Result};

pub const RETURN_ENV: &str = "DEGAUSS_SCRIPTS_RETURN";

/// Main's launcher can supply tty2 descriptors without a controlling terminal.
/// ISIG needs both that association and a foreground process group to deliver
/// VINTR to the script. Pipes used by headless callers need neither.
#[cfg(unix)]
fn acquire_script_terminal() -> Result<()> {
    let failure = |what| DegaussError::io(what, "/dev/stdin", std::io::Error::last_os_error());
    unsafe {
        if libc::isatty(libc::STDIN_FILENO) != 1 {
            return Ok(());
        }
        let session = libc::getsid(0);
        if session == -1 {
            return Err(failure("read script session"));
        }
        if libc::tcgetsid(libc::STDIN_FILENO) == session {
            return Ok(());
        }
        let pid = libc::getpid();
        if session != pid {
            // A process-group leader cannot call setsid. Joining the existing
            // session leader's group first permits a session without a fork.
            if libc::getpgrp() == pid && libc::setpgid(0, session) == -1 {
                return Err(failure(
                    "leave script process group before terminal acquisition",
                ));
            }
            if libc::setsid() == -1 {
                return Err(failure("create script terminal session"));
            }
        }
        // Never steal a terminal from another session (the force argument is 0).
        if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) == -1 {
            return Err(failure("acquire script controlling terminal"));
        }
        if libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp()) == -1 {
            return Err(failure("set script foreground process group"));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_directory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browser {
    root: PathBuf,
}

fn canonical(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .map_err(|error| DegaussError::io("resolve script path", path, error))
}

fn forbidden(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).map_or(true, |relative| {
        relative.components().any(|component| match component {
            Component::Normal(name) => name.to_string_lossy().starts_with('.'),
            Component::ParentDir => true,
            _ => false,
        })
    })
}

fn is_script(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sh"))
        && !path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("degauss.sh"))
}

impl Browser {
    pub fn open(menu_root: &Path) -> Result<Self> {
        let root = canonical(&menu_root.join("Scripts"))?;
        if !root.is_dir() {
            return Err(DegaussError::malformed(
                "Scripts directory",
                root,
                "not a directory",
            ));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Accepts the absolute directory returned by root() or an Entry. Only
    /// this directory is enumerated, never its descendants.
    pub fn list(&self, directory: &Path) -> Result<Vec<Entry>> {
        let directory = canonical(directory)?;
        if forbidden(&directory, &self.root) {
            return Err(DegaussError::malformed(
                "Scripts directory",
                directory,
                "outside the visible Scripts tree",
            ));
        }
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| DegaussError::io("read Scripts directory", &directory, error))?
        {
            let entry =
                entry.map_err(|error| DegaussError::io("read script entry", &directory, error))?;
            let path = entry.path();
            if forbidden(&path, &self.root) {
                continue;
            }
            let target = canonical(&path)?;
            if forbidden(&target, &self.root) {
                continue;
            }
            let metadata = std::fs::metadata(&target)
                .map_err(|error| DegaussError::io("inspect script entry", &target, error))?;
            let is_directory = metadata.is_dir();
            if !is_directory && !(metadata.is_file() && is_script(&path) && is_script(&target)) {
                continue;
            }
            entries.push(Entry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: target,
                is_directory,
            });
        }
        entries.sort_by(|a, b| {
            b.is_directory
                .cmp(&a.is_directory)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }
}

// User-controlled paths and arguments are positional parameters, never shell
// source. A caught INT lets the child receive Ctrl-C without losing the return
// path; its exit status is captured before any other command can replace it.
const HANDOFF: &str = r#"
script=$1
script_dir=$2
return_dir=$3
frontend=$4
shift 4
trap ':' INT
if cd -- "$script_dir"; then
    /bin/bash -- "$script"
    script_status=$?
    if [ "$script_status" -ne 0 ]; then
        printf '\nScript exited with status %s: %s\n' "$script_status" "$script" >&2
    fi
else
    printf '\nCannot enter script directory: %s\n' "$script_dir" >&2
fi
printf '\nPress any key to return to Degauss...'
IFS= read -r -n 1 return_key
printf '\n'
if ! cd -- "$return_dir"; then
    printf 'Cannot restore frontend directory: %s\n' "$return_dir" >&2
    exit 1
fi
trap - INT
export DEGAUSS_SCRIPTS_RETURN="$script"
exec "$frontend" "$@"
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    root: PathBuf,
    script: PathBuf,
    script_directory: PathBuf,
    return_directory: PathBuf,
    executable: PathBuf,
    args: Vec<OsString>,
}

impl Launch {
    /// Run before dropping the UI and terminal guards. args excludes argv[0].
    pub fn prepare(
        root: &Path,
        script: &Path,
        executable: &Path,
        args: Vec<OsString>,
    ) -> Result<Self> {
        let root = canonical(root)?;
        let selected = canonical(script)?;
        if forbidden(script, &root)
            || forbidden(&selected, &root)
            || !is_script(script)
            || !is_script(&selected)
            || !selected.is_file()
        {
            return Err(DegaussError::malformed(
                "script selection",
                script,
                "not a visible .sh file inside Scripts",
            ));
        }
        std::fs::File::open(&selected)
            .map_err(|error| DegaussError::io("open selected script", &selected, error))?;
        let executable = canonical(executable)?;
        let metadata = std::fs::metadata(&executable)
            .map_err(|error| DegaussError::io("inspect frontend executable", &executable, error))?;
        #[cfg(unix)]
        let executable_mode = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable_mode = true;
        if !metadata.is_file() || !executable_mode {
            return Err(DegaussError::malformed(
                "frontend executable",
                executable,
                "not an executable file",
            ));
        }
        std::fs::metadata("/bin/bash")
            .map_err(|error| DegaussError::io("inspect script interpreter", "/bin/bash", error))?;
        let return_directory = std::env::current_dir()
            .map_err(|error| DegaussError::io("read frontend working directory", ".", error))?;
        Ok(Self {
            root,
            script_directory: selected
                .parent()
                .expect("contained script has a parent")
                .into(),
            script: selected,
            return_directory,
            executable,
            args,
        })
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn script(&self) -> &Path {
        &self.script
    }

    /// Repeat at confirmation, while validation failures can still be shown
    /// in the browser. exec repeats this after the terminal is restored.
    pub fn validate(&self) -> Result<()> {
        let selected = canonical(&self.script)?;
        if forbidden(&selected, &self.root) || !is_script(&selected) || !selected.is_file() {
            return Err(DegaussError::malformed(
                "script selection",
                &self.script,
                "selected file changed or escaped the visible Scripts tree",
            ));
        }
        std::fs::File::open(&selected)
            .map_err(|error| DegaussError::io("open selected script", &selected, error))?;
        Ok(())
    }

    fn command(&self) -> Command {
        let mut command = Command::new("/bin/bash");
        command
            .arg("-c")
            .arg(HANDOFF)
            .arg("degauss-script")
            .arg(&self.script)
            .arg(&self.script_directory)
            .arg(&self.return_directory)
            .arg(&self.executable)
            .args(&self.args)
            .env_remove(RETURN_ENV);
        command
    }

    /// Call only after App, input, framebuffer and console guards are dropped.
    #[cfg(unix)]
    pub fn exec(self) -> Result<std::convert::Infallible> {
        use std::os::unix::process::CommandExt;
        self.validate()?;
        acquire_script_terminal()?;
        let error = self.command().exec();
        Err(DegaussError::io(
            "replace frontend with script interpreter",
            "/bin/bash",
            error,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::Stdio;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "degauss-scripts-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(root.join("Scripts/sub folder")).unwrap();
            Self(root.canonicalize().unwrap())
        }
        fn file(&self, name: &str, contents: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, contents).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn listing_is_shallow_sorted_and_excludes_hidden_and_frontend() {
        let fixture = Fixture::new();
        for name in ["z.sh", "A.SH", ".hidden.sh", "degauss.sh", "notes.txt"] {
            fixture.file(&format!("Scripts/{name}"), "");
        }
        fixture.file("Scripts/sub folder/nested.sh", "");
        let browser = Browser::open(&fixture.0).unwrap();
        let entries = browser.list(browser.root()).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["sub folder", "A.SH", "z.sh"]
        );
        assert!(entries[0].is_directory);
        assert!(browser.list(&fixture.0).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_escape_or_alias_the_frontend() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let outside = fixture.file("outside.sh", "");
        let frontend = fixture.file("Scripts/degauss.sh", "");
        symlink(outside, fixture.0.join("Scripts/escape.sh")).unwrap();
        symlink(frontend, fixture.0.join("Scripts/alias.sh")).unwrap();
        let browser = Browser::open(&fixture.0).unwrap();
        assert_eq!(browser.list(browser.root()).unwrap().len(), 1);
        assert!(Launch::prepare(
            browser.root(),
            &fixture.0.join("Scripts/escape.sh"),
            Path::new("/bin/bash"),
            vec![]
        )
        .is_err());
    }

    #[test]
    fn real_shell_preserves_arguments_cwd_and_reports_nonzero_before_return() {
        let fixture = Fixture::new();
        let script = fixture.file(
            "Scripts/sub folder/a $(not-a-command);.sh",
            "printf 'SCRIPT_CWD=%s\\nCHILD_MARKER=%s\\n' \"$PWD\" \"${DEGAUSS_SCRIPTS_RETURN-unset}\"\nexit 7\n",
        );
        let browser = Browser::open(&fixture.0).unwrap();
        let launch = Launch::prepare(browser.root(), &script, Path::new("/bin/bash"), vec!["-c".into(), "printf 'RETURN_CWD=%s\\nARG=%s\\nSELECTED=%s\\n' \"$PWD\" \"$1\" \"$DEGAUSS_SCRIPTS_RETURN\"".into(), "returned".into(), "a ; $(not-a-command) ' space".into()]).unwrap();
        let mut child = launch
            .command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("CHILD_MARKER=unset"));
        assert!(stdout.contains(&format!(
            "SCRIPT_CWD={}",
            script.parent().unwrap().display()
        )));
        assert!(stdout.contains(&format!(
            "RETURN_CWD={}",
            std::env::current_dir().unwrap().display()
        )));
        assert!(stdout.contains("ARG=a ; $(not-a-command) ' space"));
        assert!(stdout.contains(&format!("SELECTED={}", script.display())));
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("Script exited with status 7"));
    }

    #[test]
    fn real_shell_missing_directory_reports_setup_failure_and_returns() {
        let fixture = Fixture::new();
        let browser = Browser::open(&fixture.0).unwrap();
        let script = fixture.file("Scripts/sub folder/check.sh", "printf SCRIPT_RAN\nexit 7\n");
        let launch = Launch::prepare(
            browser.root(),
            &script,
            Path::new("/bin/bash"),
            vec!["-c".into(), "printf FRONTEND_RETURNED".into()],
        )
        .unwrap();
        // Exercise disappearance between successful preflight and shell cd.
        std::fs::rename(script.parent().unwrap(), fixture.0.join("Scripts/moved")).unwrap();
        let mut child = launch
            .command()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("Cannot enter script directory"), "{stderr}");
        assert!(!stderr.contains("Script exited"), "{stderr}");
        assert!(!stdout.contains("SCRIPT_RAN"), "{stdout}");
        assert!(stdout.contains("Press any key to return"));
        assert!(stdout.contains("FRONTEND_RETURNED"));
    }

    #[test]
    fn real_shell_interrupt_returns_and_interactive_script_receives_input() {
        let fixture = Fixture::new();
        let browser = Browser::open(&fixture.0).unwrap();
        for (body, input, expected, status) in [
            (
                "kill -INT \"$PPID\"\nkill -INT \"$$\"\n",
                "x",
                "RETURNED",
                Some(130),
            ),
            (
                "IFS= read -r answer\nprintf 'ANSWER=%s\\n' \"$answer\"\n",
                "hello world\nx",
                "ANSWER=hello world",
                None,
            ),
        ] {
            let script = fixture.file("Scripts/check.sh", body);
            let launch = Launch::prepare(
                browser.root(),
                &script,
                Path::new("/bin/bash"),
                vec!["-c".into(), "printf RETURNED".into()],
            )
            .unwrap();
            let mut child = launch
                .command()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(stdout.contains(expected), "{stdout}");
            assert!(stdout.contains("RETURNED"));
            let stderr = String::from_utf8(output.stderr).unwrap();
            if let Some(status) = status {
                assert!(
                    stderr.contains(&format!("Script exited with status {status}")),
                    "{stderr}"
                );
            } else {
                assert!(!stderr.contains("Script exited"), "{stderr}");
            }
        }
    }

    #[test]
    fn missing_or_invalid_selection_fails_before_handoff() {
        let fixture = Fixture::new();
        let browser = Browser::open(&fixture.0).unwrap();
        assert!(Launch::prepare(
            browser.root(),
            &fixture.0.join("Scripts/missing.sh"),
            Path::new("/bin/bash"),
            vec![]
        )
        .is_err());
        let script = fixture.file("Scripts/check.sh", "exit 0");
        assert!(Launch::prepare(
            browser.root(),
            &script,
            &fixture.0.join("missing-frontend"),
            vec![]
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn confirmation_revalidation_rejects_replaced_escaping_symlink() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let browser = Browser::open(&fixture.0).unwrap();
        let script = fixture.file("Scripts/check.sh", "exit 0");
        let outside = fixture.file("outside.sh", "exit 0");
        let launch =
            Launch::prepare(browser.root(), &script, Path::new("/bin/bash"), vec![]).unwrap();
        assert!(launch.validate().is_ok());
        std::fs::remove_file(&script).unwrap();
        symlink(outside, &script).unwrap();
        assert!(launch
            .validate()
            .unwrap_err()
            .to_string()
            .contains("escaped"));
        assert!(launch.exec().unwrap_err().to_string().contains("escaped"));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_handoff_subprocess() {
        let Some(root) = std::env::var_os("DEGAUSS_TEST_SCRIPT_TTY_ROOT") else {
            return;
        };
        assert_eq!(unsafe { libc::isatty(libc::STDIN_FILENO) }, 1);
        assert_eq!(unsafe { libc::tcgetsid(libc::STDIN_FILENO) }, -1);
        let root = PathBuf::from(root);
        let launch = Launch::prepare(
            &root.join("Scripts"),
            &root.join("Scripts/interrupt.sh"),
            Path::new("/bin/bash"),
            vec!["-c".into(), "printf 'FRONTEND_RETURNED\\n'".into()],
        )
        .unwrap();
        launch.exec().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn tty_descriptors_without_controlling_terminal_deliver_ctrl_c_and_return() {
        use std::io::Read;
        use std::os::fd::FromRawFd;
        use std::os::unix::process::CommandExt;
        use std::time::{Duration, Instant};

        fn read_until(master: &mut std::fs::File, text: &mut String, wanted: &str) -> bool {
            use std::os::fd::AsRawFd;
            let deadline = Instant::now() + Duration::from_secs(8);
            while !text.contains(wanted) && Instant::now() < deadline {
                let mut descriptor = libc::pollfd {
                    fd: master.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                if unsafe { libc::poll(&mut descriptor, 1, 100) } <= 0 {
                    continue;
                }
                let mut bytes = [0; 1024];
                match master.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(size) => text.push_str(&String::from_utf8_lossy(&bytes[..size])),
                }
            }
            text.contains(wanted)
        }

        for new_session in [false, true] {
            let fixture = Fixture::new();
            fixture.file("Scripts/interrupt.sh", "printf 'SCRIPT_READY\\n'\nIFS= read -r value\nprintf 'UNEXPECTED_READ_RETURN\\n'\n");
            let (mut master_fd, mut slave_fd) = (-1, -1);
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master_fd,
                        &mut slave_fd,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            let mut master = unsafe { std::fs::File::from_raw_fd(master_fd) };
            let slave = unsafe { std::fs::File::from_raw_fd(slave_fd) };
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "scripts::tests::terminal_handoff_subprocess",
                    "--nocapture",
                ])
                .env("DEGAUSS_TEST_SCRIPT_TTY_ROOT", &fixture.0)
                .stdin(slave.try_clone().unwrap())
                .stdout(slave.try_clone().unwrap())
                .stderr(slave);
            if new_session {
                unsafe {
                    command.pre_exec(|| {
                        if libc::setsid() == -1 {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
            }
            let mut child = command.spawn().unwrap();
            let mut text = String::new();
            let ready = read_until(&mut master, &mut text, "SCRIPT_READY");
            if ready {
                master.write_all(&[3]).unwrap();
            }
            let prompt = ready && read_until(&mut master, &mut text, "Press any key to return");
            if prompt {
                master.write_all(b"x").unwrap();
            }
            let returned = prompt && read_until(&mut master, &mut text, "FRONTEND_RETURNED");
            if !returned {
                let _ = child.kill();
            }
            let status = child.wait().unwrap();
            assert!(
                ready && prompt && returned && status.success(),
                "new_session={new_session}: {text}"
            );
            assert!(text.contains("Script exited with status 130"), "{text}");
            assert!(!text.contains("UNEXPECTED_READ_RETURN"), "{text}");
        }
    }
}
