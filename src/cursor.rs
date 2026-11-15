//! The cursor: what the compositor draws where the pointer is.
//!
//! A client names a shape (`wp_cursor_shape_v1`, which SDL3 uses) or hands over
//! a surface of its own (`wl_pointer.set_cursor`). A named shape is drawn from
//! the three sprites below, which are compiled in: there is no cursor theme to
//! rely on in a session, and nothing is read from disk (the same rule as the
//! app's fonts and shaders). Every name that is not a hand or a text beam is
//! the arrow. Their size follows the session's font scale, the one the
//! superkey and the login screen set for everyone with low vision in mind, so
//! a bigger text setting gives a bigger cursor ([`font_scale`]).
//!
//! The cursor is hidden until the pointer first moves, and again after any key
//! press until it moves again ([`crate::state::State::cursor_shown`]), so someone
//! who only uses keys never sees one.

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportAll, ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
        },
    },
    input::pointer::{CursorIcon, CursorImageAttributes, CursorImageStatus},
    render_elements,
    utils::{Logical, Physical, Point, Scale, Transform},
    wayland::compositor::with_states,
};

render_elements! {
    pub CursorRenderElement<R> where R: ImportAll + ImportMem;
    Surface=WaylandSurfaceRenderElement<R>,
    Memory=MemoryRenderBufferRenderElement<R>,
}

/// `X` is the outline, `.` the fill, a space is transparent.
const ARROW: &[&str] = &[
    "X",
    "XX",
    "X.X",
    "X..X",
    "X...X",
    "X....X",
    "X.....X",
    "X......X",
    "X.......X",
    "X........X",
    "X.........X",
    "X..........X",
    "X......XXXXX",
    "X...X..X",
    "X..XX..X",
    "X.X  X..X",
    "XX   X..X",
    "X     X..X",
    "      X..X",
    "       XX",
];

const HAND: &[&str] = &[
    "     XX",
    "    X..X",
    "    X..X",
    "    X..X",
    "    X..X",
    "    X..XXX",
    "    X..X..XXX",
    "    X..X..X..XX",
    "    X..X..X..X.X",
    "XXX X..X..X..X..X",
    "X..XX........X..X",
    "X...X...........X",
    " X..............X",
    "  X.............X",
    "  X............X",
    "   X...........X",
    "   X..........X",
    "    X.........X",
    "     X.......X",
    "     X.......X",
    "     XXXXXXXXX",
];

/// The text beam is dark with a light edge, so it shows on light and dark text.
const BEAM: &[&str] = &[
    " ..... ",
    ".XX.XX.",
    " ..X.. ",
    "  .X.  ",
    "  .X.  ",
    "  .X.  ",
    "  .X.  ",
    "  .X.  ",
    "  .X.  ",
    "  .X.  ",
    "  .X.  ",
    "  .X.  ",
    "  .X.  ",
    "  .X.  ",
    " ..X.. ",
    ".XX.XX.",
    " ..... ",
];

/// One shape, ready to draw, with the point of it that is the pointer.
pub struct Sprite {
    buffer: MemoryRenderBuffer,
    hotspot: Point<i32, Logical>,
}

/// ARGB8888 bytes (little-endian, so B, G, R, A) of `art`, each cell `scale`
/// pixels square (nearest neighbour, so a fractional scale stays sharp), and the
/// size in pixels. `X` is drawn in `outline`, `.` in `fill`.
fn pixels(art: &[&str], scale: f64, outline: u32, fill: u32) -> (Vec<u8>, i32, i32) {
    let w = art.iter().map(|l| l.len()).max().unwrap_or(0);
    let h = art.len();
    let scale = scale.max(1.0);
    let (pw, ph) = (
        (w as f64 * scale).round() as usize,
        (h as f64 * scale).round() as usize,
    );
    let mut out = vec![0u8; pw * ph * 4];
    for py in 0..ph {
        let Some(line) = art.get((py as f64 / scale) as usize) else {
            continue;
        };
        for px in 0..pw {
            let argb = match line.as_bytes().get((px as f64 / scale) as usize) {
                Some(b'X') => outline,
                Some(b'.') => fill,
                _ => continue,
            };
            let i = (py * pw + px) * 4;
            out[i..i + 4].copy_from_slice(&argb.to_le_bytes());
        }
    }
    (out, pw as i32, ph as i32)
}

