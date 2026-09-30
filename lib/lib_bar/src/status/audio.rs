//! The volume of the default output, from WirePlumber's `wpctl`.
//!
//! PipeWire has no D-Bus interface for this, and a native client would link
//! libpipewire into the bar for one number. `wpctl` is on every system that
//! runs WirePlumber, and asking it every two seconds costs nothing. Without it
//! there is no audio icon.

use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

use desicompass_bar_protocol::status::Audio;

use crate::model::Update;

const EVERY: Duration = Duration::from_secs(2);

pub fn start(tx: Sender<Update>) {
    let spawned = std::thread::Builder::new()
        .name("bar-audio".into())
        .spawn(move || {
            let mut last = None;
            loop {
                let seen = match Command::new("wpctl")
                    .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
                    .output()
                {
                    Ok(out) if out.status.success() => parse(&String::from_utf8_lossy(&out.stdout)),
                    Ok(_) => None,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        tracing::info!("audio: no wpctl, not shown");
                        let _ = tx.send(Update::Audio(None));
                        return;
                    }
                    Err(e) => {
                        tracing::debug!("audio: wpctl failed: {e}");
                        None
                    }
                };
                if last.as_ref() != Some(&seen) {
                    if tx.send(Update::Audio(seen)).is_err() {
                        return;
                    }
                    last = Some(seen);
                }
                std::thread::sleep(EVERY);
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("audio: could not start its thread: {e}");
    }
}

/// `Volume: 0.45` or `Volume: 0.45 [MUTED]`.
pub fn parse(out: &str) -> Option<Audio> {
    let rest = out.trim().strip_prefix("Volume:")?.trim();
    let mut words = rest.split_whitespace();
    let volume: f64 = words.next()?.parse().ok()?;
    let muted = words.any(|w| w == "[MUTED]");
    Some(Audio {
        volume: (volume * 100.0).round().clamp(0.0, 999.0) as u16,
        muted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wpctl_says_the_volume_and_whether_it_is_muted() {
        assert_eq!(
            parse("Volume: 0.45\n"),
            Some(Audio {
                volume: 45,
                muted: false
            })
        );
        assert_eq!(
            parse("Volume: 0.40 [MUTED]\n"),
            Some(Audio {
                volume: 40,
                muted: true
            })
        );
        assert_eq!(parse("Volume: 1.25").unwrap().volume, 125);
    }

    #[test]
    fn anything_else_is_not_a_volume() {
        for out in ["", "Volume:", "Volume: loud", "Translate ID 4 failed"] {
            assert_eq!(parse(out), None, "{out:?}");
        }
    }
}
