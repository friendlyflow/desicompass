//! The bar's icons, drawn here at the size and in the colour they are shown.
//!
//! The renderer has no icon font, no SVG and no tint (its image shader draws a
//! texture as it is), and the bar's icons must be exactly the focused row's
//! text colour at whatever size the font scale makes a line. So each icon is a
//! handful of shapes in a unit square, rasterised with signed distances
//! (anti-aliased, crisp at any size), and handed to the renderer as a PNG under
//! an `asset:` URI. Nothing is stored in the repository, and nothing needs
//! regenerating.
//!
//! Good and bad are told apart by shape, never by colour alone: a slash through
//! what is off, an exclamation mark on a network that does not reach the
//! internet, dimmed arcs for a weak signal, a bolt on a charging battery.

use std::collections::HashMap;

/// One of the bar's own icons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    /// Connected wirelessly, with `bars` of signal (0 to 3).
    Wifi {
        bars: u8,
        limited: bool,
    },
    /// Connected by cable, or some other way.
    Wired {
        limited: bool,
    },
    /// Not connected.
    NetworkOff,
    /// `level` 0 (silent) to 2 (loud).
    Volume {
        level: u8,
    },
    VolumeMuted,
    /// `level` 0 (empty) to 4 (full).
    Battery {
        level: u8,
        charging: bool,
    },
    Bluetooth {
        connected: bool,
    },
    BluetoothOff,
    Bell {
        unread: bool,
    },
}

impl Icon {
    /// The shapes, in drawing order.
    fn ops(self) -> Vec<Op> {
        let mut ops = Vec::new();
        match self {
            Icon::Wifi { bars, limited } => {
                wifi(&mut ops, bars, 1.0);
                if limited {
                    exclamation(&mut ops);
                }
            }
            Icon::Wired { limited } => {
                wired(&mut ops, 1.0);
                if limited {
                    exclamation(&mut ops);
                }
            }
            Icon::NetworkOff => {
                wifi(&mut ops, 3, DIM);
                slash(&mut ops);
            }
            Icon::Volume { level } => {
                speaker(&mut ops);
                for (i, r) in [0.2, 0.36].into_iter().enumerate() {
                    if level as usize > i {
                        ops.push(paint(arc((0.5, 0.5), r, STROKE, -0.8, 0.8)));
                    }
                }
            }
            Icon::VolumeMuted => {
                speaker(&mut ops);
                ops.push(paint(capsule((0.66, 0.36), (0.9, 0.64), STROKE / 2.0)));
                ops.push(paint(capsule((0.66, 0.64), (0.9, 0.36), STROKE / 2.0)));
            }
            Icon::Battery { level, charging } => {
                ops.push(paint(Shape::RectOutline {
                    min: (0.06, 0.26),
                    max: (0.86, 0.74),
                    r: 0.08,
                    w: 0.07,
                }));
                ops.push(paint(rrect((0.86, 0.4), (0.95, 0.6), 0.03)));
                let frac = f32::from(level.min(4)) / 4.0;
                if frac > 0.0 {
                    ops.push(paint(rrect((0.15, 0.35), (0.15 + 0.62 * frac, 0.65), 0.03)));
                }
                if charging {
                    let bolt = Shape::Polygon(vec![
                        (0.52, 0.16),
                        (0.3, 0.54),
                        (0.46, 0.54),
                        (0.4, 0.84),
                        (0.64, 0.44),
                        (0.48, 0.44),
                        (0.57, 0.16),
                    ]);
                    ops.push(Op::Erase(Shape::Grow(Box::new(bolt.clone()), 0.06)));
                    ops.push(paint(bolt));
                }
            }
            Icon::Bluetooth { connected } => {
                rune(&mut ops, 1.0);
                if connected {
                    ops.push(paint(circle((0.14, 0.5), 0.06)));
                    ops.push(paint(circle((0.86, 0.5), 0.06)));
                }
            }
            Icon::BluetoothOff => {
                rune(&mut ops, DIM);
                slash(&mut ops);
            }
            Icon::Bell { unread } => {
                let a = if unread { 1.0 } else { DIM };
                for s in [
                    circle((0.5, 0.42), 0.26),
                    rrect((0.24, 0.42), (0.76, 0.72), 0.02),
                    rrect((0.12, 0.68), (0.88, 0.77), 0.04),
                    circle((0.5, 0.14), 0.06),
                    circle((0.5, 0.85), 0.08),
                ] {
                    ops.push(Op::Paint(s, a));
                }
            }
        }
        ops
    }