const BLACK: u32 = 0xFF00_0000;
const WHITE: u32 = 0xFFFF_FFFF;

impl Sprite {
    /// `art` drawn `size` times its cells in logical pixels, on an output of
    /// integer scale `output`.
    fn new(
        art: &[&str],
        output: i32,
        size: f64,
        hotspot: (i32, i32),
        outline: u32,
        fill: u32,
    ) -> Self {
        let output = output.max(1);
        let (mem, w, h) = pixels(art, f64::from(output) * size, outline, fill);
        let at = |c: i32| (f64::from(c) * size).round() as i32;
        Sprite {
            buffer: MemoryRenderBuffer::from_slice(
                &mem,
                Fourcc::Argb8888,
                (w, h),
                output,
                Transform::Normal,
                None,
            ),
            hotspot: (at(hotspot.0), at(hotspot.1)).into(),
        }
    }
}

/// The three built-in shapes.
pub struct Sprites {
    arrow: Sprite,
    hand: Sprite,
    beam: Sprite,
}

impl Sprites {
    /// The shapes for an output of integer scale `output`, at `size` times their
    /// cells: the session's font scale ([`font_scale`]), so a bigger text setting
    /// gives a bigger cursor too.
    pub fn new(output: i32, size: f64) -> Self {
        Sprites {
            arrow: Sprite::new(ARROW, output, size, (0, 0), BLACK, WHITE),
            hand: Sprite::new(HAND, output, size, (5, 0), BLACK, WHITE),
            beam: Sprite::new(BEAM, output, size, (3, 8), BLACK, WHITE),
        }
    }

    fn for_icon(&self, icon: CursorIcon) -> &Sprite {
        match icon {
            CursorIcon::Pointer | CursorIcon::Grab | CursorIcon::Grabbing => &self.hand,
            CursorIcon::Text | CursorIcon::VerticalText => &self.beam,
            _ => &self.arrow,
        }
    }
}

/// The cursor's elements at `location` (the pointer's position), front first, as
/// a frame's element list wants them: empty when it is hidden.
pub fn render_elements<R>(
    renderer: &mut R,
    sprites: &Sprites,
    status: &CursorImageStatus,
    location: Point<f64, Logical>,
    scale: Scale<f64>,
) -> Vec<CursorRenderElement<R>>
where
    R: Renderer + ImportAll + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    match status {
        CursorImageStatus::Hidden => Vec::new(),
        CursorImageStatus::Named(icon) => {
            let sprite = sprites.for_icon(*icon);
            let at: Point<f64, Physical> = (location - sprite.hotspot.to_f64()).to_physical(scale);
            MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                at,
                &sprite.buffer,
                None,
                None,
                None,
                Kind::Cursor,
            )
            .map(|e| vec![CursorRenderElement::Memory(e)])
            .unwrap_or_default()
        }
        CursorImageStatus::Surface(surface) => {
            let hotspot = with_states(surface, |states| {
                states
                    .data_map
                    .get::<std::sync::Mutex<CursorImageAttributes>>()
                    .and_then(|a| a.lock().ok().map(|a| a.hotspot))
                    .unwrap_or_default()
            });
            let at = (location - hotspot.to_f64()).to_physical(scale).to_i32_round();
            render_elements_from_surface_tree(renderer, surface, at, scale, 1.0, Kind::Cursor)
        }
    }
}

/// The machine-wide accessibility settings the superkey and the login screen
/// write, over the system's defaults. The same files and the same order as
/// `sicompass_ui::accessibility` (`SHARED_PATH` over `DEFAULTS_PATH`), which the
/// compositor must not link.
const ACCESSIBILITY_FILES: [&str; 2] = [
    "/var/lib/sicompass/accessibility.json",
    "/etc/sicompass/accessibility.json",
];

