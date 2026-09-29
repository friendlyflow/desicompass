//! Suspend, restart and shut down from the superkey.
//!
//! The same shape as loginsicompass's `power.rs`, which explains the choices:
//! `systemctl` *is* the logind D-Bus call, the user's own active local session
//! is allowed to make it without a polkit rule, and nothing here goes through
//! a shell. The commands are configurable so the NixOS module can pass
//! absolute paths.

use std::process::{Command, Stdio};

/// The three actions, as pre-split argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commands {
    suspend: Vec<String>,
    reboot: Vec<String>,
    poweroff: Vec<String>,
}

pub const SUSPEND: &str = "suspend";
pub const REBOOT: &str = "reboot";
pub const POWEROFF: &str = "poweroff";

impl Default for Commands {
    fn default() -> Self {
        Self {
            suspend: vec!["systemctl".into(), "suspend".into()],
            reboot: vec!["systemctl".into(), "reboot".into()],
            poweroff: vec!["systemctl".into(), "poweroff".into()],
        }
    }
}

impl Commands {
    /// Split each command line into an argv once, at startup. An empty or
    /// unusable one keeps the default, with a log line.
    pub fn from_args(suspend: &str, reboot: &str, poweroff: &str) -> Self {
        let d = Self::default();
        Self {
            suspend: parse_one("suspend", suspend, d.suspend),
            reboot: parse_one("reboot", reboot, d.reboot),
            poweroff: parse_one("poweroff", poweroff, d.poweroff),
        }
    }

    fn argv(&self, which: &str) -> Option<&[String]> {
        match which {
            SUSPEND => Some(&self.suspend),
            REBOOT => Some(&self.reboot),
            POWEROFF => Some(&self.poweroff),
            _ => None,
        }
    }

    /// Spawn the action. Never waits: `systemctl poweroff` returns once the job
    /// is queued.
    pub fn run(&self, which: &str) -> std::io::Result<()> {
        let Some(argv) = self.argv(which) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown power action '{which}'"),
            ));
        };
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // Reaped on a thread of its own, so it never lingers as a zombie and
        // the superkey never waits on it.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    /// The fluent key of an action's button label.
    pub fn label_key(which: &str) -> Option<&'static str> {
        Some(match which {
            SUSPEND => "superkey-button-suspend",
            REBOOT => "superkey-button-reboot",
            POWEROFF => "superkey-button-poweroff",
            _ => return None,
        })
    }

    /// The fluent key of what is said just before the action is spawned.
    pub fn announcement_key(which: &str) -> Option<&'static str> {
        Some(match which {
            SUSPEND => "superkey-announce-suspend",
            REBOOT => "superkey-announce-reboot",
            POWEROFF => "superkey-announce-poweroff",
            _ => return None,
        })
    }
}

fn parse_one(label: &str, given: &str, fallback: Vec<String>) -> Vec<String> {
    let argv = split_words(given);
    if argv.is_empty() {
        if !given.is_empty() {
            tracing::warn!("unusable --{label}-command; keeping {fallback:?}");
        }
        return fallback;
    }
    argv
}

/// Split on whitespace, honouring single and double quotes so a store path
/// with a space in it survives. Not a shell: no expansion, no escapes.
fn split_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match c {
            '\'' | '"' if quote.is_none() => {
                quote = Some(c);
                has = true;
            }
            c if Some(c) == quote => quote = None,
            c if c.is_whitespace() && quote.is_none() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_systemctl() {
        let c = Commands::default();
        assert_eq!(c.argv(SUSPEND).unwrap(), ["systemctl", "suspend"]);
        assert_eq!(c.argv(REBOOT).unwrap(), ["systemctl", "reboot"]);
        assert_eq!(c.argv(POWEROFF).unwrap(), ["systemctl", "poweroff"]);
    }

    #[test]
    fn given_commands_are_split_without_a_shell() {
        let c = Commands::from_args(
            "/run/current-system/sw/bin/systemctl suspend",
            "'/opt/my tools/reboot' now",
            "",
        );
        assert_eq!(
            c.argv(SUSPEND).unwrap(),
            ["/run/current-system/sw/bin/systemctl", "suspend"]
        );
        assert_eq!(c.argv(REBOOT).unwrap(), ["/opt/my tools/reboot", "now"]);
        assert_eq!(
            c.argv(POWEROFF).unwrap(),
            ["systemctl", "poweroff"],
            "empty keeps the default"
        );
    }

    #[test]
    fn an_unknown_action_is_an_error_not_a_spawn() {
        assert!(Commands::default().run("hibernate").is_err());
    }

    #[test]
    fn a_configured_command_is_run() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("ran");
        let c = Commands::from_args(&format!("touch {}", marker.display()), "", "");
        c.run(SUSPEND).unwrap();
        for _ in 0..100 {
            if marker.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the suspend command never ran");
    }

    #[test]
    fn every_action_has_a_label_and_an_announcement() {
        for w in [SUSPEND, REBOOT, POWEROFF] {
            assert!(Commands::label_key(w).is_some());
            assert!(Commands::announcement_key(w).is_some());
        }
        assert!(Commands::label_key("x").is_none());
    }
}
