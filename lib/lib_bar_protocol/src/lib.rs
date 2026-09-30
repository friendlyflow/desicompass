//! What desicompass, its bar and its superkey share about the bar.
//!
//! * The private channel between the compositor and the bar, built like the
//!   superkey's: newline-delimited JSON, one message per line, tagged by
//!   `type`, over a socketpair the compositor hands the bar as
//!   [`ENV_IPC_FD`].
//! * The bar's settings ([`settings`]), a file of the user's that the
//!   superkey's Settings > Bar writes and the bar follows.
//! * The bar's status ([`status`]), a file in the runtime directory that the
//!   bar writes on every change and the superkey's Status section reads.
//! * The clock ([`clock`]), which both of them show.
//!
//! ```text
//! compositor -> bar
//!   {"type":"say-time"}
//! bar -> compositor
//!   {"type":"hello","version":1}
//!   {"type":"place","edge":"bottom","height":56}
//! ```
//!
//! Neither side trusts a line: one that is too long, is not JSON, or names a
//! type this version does not know is logged and skipped. See `docs/bar.md`
//! in the desicompass repository.

use serde::{Deserialize, Serialize};

pub mod clock;
pub mod settings;
pub mod status;

pub use desicompass_superkey_protocol::{LineDecoder, MAX_LINE, encode};

/// The protocol version, sent in [`FromBar::Hello`].
pub const VERSION: u32 = 1;

/// The environment variable holding the bar's end of this channel, as a file
/// descriptor number.
pub const ENV_IPC_FD: &str = "DESICOMPASS_BAR_FD";

/// Which edge of the output the bar sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Edge {
    Top,
    #[default]
    Bottom,
}

impl Edge {
    /// The stored value, as the settings file and the superkey's rows use it.
    pub fn as_str(self) -> &'static str {
        match self {
            Edge::Top => "top",
            Edge::Bottom => "bottom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "top" => Some(Edge::Top),
            "bottom" => Some(Edge::Bottom),
            _ => None,
        }
    }
}

/// What the compositor tells the bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ToBar {
    /// Super+T: say the time out loud.
    SayTime,
}

/// What the bar tells the compositor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum FromBar {
    /// The first message, naming the protocol version the bar speaks.
    Hello { version: u32 },
    /// Where the bar wants to be, and how tall it is in logical pixels. Sent
    /// at start and again whenever the position or the font scale changes.
    /// The windows are tiled in what is left of the output.
    Place { edge: Edge, height: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line<T: Serialize>(m: &T) -> String {
        String::from_utf8(encode(m)).unwrap()
    }

    #[test]
    fn messages_have_this_exact_shape() {
        assert_eq!(line(&ToBar::SayTime), "{\"type\":\"say-time\"}\n");
        assert_eq!(
            line(&FromBar::Hello { version: VERSION }),
            "{\"type\":\"hello\",\"version\":1}\n"
        );
        assert_eq!(
            line(&FromBar::Place {
                edge: Edge::Bottom,
                height: 56
            }),
            "{\"type\":\"place\",\"edge\":\"bottom\",\"height\":56}\n"
        );
    }

    #[test]
    fn every_message_round_trips() {
        for m in [
            FromBar::Hello { version: VERSION },
            FromBar::Place {
                edge: Edge::Top,
                height: 72,
            },
            FromBar::Place {
                edge: Edge::Bottom,
                height: 40,
            },
        ] {
            let back: Vec<FromBar> = LineDecoder::new().feed(&encode(&m));
            assert_eq!(back, vec![m]);
        }
        let back: Vec<ToBar> = LineDecoder::new().feed(&encode(&ToBar::SayTime));
        assert_eq!(back, vec![ToBar::SayTime]);
    }

    #[test]
    fn an_unknown_edge_drops_the_line_not_the_stream() {
        let mut d = LineDecoder::new();
        let mut bytes = b"{\"type\":\"place\",\"edge\":\"left\",\"height\":9}\n".to_vec();
        bytes.extend(encode(&FromBar::Hello { version: 1 }));
        assert_eq!(
            d.feed::<FromBar>(&bytes),
            vec![FromBar::Hello { version: 1 }]
        );
    }

    #[test]
    fn edges_parse_their_own_names_only() {
        for e in [Edge::Top, Edge::Bottom] {
            assert_eq!(Edge::parse(e.as_str()), Some(e));
        }
        assert_eq!(Edge::parse("left"), None);
        assert_eq!(Edge::default(), Edge::Bottom);
    }
}
