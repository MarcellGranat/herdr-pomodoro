use std::collections::HashSet;
use std::io::Read;
use std::process::Child;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub const POPUP_TITLE: &str = "Herdr Pomodoro";
pub const BREAK_TIME_BUTTON: &str = "OK";
pub const KEEP_SESSION_BUTTON: &str = "Resume session";
pub const LET_END_BUTTON: &str = "Let it end";
pub const NEW_SESSION_BUTTON: &str = "Start new session";
pub const DECLINE_BUTTON: &str = "OK";
const NEW_SESSION_TIMEOUT_SECONDS: i64 = 600;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PopupKind {
    /// Focus finished; the break only starts once this is acknowledged.
    BreakTime,
    /// A grace break is about to expire and kill the session.
    KeepSession,
    /// The break is over; offers to start another focus session.
    NewSession,
}

struct PopupSpec {
    message: &'static str,
    buttons: &'static [&'static str],
    default_button: &'static str,
}

impl PopupKind {
    fn spec(self) -> PopupSpec {
        match self {
            Self::BreakTime => PopupSpec {
                message: "Focus session complete. It is break time!\n\nPress OK to start the break.",
                buttons: &[BREAK_TIME_BUTTON],
                default_button: BREAK_TIME_BUTTON,
            },
            Self::KeepSession => PopupSpec {
                message: "Your focus session ends in 20 seconds.\n\nResume it to return to focus.",
                buttons: &[LET_END_BUTTON, KEEP_SESSION_BUTTON],
                default_button: KEEP_SESSION_BUTTON,
            },
            Self::NewSession => PopupSpec {
                message: "The break is over.\n\nStart a new focus session?",
                buttons: &[DECLINE_BUTTON, NEW_SESSION_BUTTON],
                default_button: NEW_SESSION_BUTTON,
            },
        }
    }

    pub fn timeout_seconds(self, fallback: i64) -> i64 {
        match self {
            Self::NewSession => NEW_SESSION_TIMEOUT_SECONDS,
            Self::BreakTime | Self::KeepSession => fallback.max(1),
        }
    }
}

/// Result of a finished popup. `button` is `None` when nobody answered in time
/// or the dialog was dismissed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PopupOutcome {
    pub kind: PopupKind,
    pub button: Option<String>,
}

struct ActivePopup {
    kind: PopupKind,
    child: Child,
}

#[derive(Default)]
pub struct Popups {
    active: Vec<ActivePopup>,
    shown: HashSet<String>,
}

impl Popups {
    /// Shows a popup without blocking. Returns false only when the dialog could
    /// not be launched; a popup already shown for `key` counts as handled.
    pub fn show(&mut self, kind: PopupKind, key: &str, timeout_seconds: i64) -> bool {
        if self.shown.contains(key) {
            return true;
        }
        match spawn_dialog(kind, timeout_seconds) {
            Ok(child) => {
                self.shown.insert(key.to_owned());
                self.active.push(ActivePopup { kind, child });
                true
            }
            Err(_) => false,
        }
    }

    /// Closes popups whose situation no longer applies.
    pub fn cancel_unwanted(&mut self, wanted: &[PopupKind]) {
        self.active.retain_mut(|popup| {
            if wanted.contains(&popup.kind) {
                return true;
            }
            let _ = popup.child.kill();
            let _ = popup.child.wait();
            false
        });
    }

    pub fn poll(&mut self) -> Vec<PopupOutcome> {
        let mut outcomes = Vec::new();
        self.active
            .retain_mut(|popup| match popup.child.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) => {
                    let mut output = String::new();
                    if let Some(mut stdout) = popup.child.stdout.take() {
                        let _ = stdout.read_to_string(&mut output);
                    }
                    outcomes.push(PopupOutcome {
                        kind: popup.kind,
                        button: status.success().then(|| parse_answer(&output)).flatten(),
                    });
                    false
                }
                Err(_) => false,
            });
        outcomes
    }
}

impl Drop for Popups {
    fn drop(&mut self) {
        self.cancel_unwanted(&[]);
    }
}

/// Parses osascript output of the form `Button|false` (`|true` = gave up).
fn parse_answer(output: &str) -> Option<String> {
    let (button, gave_up) = output.trim().rsplit_once('|')?;
    (gave_up.trim() == "false" && !button.is_empty()).then(|| button.to_owned())
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn applescript(kind: PopupKind, timeout_seconds: i64) -> String {
    let spec = kind.spec();
    let buttons = spec
        .buttons
        .iter()
        .map(|button| format!("\"{button}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let message = spec.message.replace('\n', "\" & return & \"");
    format!(
        "tell application \"System Events\"\n\
         activate\n\
         set answer to display dialog \"{message}\" with title \"{POPUP_TITLE}\" \
         buttons {{{buttons}}} default button \"{}\" with icon note giving up after {}\n\
         return (button returned of answer) & \"|\" & (gave up of answer)\n\
         end tell",
        spec.default_button,
        kind.timeout_seconds(timeout_seconds)
    )
}

#[cfg(target_os = "macos")]
fn spawn_dialog(kind: PopupKind, timeout_seconds: i64) -> Result<Child> {
    Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(applescript(kind, timeout_seconds))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("launch osascript")
}

#[cfg(not(target_os = "macos"))]
fn spawn_dialog(_kind: PopupKind, _timeout_seconds: i64) -> Result<Child> {
    anyhow::bail!("popups require macOS")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pressed_and_expired_dialogs() {
        assert_eq!(parse_answer("OK|false\n").as_deref(), Some("OK"));
        assert_eq!(parse_answer("OK|true\n"), None);
        assert_eq!(parse_answer(""), None);
    }

    #[test]
    fn script_lists_buttons_and_timeout() {
        let script = applescript(PopupKind::KeepSession, 20);
        assert!(script.contains("buttons {\"Let it end\", \"Resume session\"}"));
        assert!(script.contains("default button \"Resume session\""));
        assert!(script.contains("giving up after 20"));
    }

    #[test]
    fn break_time_waits_for_the_fallback_timeout() {
        assert_eq!(PopupKind::BreakTime.timeout_seconds(180), 180);
        assert_eq!(PopupKind::NewSession.timeout_seconds(5), 600);
    }
}
