//! The private channel between desicompass and desicompass-superkey.
//!
//! The compositor starts the superkey itself and hands it two sockets: one it
//! inserted as a Wayland client (so the superkey's window is known by its
//! `ClientId`, not by an app_id any client could claim), and this one. Over it
//! travel newline-delimited JSON objects, one message per line, tagged by
//! `type`.
//!
//! ```text
//! compositor -> superkey
//!   {"type":"show","section":"windows","windows":[{"id":3,"title":"foot","app_id":"foot","focused":true}]}
//!   {"type":"windows","windows":[...]}
//!   {"type":"hidden"}
//! superkey -> compositor
//!   {"type":"hello","version":1}
//!   {"type":"focus","id":3}
//!   {"type":"spawn","argv":["firefox"],"cwd":null}
//!   {"type":"hide"}
//!   {"type":"quit-session"}
//! ```
//!
//! Neither side trusts a line: one that is too long, is not JSON, or names a
//! type this version does not know is logged and skipped, and the stream goes
//! on. See `docs/superkey.md` in the desicompass repository.

use serde::{Deserialize, Serialize};

/// The protocol version, sent in [`FromSuperkey::Hello`].
pub const VERSION: u32 = 1;

/// The environment variable holding the superkey's end of this channel, as a
/// file descriptor number.
pub const ENV_IPC_FD: &str = "DESICOMPASS_SUPERKEY_FD";

/// The longest line either side accepts, newline excluded. A window list of a
/// few hundred windows fits many times over.
pub const MAX_LINE: usize = 64 * 1024;

/// The longest window title passed on, in characters. A title is for reading
/// out, and some programs put a whole document path or URL in it.
pub const MAX_TITLE: usize = 256;

/// Where the superkey opens: its root, or one of the sections at the top of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Section {
    /// The whole list: the three sections, then the programs. A bare Super tap.
    Root,
    /// The open windows. Super+W.
    Windows,
    /// Suspend, reboot, power off, log out. Super+C.
    Controls,
    /// The settings: accessibility, then the bar. Super+S.
    Settings,
    /// What the bar shows, in words. Super+B.
    Status,
    /// The notifications. Super+N.
    Notifications,
}

/// One toplevel, as the superkey lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// The compositor's own id for it, which is what [`FromSuperkey::Focus`]
    /// sends back.
    pub id: u64,
    pub title: String,
    pub app_id: String,
    /// The window that had the keyboard when the superkey opened.
    #[serde(default)]
    pub focused: bool,
}

impl WindowInfo {
    /// Build one, cutting an overlong title down to [`MAX_TITLE`] characters.
    pub fn new(id: u64, title: &str, app_id: &str, focused: bool) -> Self {
        Self {
            id,
            title: truncate_chars(title, MAX_TITLE),
            app_id: truncate_chars(app_id, MAX_TITLE),
            focused,
        }
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

/// What the compositor tells the superkey.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ToSuperkey {
    /// Open on `section`. The window list is current as of now, most recently
    /// used first.
    Show {
        section: Section,
        windows: Vec<WindowInfo>,
    },
    /// The window list changed while the superkey is on screen (a window
    /// opened, closed, or was renamed).
    Windows { windows: Vec<WindowInfo> },
    /// The superkey is off screen, whoever decided it. Sent on every hide, so
    /// the superkey can stop drawing; harmless to receive twice.
    Hidden,
}

/// What the superkey asks of the compositor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FromSuperkey {
    /// The first message, naming the protocol version the superkey speaks.
    Hello { version: u32 },
    /// Hide the superkey and give the keyboard to this window.
    Focus { id: u64 },
    /// Hide the superkey and start a program. The compositor starts it, so it
    /// is the compositor's child with the compositor's environment, not the
    /// superkey's.
    Spawn {
        argv: Vec<String>,
        #[serde(default)]
        cwd: Option<String>,
    },
    /// Hide the superkey, giving the keyboard back to whichever window had it.
    Hide,
    /// End the session, as the Super+Shift+E chord does.
    QuitSession,
}

/// One message and its newline, ready to write.
pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    // Serialising these types cannot fail: every field is a string, a number,
    // a bool or a list of those.
    let mut line = serde_json::to_vec(msg).expect("protocol messages always serialise");
    line.push(b'\n');
    line
}

/// Splits a byte stream into messages.
///
/// Bytes can arrive in any chunks: half a line, or three and a half. Complete
/// lines are decoded as they appear. A line longer than [`MAX_LINE`] is thrown
/// away up to its newline, and a line that does not decode is logged and
/// skipped, so one bad message never wedges the channel.
#[derive(Debug, Default)]
pub struct LineDecoder {
    buf: Vec<u8>,
    /// Inside an overlong line: drop everything up to the next newline.
    skipping: bool,
}

impl LineDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take in `bytes` and return every message they complete.
    pub fn feed<T: for<'de> Deserialize<'de>>(&mut self, bytes: &[u8]) -> Vec<T> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                if !self.skipping {
                    let line = std::mem::take(&mut self.buf);
                    if let Some(msg) = decode_line(&line) {
                        out.push(msg);
                    }
                }
                self.buf.clear();
                self.skipping = false;
            } else if !self.skipping {
                if self.buf.len() >= MAX_LINE {
                    tracing::warn!("superkey protocol: line longer than {MAX_LINE} bytes, skipped");
                    self.buf.clear();
                    self.skipping = true;
                } else {
                    self.buf.push(b);
                }
            }
        }
        out
    }
}

