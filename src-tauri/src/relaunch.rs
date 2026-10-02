//! User-requested relaunch through LaunchServices for packaged macOS apps.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::error::CmdError;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RelaunchTarget {
    Bundle(PathBuf),
    Executable,
}

pub(crate) fn prepare(app: &AppHandle) -> Result<RelaunchTarget, CmdError> {
    // Tauri validates the current executable path, including its macOS symlink
    // protections. Derive the enclosing bundle only from that trusted path.
    let executable = tauri::process::current_binary(&app.env())
        .map_err(|error| CmdError::other(format!("could not locate Tomari to restart: {error}")))?;
    Ok(target_for_executable(&executable))
}

fn target_for_executable(executable: &Path) -> RelaunchTarget {
    if executable.is_absolute()
        && let Some(macos) = executable.parent()
        && macos.file_name().is_some_and(|name| name == "MacOS")
        && let Some(contents) = macos.parent()
        && contents.file_name().is_some_and(|name| name == "Contents")
        && let Some(bundle) = contents.parent()
        && bundle
            .extension()
            .is_some_and(|extension| extension == "app")
    {
        RelaunchTarget::Bundle(bundle.to_owned())
    } else {
        RelaunchTarget::Executable
    }
}

/// Called only after terminal cleanup has completed and released every gate.
pub(crate) fn restart(app: &AppHandle, target: &RelaunchTarget) {
    restart_with(
        target,
        |bundle| {
            std::process::Command::new("/usr/bin/open")
                .arg("-n")
                .arg(bundle)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map(|_| ())
        },
        || app.exit(0),
        || app.restart(),
    );
}

fn restart_with(
    target: &RelaunchTarget,
    open_bundle: impl FnOnce(&Path) -> std::io::Result<()>,
    exit: impl FnOnce(),
    restart_executable: impl FnOnce(),
) {
    if let RelaunchTarget::Bundle(bundle) = target {
        match open_bundle(bundle) {
            Ok(()) => {
                // Do not wait for open: the new instance waits for this process
                // to exit and release its lock before it can finish launching.
                exit();
                return;
            }
            Err(error) => {
                // Cleanup is already terminal. Tauri's direct executable
                // relaunch is preferable to leaving a stopped app on screen.
                tracing::error!(%error, "could not launch Tomari through LaunchServices; restarting its executable");
            }
        }
    }
    restart_executable();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_exact_absolute_bundle_executable_uses_launch_services() {
        assert_eq!(
            target_for_executable(Path::new(
                "/Applications/Tomari Test.app/Contents/MacOS/tomari"
            )),
            RelaunchTarget::Bundle(PathBuf::from("/Applications/Tomari Test.app"))
        );
        for path in [
            "/build/target/debug/tomari",
            "Tomari.app/Contents/MacOS/tomari",
            "/Applications/Tomari.app/Contents/Helpers/tomari",
            "/Applications/Tomari/Contents/MacOS/tomari",
        ] {
            assert_eq!(
                target_for_executable(Path::new(path)),
                RelaunchTarget::Executable
            );
        }
    }

    #[test]
    fn a_bundle_launch_is_spawned_before_the_old_process_exits() {
        let calls = std::cell::RefCell::new(Vec::new());
        let bundle = PathBuf::from("/Applications/Tomari Test.app");
        restart_with(
            &RelaunchTarget::Bundle(bundle.clone()),
            |path| {
                assert_eq!(path, bundle);
                calls.borrow_mut().push("open");
                Ok(())
            },
            || calls.borrow_mut().push("exit"),
            || panic!("a launched bundle must not be started twice"),
        );
        assert_eq!(*calls.borrow(), ["open", "exit"]);
    }

    #[test]
    fn a_spawn_failure_restarts_instead_of_leaving_a_terminal_process() {
        let restarted = std::cell::Cell::new(false);
        restart_with(
            &RelaunchTarget::Bundle(PathBuf::from("/Applications/Tomari.app")),
            |_| Err(std::io::Error::other("spawn failed")),
            || panic!("do not exit before requesting a replacement"),
            || restarted.set(true),
        );
        assert!(restarted.get());
    }

    #[test]
    fn unbundled_development_restarts_its_executable() {
        let restarted = std::cell::Cell::new(false);
        restart_with(
            &RelaunchTarget::Executable,
            |_| panic!("an unbundled executable cannot open as a bundle"),
            || panic!("Tauri manages executable restart"),
            || restarted.set(true),
        );
        assert!(restarted.get());
    }
}