    /// The icon as `size` x `size` RGBA pixels in `color` (`0xRRGGBBAA`).
    pub fn rgba(self, size: u32, color: u32) -> Vec<u8> {
        let coverage = rasterize(&self.ops(), size);
        let [r, g, b, a] = color.to_be_bytes();
        let mut out = Vec::with_capacity(coverage.len() * 4);
        for c in coverage {
            out.extend([r, g, b, (c * f32::from(a)).round() as u8]);
        }
        out
    }
}

const STROKE: f32 = 0.09;
/// How faint the parts that are off are drawn.
const DIM: f32 = 0.35;

fn wifi(ops: &mut Vec<Op>, bars: u8, alpha: f32) {
    let c = (0.5, 0.84);
    ops.push(Op::Paint(circle(c, 0.08), alpha));
    for (i, r) in [0.3, 0.5, 0.7].into_iter().enumerate() {
        let a = if (bars as usize) > i {
            alpha
        } else {
            DIM.min(alpha)
        };
        ops.push(Op::Paint(arc(c, r, STROKE, -2.36, -0.785), a));
    }
}

fn wired(ops: &mut Vec<Op>, alpha: f32) {
    let r = STROKE / 2.0;
    for s in [
        rrect((0.36, 0.1), (0.64, 0.34), 0.04),
        capsule((0.5, 0.34), (0.5, 0.55), r),
        capsule((0.22, 0.55), (0.78, 0.55), r),
        capsule((0.22, 0.55), (0.22, 0.66), r),
        capsule((0.78, 0.55), (0.78, 0.66), r),
        rrect((0.08, 0.66), (0.36, 0.9), 0.04),
        rrect((0.64, 0.66), (0.92, 0.9), 0.04),
    ] {
        ops.push(Op::Paint(s, alpha));
    }
}

fn speaker(ops: &mut Vec<Op>) {
    ops.push(paint(Shape::Polygon(vec![
        (0.08, 0.36),
        (0.28, 0.36),
        (0.5, 0.14),
        (0.5, 0.86),
        (0.28, 0.64),
        (0.08, 0.64),
    ])));
}

fn rune(ops: &mut Vec<Op>, alpha: f32) {
    let pts = [
        (0.28, 0.3),
        (0.72, 0.7),
        (0.5, 0.9),
        (0.5, 0.1),
        (0.72, 0.3),
        (0.28, 0.7),
    ];
    for w in pts.windows(2) {
        ops.push(Op::Paint(capsule(w[0], w[1], STROKE / 2.0), alpha));
    }
}

/// A slash from top left to bottom right, cut clear of what is under it.
fn slash(ops: &mut Vec<Op>) {
    let (a, b) = ((0.1, 0.1), (0.9, 0.9));
    ops.push(Op::Erase(capsule(a, b, STROKE * 1.1)));
    ops.push(paint(capsule(a, b, STROKE / 2.0)));
}

/// An exclamation mark in the bottom right corner, cut clear.
fn exclamation(ops: &mut Vec<Op>) {
    ops.push(Op::Erase(rrect((0.72, 0.5), (0.98, 1.0), 0.04)));
    ops.push(paint(capsule((0.85, 0.6), (0.85, 0.78), 0.045)));
    ops.push(paint(circle((0.85, 0.92), 0.05)));
}

// ---- Rasterising -------------------------------------------------------------

