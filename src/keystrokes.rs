//! What the bar shows for a key press, when the user has asked it to show the
//! keys being pressed (Settings > Bar > show key strokes in the superkey).
//!
//! The compositor is the only process that sees every key, so it names each
//! press here and sends the name to the bar (`ToBar::Key`), and only while the
//! bar has said it wants them (`FromBar::ShowKeys`).
//!
//! ```text
//! a  A  é  ␣  Enter  Backspace  ←  Shift+Tab  Ctrl+C  Ctrl+Shift+T  Super+J
//! ```
//!
//! Typing shows what it types, as the layout and Shift made it. A key held
//! with Ctrl, Alt or Super is a chord, named by its unshifted key so it reads
//! like the binding it is. A modifier on its own shows nothing: it shows as
//! part of the next key.

use smithay::input::keyboard::xkb::Keysym;

/// The modifiers held when the key went down.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Held {
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
    pub shift: bool,
}

/// The name of one key press: `sym` is the key's unshifted (Latin) keysym,
/// `text` what it types with the modifiers held. `None` for a modifier key.
pub fn label(held: Held, sym: Keysym, text: Option<char>) -> Option<String> {
    if sym.is_modifier_key() || sym == Keysym::Caps_Lock {
        return None;
    }
    let chord = held.ctrl || held.alt || held.logo;
    if !chord && let Some(c) = text.filter(|c| !c.is_control()) {
        return Some(if c == ' ' {
            "\u{2423}".into()
        } else {
            c.into()
        });
    }
    let mut out = String::new();
    for (on, name) in [
        (held.ctrl, "Ctrl+"),
        (held.alt, "Alt+"),
        (held.logo, "Super+"),
        (held.shift, "Shift+"),
    ] {
        if on {
            out.push_str(name);
        }
    }
    out.push_str(&key_name(sym));
    Some(out)
}

/// A key's name on its own: a letter as on its cap, the others as a keyboard
/// labels them.
fn key_name(sym: Keysym) -> String {
    let named = match sym {
        Keysym::Return | Keysym::KP_Enter => "Enter",
        Keysym::BackSpace => "Backspace",
        Keysym::Escape => "Esc",
        Keysym::Tab | Keysym::ISO_Left_Tab => "Tab",
        Keysym::Delete | Keysym::KP_Delete => "Del",
        Keysym::Insert | Keysym::KP_Insert => "Ins",
        Keysym::space => "Space",
        Keysym::Left | Keysym::KP_Left => "\u{2190}",
        Keysym::Up | Keysym::KP_Up => "\u{2191}",
        Keysym::Right | Keysym::KP_Right => "\u{2192}",
        Keysym::Down | Keysym::KP_Down => "\u{2193}",
        Keysym::Prior | Keysym::KP_Prior => "PgUp",
        Keysym::Next | Keysym::KP_Next => "PgDn",
        Keysym::Home | Keysym::KP_Home => "Home",
        Keysym::End | Keysym::KP_End => "End",
        Keysym::Print => "PrtSc",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_owned();
    }
    if let Some(c) = sym.key_char().filter(|c| !c.is_control() && *c != ' ') {
        return c.to_uppercase().collect();
    }
    match sym.name() {
        // `XK_F5`, `XF86XK_AudioRaiseVolume`: the part after the prefix.
        Some(n) => n.split_once("XK_").map_or(n, |(_, rest)| rest).to_owned(),
        None => format!("{:#x}", sym.raw()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Held = Held {
        ctrl: false,
        alt: false,
        logo: false,
        shift: false,
    };

    fn with(f: impl FnOnce(&mut Held)) -> Held {
        let mut h = NONE;
        f(&mut h);
        h
    }

    #[test]
    fn typing_shows_what_it_types() {
        assert_eq!(label(NONE, Keysym::a, Some('a')).as_deref(), Some("a"));
        // Shift and the layout have had their say already.
        let shift = with(|h| h.shift = true);
        assert_eq!(label(shift, Keysym::a, Some('A')).as_deref(), Some("A"));
        assert_eq!(label(shift, Keysym::_1, Some('!')).as_deref(), Some("!"));
        assert_eq!(label(NONE, Keysym::_2, Some('é')).as_deref(), Some("é"));
        assert_eq!(
            label(NONE, Keysym::space, Some(' ')).as_deref(),
            Some("\u{2423}")
        );
    }

    #[test]
    fn keys_that_type_nothing_visible_are_named() {
        for (sym, text, name) in [
            (Keysym::Return, Some('\r'), "Enter"),
            (Keysym::BackSpace, Some('\u{8}'), "Backspace"),
            (Keysym::Escape, Some('\u{1b}'), "Esc"),
            (Keysym::Tab, Some('\t'), "Tab"),
            (Keysym::Delete, Some('\u{7f}'), "Del"),
            (Keysym::Left, None, "\u{2190}"),
            (Keysym::Next, None, "PgDn"),
            (Keysym::F5, None, "F5"),
            (Keysym::XF86_AudioRaiseVolume, None, "AudioRaiseVolume"),
        ] {
            assert_eq!(label(NONE, sym, text).as_deref(), Some(name), "{sym:?}");
        }
        // Shift shows on a key that types nothing.
        let shift = with(|h| h.shift = true);
        assert_eq!(
            label(shift, Keysym::Tab, Some('\t')).as_deref(),
            Some("Shift+Tab")
        );
    }

    #[test]
    fn a_chord_is_named_by_its_key_cap() {
        let ctrl = with(|h| h.ctrl = true);
        assert_eq!(
            label(ctrl, Keysym::c, Some('\u{3}')).as_deref(),
            Some("Ctrl+C")
        );
        let ctrl_shift = with(|h| {
            h.ctrl = true;
            h.shift = true;
        });
        assert_eq!(
            label(ctrl_shift, Keysym::t, Some('\u{14}')).as_deref(),
            Some("Ctrl+Shift+T")
        );
        let all = Held {
            ctrl: true,
            alt: true,
            logo: true,
            shift: true,
        };
        assert_eq!(
            label(all, Keysym::Delete, None).as_deref(),
            Some("Ctrl+Alt+Super+Shift+Del")
        );
        let logo = with(|h| h.logo = true);
        assert_eq!(
            label(logo, Keysym::j, Some('j')).as_deref(),
            Some("Super+J")
        );
        assert_eq!(
            label(logo, Keysym::space, Some(' ')).as_deref(),
            Some("Super+Space")
        );
        let alt = with(|h| h.alt = true);
        assert_eq!(
            label(alt, Keysym::minus, Some('-')).as_deref(),
            Some("Alt+-")
        );
    }

    #[test]
    fn a_modifier_alone_shows_nothing() {
        for sym in [
            Keysym::Shift_L,
            Keysym::Control_R,
            Keysym::Alt_L,
            Keysym::Super_L,
            Keysym::ISO_Level3_Shift,
            Keysym::Caps_Lock,
            Keysym::Num_Lock,
        ] {
            assert_eq!(label(with(|h| h.ctrl = true), sym, None), None, "{sym:?}");
        }
    }
}