/// sicompass's font scale when nothing sets one (`DEFAULT_FONT_SCALE`).
pub const DEFAULT_FONT_SCALE: f64 = 1.75;

/// The font scale in an accessibility file's text: `"1.75"` or `1.75`, clamped
/// to the range sicompass offers. `None` when the file does not set one.
fn parse_font_scale(text: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let f = match v.get("fontScale")? {
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
        serde_json::Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    f.is_finite().then(|| f.clamp(1.0, 2.5))
}

/// The session's font scale, which sizes the cursor: the first accessibility
/// file that sets one, or [`DEFAULT_FONT_SCALE`].
pub fn font_scale() -> f64 {
    ACCESSIBILITY_FILES
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok().as_deref().and_then(parse_font_scale))
        .unwrap_or(DEFAULT_FONT_SCALE)
}

/// Before a frame: a cursor surface its client destroyed is the arrow again.
pub fn refresh_status(status: &mut CursorImageStatus) {
    use smithay::utils::IsAlive;
    if let CursorImageStatus::Surface(surface) = status
        && !surface.alive()
    {
        *status = CursorImageStatus::default_named();
    }
}

/// After a frame: a client's own cursor surface may draw its next one.
pub fn send_frame(status: &CursorImageStatus, output: &smithay::output::Output, now: std::time::Duration) {
    if let CursorImageStatus::Surface(surface) = status {
        smithay::desktop::utils::send_frames_surface_tree(
            surface,
            output,
            now,
            Some(std::time::Duration::ZERO),
            |_, _| Some(output.clone()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sprite_line_is_outline_fill_or_nothing() {
        for art in [ARROW, HAND, BEAM] {
            for line in art {
                assert!(line.bytes().all(|c| matches!(c, b'X' | b'.' | b' ')), "{line:?}");
            }
        }
    }

    #[test]
    fn the_font_scale_is_read_as_sicompass_writes_it() {
        assert_eq!(parse_font_scale(r#"{"fontScale": "2.00"}"#), Some(2.0));
        assert_eq!(parse_font_scale(r#"{"fontScale": 1.5}"#), Some(1.5));
        assert_eq!(parse_font_scale(r#"{"fontScale": 9}"#), Some(2.5));
        assert_eq!(parse_font_scale(r#"{"screenReader": true}"#), None);
        assert_eq!(parse_font_scale("not json"), None);
    }

    #[test]
    fn a_bigger_font_scale_gives_a_bigger_cursor() {
        let (_, w1, h1) = pixels(ARROW, 1.0, BLACK, WHITE);
        let (_, w2, h2) = pixels(ARROW, 2.5, BLACK, WHITE);
        assert_eq!((w2, h2), ((f64::from(w1) * 2.5).round() as i32, (f64::from(h1) * 2.5).round() as i32));
    }

    #[test]
    fn a_sprite_is_scaled_cell_by_cell() {
        let (mem, w, h) = pixels(&["X.", " X"], 2.0, BLACK, WHITE);
        assert_eq!((w, h), (4, 4));
        let at = |x: usize, y: usize| {
            let i = (y * 4 + x) * 4;
            u32::from_le_bytes(mem[i..i + 4].try_into().unwrap())
        };
        assert_eq!(at(0, 0), BLACK);
        assert_eq!(at(1, 1), BLACK);
        assert_eq!(at(2, 0), WHITE);
        assert_eq!(at(0, 2), 0, "a space is transparent");
        assert_eq!(at(3, 3), BLACK);
    }

    #[test]
    fn a_hotspot_lies_on_its_sprite() {
        for (art, (x, y)) in [(ARROW, (0usize, 0usize)), (HAND, (5, 0)), (BEAM, (3, 8))] {
            let c = art[y].as_bytes().get(x).copied();
            assert!(matches!(c, Some(b'X' | b'.')), "hotspot ({x},{y}) is on {c:?}");
        }
    }
}