type P = (f32, f32);

#[derive(Debug, Clone)]
enum Shape {
    Capsule {
        a: P,
        b: P,
        r: f32,
    },
    Circle {
        c: P,
        r: f32,
    },
    /// A stroked arc of a circle, from angle `from` to `to` (radians, y down,
    /// so up is negative).
    Arc {
        c: P,
        r: f32,
        w: f32,
        from: f32,
        to: f32,
    },
    RoundRect {
        min: P,
        max: P,
        r: f32,
    },
    RectOutline {
        min: P,
        max: P,
        r: f32,
        w: f32,
    },
    Polygon(Vec<P>),
    /// The shape made bigger all round.
    Grow(Box<Shape>, f32),
}

#[derive(Debug, Clone)]
enum Op {
    /// Add the shape at this opacity.
    Paint(Shape, f32),
    /// Cut the shape out of what is there.
    Erase(Shape),
}

fn paint(s: Shape) -> Op {
    Op::Paint(s, 1.0)
}
fn capsule(a: P, b: P, r: f32) -> Shape {
    Shape::Capsule { a, b, r }
}
fn circle(c: P, r: f32) -> Shape {
    Shape::Circle { c, r }
}
fn arc(c: P, r: f32, w: f32, from: f32, to: f32) -> Shape {
    Shape::Arc { c, r, w, from, to }
}
fn rrect(min: P, max: P, r: f32) -> Shape {
    Shape::RoundRect { min, max, r }
}

fn len(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

fn segment_distance(p: P, a: P, b: P) -> f32 {
    let (pax, pay) = (p.0 - a.0, p.1 - a.1);
    let (bax, bay) = (b.0 - a.0, b.1 - a.1);
    let denom = bax * bax + bay * bay;
    let h = if denom > 0.0 {
        ((pax * bax + pay * bay) / denom).clamp(0.0, 1.0)
    } else {
        0.0
    };
    len(pax - bax * h, pay - bay * h)
}

fn round_rect_distance(p: P, min: P, max: P, r: f32) -> f32 {
    let (cx, cy) = ((min.0 + max.0) / 2.0, (min.1 + max.1) / 2.0);
    let (hx, hy) = ((max.0 - min.0) / 2.0, (max.1 - min.1) / 2.0);
    let qx = (p.0 - cx).abs() - hx + r;
    let qy = (p.1 - cy).abs() - hy + r;
    len(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Signed distance to a polygon (negative inside), by the even-odd rule.
fn polygon_distance(p: P, v: &[P]) -> f32 {
    let n = v.len();
    let mut d = f32::MAX;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        d = d.min(segment_distance(p, v[j], v[i]));
        let (vi, vj) = (v[i], v[j]);
        if (vi.1 > p.1) != (vj.1 > p.1) && p.0 < (vj.0 - vi.0) * (p.1 - vi.1) / (vj.1 - vi.1) + vi.0
        {
            inside = !inside;
        }
        j = i;
    }
    if inside { -d } else { d }
}

impl Shape {
    /// Signed distance from `p` to the shape's edge, negative inside, in
    /// units of the icon's size.
    fn distance(&self, p: P) -> f32 {
        match self {
            Shape::Capsule { a, b, r } => segment_distance(p, *a, *b) - r,
            Shape::Circle { c, r } => len(p.0 - c.0, p.1 - c.1) - r,
            Shape::Arc { c, r, w, from, to } => {
                let (dx, dy) = (p.0 - c.0, p.1 - c.1);
                let angle = dy.atan2(dx);
                if (*from..=*to).contains(&angle) {
                    (len(dx, dy) - r).abs() - w / 2.0
                } else {
                    // Past either end: round caps.
                    let end = |t: f32| (c.0 + r * t.cos(), c.1 + r * t.sin());
                    let (e0, e1) = (end(*from), end(*to));
                    len(p.0 - e0.0, p.1 - e0.1).min(len(p.0 - e1.0, p.1 - e1.1)) - w / 2.0
                }
            }
            Shape::RoundRect { min, max, r } => round_rect_distance(p, *min, *max, *r),
            Shape::RectOutline { min, max, r, w } => {
                round_rect_distance(p, *min, *max, *r).abs() - w / 2.0
            }
            Shape::Polygon(v) => polygon_distance(p, v),
            Shape::Grow(s, by) => s.distance(p) - by,
        }
    }
}

/// Coverage, 0 to 1, of every pixel of a `size` x `size` icon, row by row.
fn rasterize(ops: &[Op], size: u32) -> Vec<f32> {
    let size = size.max(1);
    let s = size as f32;
    let mut out = vec![0.0f32; (size * size) as usize];
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
            let mut c = 0.0f32;
            for op in ops {
                match op {
                    Op::Paint(shape, alpha) => {
                        let cov = (0.5 - shape.distance(p) * s).clamp(0.0, 1.0);
                        c = c.max(cov * alpha);
                    }
                    Op::Erase(shape) => {
                        let cov = (0.5 - shape.distance(p) * s).clamp(0.0, 1.0);
                        c *= 1.0 - cov;
                    }
                }
            }
            out[(y * size + x) as usize] = c;
        }
    }
    out
}

