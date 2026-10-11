//! Explicit navigation to macOS permission settings after registering a request.

use crate::error::CmdError;

type CmdResult<T> = Result<T, CmdError>;

#[derive(Debug, Clone, Copy)]
pub enum PermissionPane {
    Accessibility,
    InputMonitoring,
}

impl PermissionPane {
    fn url(self) -> &'static str {
        match self {
            Self::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Self::InputMonitoring => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
            }
        }
    }
}

/// The native request registers Tomari with TCC, but macOS may suppress its
/// prompt after an earlier decision. Explicitly open the pane on every denied
/// request so the user's settings button remains useful on subsequent attempts.
pub fn request(pane: PermissionPane) -> CmdResult<bool> {
    #[cfg(target_os = "macos")]
    {
        request_with(
            pane,
            || match pane {
                PermissionPane::Accessibility => tomari_window::request_permission(),
                PermissionPane::InputMonitoring => crate::eventtap::request_input_monitoring(),
            },
            open_system_settings,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pane;
        Err(CmdError::other("permission settings require macOS"))
    }
}

fn request_with(
    pane: PermissionPane,
    register: impl FnOnce() -> bool,
    open: impl FnOnce(&str) -> CmdResult<()>,
) -> CmdResult<bool> {
    if register() {
        return Ok(true);
    }
    open(pane.url())?;
    Ok(false)
}

#[cfg(target_os = "macos")]
pub(crate) fn open_system_settings(url: &str) -> CmdResult<()> {
    let outcome = crate::childproc::output_with_deadline(
        std::process::Command::new("/usr/bin/open").arg(url),
        std::time::Duration::from_secs(3),
    )
    .map_err(|error| CmdError::other(format!("could not open System Settings: {error}")))?;
    match outcome {
        crate::childproc::Outcome::Exited { status, .. } if status.success() => Ok(()),
        crate::childproc::Outcome::Exited { stderr, .. } => Err(CmdError::other(format!(
            "could not open System Settings: {}",
            String::from_utf8_lossy(&stderr).trim()
        ))),
        crate::childproc::Outcome::TimedOut => {
            Err(CmdError::other("opening System Settings timed out"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_and_repeated_requests_open_the_matching_permission_pane() {
        for (pane, expected_url) in [
            (
                PermissionPane::Accessibility,
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            ),
            (
                PermissionPane::InputMonitoring,
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
            ),
        ] {
            let events = std::cell::RefCell::new(Vec::new());
            for _ in 0..2 {
                assert!(
                    !request_with(
                        pane,
                        || {
                            events.borrow_mut().push("register");
                            false
                        },
                        |url| {
                            assert_eq!(url, expected_url);
                            events.borrow_mut().push("open");
                            Ok(())
                        }
                    )
                    .unwrap()
                );
            }
            assert_eq!(*events.borrow(), ["register", "open", "register", "open"]);
        }
    }

    #[test]
    fn an_existing_grant_does_not_open_settings() {
        for pane in [
            PermissionPane::Accessibility,
            PermissionPane::InputMonitoring,
        ] {
            assert!(request_with(pane, || true, |_| panic!("already granted")).unwrap());
        }
    }

    #[test]
    fn a_settings_launch_failure_reaches_the_caller() {
        let error = request_with(
            PermissionPane::InputMonitoring,
            || false,
            |_| Err(CmdError::other("opening System Settings timed out")),
        )
        .unwrap_err();
        assert_eq!(error.message, "opening System Settings timed out");
    }
}