fn decode_line<T: for<'de> Deserialize<'de>>(line: &[u8]) -> Option<T> {
    if line.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    match serde_json::from_slice(line) {
        Ok(msg) => Some(msg),
        Err(e) => {
            tracing::warn!(
                "superkey protocol: skipped a message ({e}): {}",
                String::from_utf8_lossy(&line[..line.len().min(200)])
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line<T: Serialize>(m: &T) -> String {
        String::from_utf8(encode(m)).unwrap()
    }

    #[test]
    fn messages_to_the_superkey_have_this_exact_shape() {
        assert_eq!(
            line(&ToSuperkey::Show {
                section: Section::Windows,
                windows: vec![WindowInfo::new(3, "foot", "foot", true)],
            }),
            "{\"type\":\"show\",\"section\":\"windows\",\"windows\":[{\"id\":3,\"title\":\"foot\",\"app_id\":\"foot\",\"focused\":true}]}\n"
        );
        assert_eq!(
            line(&ToSuperkey::Windows { windows: vec![] }),
            "{\"type\":\"windows\",\"windows\":[]}\n"
        );
        assert_eq!(line(&ToSuperkey::Hidden), "{\"type\":\"hidden\"}\n");
    }

    #[test]
    fn messages_from_the_superkey_have_this_exact_shape() {
        assert_eq!(
            line(&FromSuperkey::Hello { version: VERSION }),
            "{\"type\":\"hello\",\"version\":1}\n"
        );
        assert_eq!(
            line(&FromSuperkey::Focus { id: 3 }),
            "{\"type\":\"focus\",\"id\":3}\n"
        );
        assert_eq!(
            line(&FromSuperkey::Spawn {
                argv: vec!["firefox".into(), "--new-window".into()],
                cwd: None,
            }),
            "{\"type\":\"spawn\",\"argv\":[\"firefox\",\"--new-window\"],\"cwd\":null}\n"
        );
        assert_eq!(line(&FromSuperkey::Hide), "{\"type\":\"hide\"}\n");
        assert_eq!(
            line(&FromSuperkey::QuitSession),
            "{\"type\":\"quit-session\"}\n"
        );
    }

    #[test]
    fn every_section_round_trips() {
        for section in [
            Section::Root,
            Section::Windows,
            Section::Controls,
            Section::Settings,
            Section::Status,
            Section::Notifications,
        ] {
            let m = ToSuperkey::Show {
                section,
                windows: vec![],
            };
            let back: Vec<ToSuperkey> = LineDecoder::new().feed(&encode(&m));
            assert_eq!(back, vec![m]);
        }
    }

    #[test]
    fn a_message_split_across_reads_is_put_back_together() {
        let bytes = encode(&FromSuperkey::Focus { id: 42 });
        let mut d = LineDecoder::new();
        for chunk in bytes[..bytes.len() - 1].chunks(3) {
            assert!(d.feed::<FromSuperkey>(chunk).is_empty());
        }
        assert_eq!(
            d.feed::<FromSuperkey>(&bytes[bytes.len() - 1..]),
            vec![FromSuperkey::Focus { id: 42 }]
        );
    }

    #[test]
    fn several_messages_in_one_read_all_arrive() {
        let mut bytes = encode(&FromSuperkey::Hide);
        bytes.extend(encode(&FromSuperkey::Focus { id: 1 }));
        bytes.extend(b"{\"type\":\"quit-se");
        let mut d = LineDecoder::new();
        assert_eq!(
            d.feed::<FromSuperkey>(&bytes),
            vec![FromSuperkey::Hide, FromSuperkey::Focus { id: 1 }]
        );
        assert_eq!(
            d.feed::<FromSuperkey>(b"ssion\"}\n"),
            vec![FromSuperkey::QuitSession]
        );
    }

    #[test]
    fn bad_lines_are_skipped_and_the_stream_goes_on() {
        let mut d = LineDecoder::new();
        let mut bytes = b"not json\n{\"type\":\"teleport\"}\n\n   \n".to_vec();
        bytes.extend(encode(&FromSuperkey::Hide));
        assert_eq!(d.feed::<FromSuperkey>(&bytes), vec![FromSuperkey::Hide]);
    }

    #[test]
    fn an_overlong_line_is_dropped_up_to_its_newline() {
        let mut d = LineDecoder::new();
        let mut bytes = vec![b'x'; MAX_LINE + 10];
        bytes.push(b'\n');
        bytes.extend(encode(&FromSuperkey::Hide));
        assert_eq!(d.feed::<FromSuperkey>(&bytes), vec![FromSuperkey::Hide]);
    }

    #[test]
    fn a_missing_focused_flag_or_cwd_is_tolerated() {
        let got: Vec<ToSuperkey> = LineDecoder::new().feed(
            b"{\"type\":\"windows\",\"windows\":[{\"id\":1,\"title\":\"t\",\"app_id\":\"a\"}]}\n",
        );
        assert_eq!(
            got,
            vec![ToSuperkey::Windows {
                windows: vec![WindowInfo::new(1, "t", "a", false)]
            }]
        );
        let got: Vec<FromSuperkey> =
            LineDecoder::new().feed(b"{\"type\":\"spawn\",\"argv\":[\"foot\"]}\n");
        assert_eq!(
            got,
            vec![FromSuperkey::Spawn {
                argv: vec!["foot".into()],
                cwd: None
            }]
        );
    }

    #[test]
    fn long_titles_are_cut_on_a_character_boundary() {
        let title = "é".repeat(MAX_TITLE + 5);
        let w = WindowInfo::new(1, &title, "x", false);
        assert_eq!(
            w.title.chars().count(),
            MAX_TITLE + 1,
            "the cut is marked with an ellipsis"
        );
        assert!(w.title.ends_with('…'));
        assert_eq!(WindowInfo::new(1, "short", "x", false).title, "short");
    }
}