// ---- Handing them to the renderer -------------------------------------------

/// The renderer's name for this program's assets.
pub const ASSET_PROVIDER: &str = "desicompass-bar";

/// Encode RGBA pixels as a PNG.
pub fn png(width: u32, height: u32, rgba: Vec<u8>) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(width, height, rgba)?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Icons already handed to the renderer, by what they are.
///
/// Each distinct icon is registered once and never again: the renderer caches
/// textures by URI, so bytes behind a URI must never change. A new size or
/// colour is a new URI. The set is small and bounded (a few dozen icons, and
/// the tray's, which are keyed by their pixels), so registrations are kept for
/// the life of the process.
#[derive(Default)]
pub struct IconCache {
    uris: HashMap<String, String>,
}

impl IconCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The URI of `icon` at `size` in `color`, drawing it the first time.
    pub fn icon(&mut self, icon: Icon, size: u32, color: u32) -> Option<String> {
        let key = format!("{icon:?}-{size}-{color:08x}");
        if let Some(uri) = self.uris.get(&key) {
            return Some(uri.clone());
        }
        let bytes = png(size, size, icon.rgba(size, color))?;
        Some(self.register(key, bytes))
    }

    /// The URI of a picture someone else drew (a tray item's), keyed by its
    /// pixels, so an animation's frames are registered once each.
    pub fn picture(&mut self, width: u32, height: u32, rgba: &[u8]) -> Option<String> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (width, height, rgba).hash(&mut h);
        let key = format!("tray-{:016x}", h.finish());
        if let Some(uri) = self.uris.get(&key) {
            return Some(uri.clone());
        }
        let bytes = png(width, height, rgba.to_vec())?;
        Some(self.register(key, bytes))
    }

    fn register(&mut self, key: String, bytes: Vec<u8>) -> String {
        let name = format!("{key}.png");
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        sicompass_sdk::assets::register_bytes(ASSET_PROVIDER, &name, bytes);
        let uri = sicompass_sdk::assets::uri(ASSET_PROVIDER, &name);
        self.uris.insert(key, uri.clone());
        uri
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[Icon] = &[
        Icon::Wifi {
            bars: 3,
            limited: false,
        },
        Icon::Wifi {
            bars: 0,
            limited: true,
        },
        Icon::Wired { limited: false },
        Icon::Wired { limited: true },
        Icon::NetworkOff,
        Icon::Volume { level: 2 },
        Icon::Volume { level: 0 },
        Icon::VolumeMuted,
        Icon::Battery {
            level: 4,
            charging: false,
        },
        Icon::Battery {
            level: 1,
            charging: true,
        },
        Icon::Bluetooth { connected: true },
        Icon::BluetoothOff,
        Icon::Bell { unread: true },
        Icon::Bell { unread: false },
    ];

    fn ink(icon: Icon, size: u32) -> f32 {
        rasterize(&icon.ops(), size).iter().sum::<f32>() / (size * size) as f32
    }

    #[test]
    fn every_icon_draws_something_but_not_a_solid_square() {
        for &icon in ALL {
            let i = ink(icon, 32);
            assert!(i > 0.05 && i < 0.8, "{icon:?}: {i}");
        }
    }

    #[test]
    fn the_pixels_are_the_colour_asked_for() {
        let px = Icon::Bell { unread: true }.rgba(16, 0x2D4A28FF);
        assert_eq!(px.len(), 16 * 16 * 4);
        for p in px.chunks(4) {
            assert_eq!(&p[..3], &[0x2D, 0x4A, 0x28]);
        }
        assert!(px.chunks(4).any(|p| p[3] == 255));
        assert!(px.chunks(4).any(|p| p[3] == 0));
    }

    #[test]
    fn more_is_drawn_for_more() {
        let battery = |level| {
            ink(
                Icon::Battery {
                    level,
                    charging: false,
                },
                48,
            )
        };
        assert!(battery(4) > battery(2) && battery(2) > battery(0));
        let wifi = |bars| {
            ink(
                Icon::Wifi {
                    bars,
                    limited: false,
                },
                48,
            )
        };
        assert!(wifi(3) > wifi(1));
        assert!(ink(Icon::Volume { level: 2 }, 48) > ink(Icon::Volume { level: 0 }, 48));
    }

    #[test]
    fn off_looks_different_from_on() {
        let on = rasterize(&Icon::Bluetooth { connected: false }.ops(), 32);
        let off = rasterize(&Icon::BluetoothOff.ops(), 32);
        assert_ne!(on, off);
    }

    #[test]
    fn distances_are_signed() {
        let c = circle((0.5, 0.5), 0.25);
        assert!(c.distance((0.5, 0.5)) < 0.0);
        assert!(c.distance((0.0, 0.0)) > 0.0);
        let tri = Shape::Polygon(vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)]);
        assert!(tri.distance((0.2, 0.2)) < 0.0);
        assert!(tri.distance((0.9, 0.9)) > 0.0);
        let arc = arc((0.5, 0.5), 0.3, 0.1, -2.0, -1.0);
        // On the arc, straight up from the centre.
        assert!(arc.distance((0.5, 0.2)) < 0.0);
        // Straight down is outside its range.
        assert!(arc.distance((0.5, 0.8)) > 0.0);
    }

    #[test]
    fn a_png_decodes_back_to_the_same_pixels() {
        let px = Icon::VolumeMuted.rgba(12, 0xFFFFFFFF);
        let bytes = png(12, 12, px.clone()).unwrap();
        let back = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(back.into_raw(), px);
        assert!(png(3, 3, vec![0; 5]).is_none(), "the wrong number of bytes");
    }

    #[test]
    fn each_icon_is_registered_once_per_size_and_colour() {
        let mut cache = IconCache::new();
        let icon = Icon::Bell { unread: true };
        let a = cache.icon(icon, 20, 0xFFFFFFFF).unwrap();
        assert_eq!(cache.icon(icon, 20, 0xFFFFFFFF).unwrap(), a);
        assert_ne!(cache.icon(icon, 20, 0x000000FF).unwrap(), a);
        assert_ne!(cache.icon(icon, 24, 0xFFFFFFFF).unwrap(), a);
        assert!(sicompass_sdk::assets::resolve(&a).is_some());
        let p = cache.picture(1, 1, &[1, 2, 3, 4]).unwrap();
        assert_eq!(cache.picture(1, 1, &[1, 2, 3, 4]).unwrap(), p);
        assert_ne!(cache.picture(1, 1, &[1, 2, 3, 5]).unwrap(), p);
    }
}
