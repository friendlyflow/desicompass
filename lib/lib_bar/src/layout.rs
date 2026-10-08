//! Where everything in the bar goes. Pure arithmetic, so it can be tested
//! without a window.
//!
//! ```text
//! |  tray  tray   net  bt  vol 45%  bat 81%  bell 2   Wed 30 Sep 14:05  |
//! ```
//!
//! Everything hangs from the right edge: the clock last, one em in, and the
//! status items to its left, nearest first. Each item is an icon one text
//! height square, and maybe an amount right after it. The line is centred in
//! the bar, which is 1.7 lines tall. What does not fit on the left is left
//! out, the farthest first.
//!
//! With "show key strokes" on, the bar is 2.7 lines tall: the keys being
//! pressed take its left half, one em in, in one line of twice the text's
//! size, with the same margin above and below as the normal line has. The
//! status items then stop at the middle.
//!
//! ```text
//! |  a b×3 Ctrl+C Enter          |       net  vol 45%  Wed 30 Sep 14:05  |
//! ```

/// The measures the layout needs, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub width: f32,
    pub height: f32,
    /// The height of one line of text, padding included.
    pub line_height: f32,
    /// The text's vertical padding within a line.
    pub padding: f32,
    /// The width of an M.
    pub em: f32,
}

impl Metrics {
    /// An icon's side: the line without its padding, so an icon stands as
    /// tall as the text beside it.
    pub fn icon_size(&self) -> f32 {
        (self.line_height - 2.0 * self.padding).round().max(8.0)
    }

    /// The top of the one line of text, centred in the bar.
    pub fn line_top(&self) -> f32 {
        ((self.height - self.line_height) / 2.0).round()
    }

    /// The top of every icon, centred in the bar.
    pub fn icon_top(&self) -> f32 {
        ((self.height - self.icon_size()) / 2.0).round()
    }

    /// The top of the keys' line, which is twice as tall, centred in the bar.
    pub fn keys_line_top(&self) -> f32 {
        ((self.height - 2.0 * self.line_height) / 2.0).round()
    }

    /// Where the keys go: from one em in to the middle, as `(x, width)`.
    pub fn keys_area(&self) -> (f32, f32) {
        (self.em, (self.width / 2.0 - self.em).max(0.0).round())
    }
}

/// One status item, as far as the layout cares: an icon, and the width of the
/// amount after it (0 for none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemSize {
    pub text_width: f32,
}

/// Where one item landed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub icon_x: f32,
    /// Where its amount starts, when it has one.
    pub text_x: Option<f32>,
}

/// Where everything landed.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub clock_x: f32,
    /// One per item that fits, in the order given (nearest the clock first).
    /// Items past the last one here did not fit.
    pub items: Vec<Placed>,
}

/// Lay out a clock `clock_width` wide and `items`, nearest the clock first.
/// With `keys`, the left half is the keys' and the items stop one em past
/// the middle.
pub fn layout(m: &Metrics, clock_width: f32, items: &[ItemSize], keys: bool) -> Layout {
    let left_limit = if keys {
        (m.width / 2.0).round() + m.em
    } else {
        m.em
    };
    let icon = m.icon_size();
    let gap = (m.em / 3.0).round();
    let spacing = (m.em * 0.8).round();
    let clock_x = (m.width - m.em - clock_width).round().max(0.0);
    let mut right = clock_x - m.em;
    let mut placed = Vec::new();
    for item in items {
        let text = if item.text_width > 0.0 {
            gap + item.text_width
        } else {
            0.0
        };
        let left = (right - icon - text).round();
        if left < left_limit {
            break;
        }
        placed.push(Placed {
            icon_x: left,
            text_x: (item.text_width > 0.0).then_some(left + icon + gap),
        });
        right = left - spacing;
    }
    Layout {
        clock_x,
        items: placed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(width: f32) -> Metrics {
        Metrics {
            width,
            // 1.7 lines of 36, rounded up as the bar asks for it.
            height: 62.0,
            line_height: 36.0,
            padding: 4.0,
            em: 15.0,
        }
    }

    #[test]
    fn the_line_and_the_icons_are_centred_in_the_bar() {
        let m = metrics(1000.0);
        assert_eq!(m.icon_size(), 28.0);
        assert_eq!(m.line_top(), 13.0);
        assert_eq!(m.icon_top(), 17.0);
        // As far above the line as below it.
        assert_eq!(m.line_top() * 2.0 + m.line_height, m.height);
        assert_eq!(m.icon_top() * 2.0 + m.icon_size(), m.height);
    }

    #[test]
    fn the_clock_hangs_one_em_from_the_right_edge() {
        let l = layout(&metrics(1000.0), 200.0, &[], false);
        assert_eq!(l.clock_x, 1000.0 - 15.0 - 200.0);
        assert!(l.items.is_empty());
    }

    #[test]
    fn items_go_leftwards_from_the_clock_with_amounts_after_their_icons() {
        let m = metrics(1000.0);
        let l = layout(
            &m,
            200.0,
            &[ItemSize { text_width: 30.0 }, ItemSize { text_width: 0.0 }],
            false,
        );
        let first = l.items[0];
        let second = l.items[1];
        let text_x = first.text_x.unwrap();
        // The amount ends one em before the clock.
        assert_eq!(text_x + 30.0, l.clock_x - 15.0);
        assert!(text_x > first.icon_x + m.icon_size());
        // The next item ends before the first begins, with room between.
        assert!(second.icon_x + m.icon_size() < first.icon_x);
        assert_eq!(second.text_x, None);
    }

    #[test]
    fn what_does_not_fit_is_left_out_farthest_first() {
        let m = metrics(300.0);
        let items = vec![ItemSize { text_width: 0.0 }; 20];
        let l = layout(&m, 150.0, &items, false);
        assert!(!l.items.is_empty());
        assert!(l.items.len() < 20);
        assert!(l.items.iter().all(|p| p.icon_x >= m.em));
    }

    #[test]
    fn a_narrow_bar_still_shows_the_clock() {
        let l = layout(
            &metrics(100.0),
            400.0,
            &[ItemSize { text_width: 0.0 }],
            false,
        );
        assert_eq!(l.clock_x, 0.0);
        assert!(l.items.is_empty());
    }

    #[test]
    fn the_keys_line_is_twice_as_tall_with_the_same_margins() {
        // 2.7 lines of 36, rounded up as the bar asks for it.
        let m = Metrics {
            height: 98.0,
            ..metrics(1000.0)
        };
        let normal = metrics(1000.0);
        assert_eq!(m.keys_line_top(), 13.0);
        assert_eq!(m.keys_line_top(), normal.line_top());
        assert_eq!(m.keys_line_top() * 2.0 + 2.0 * m.line_height, m.height);
        // The clock and the icons stay centred.
        assert_eq!(m.line_top() * 2.0 + m.line_height, m.height);
    }

    #[test]
    fn the_keys_take_the_left_half_one_em_in() {
        let m = metrics(1000.0);
        assert_eq!(m.keys_area(), (15.0, 485.0));
        let (x, w) = m.keys_area();
        assert_eq!(x + w, 500.0);
        assert_eq!(metrics(20.0).keys_area(), (15.0, 0.0));
    }

    #[test]
    fn with_the_keys_shown_the_items_stop_at_the_middle() {
        let m = metrics(1000.0);
        let items = vec![ItemSize { text_width: 0.0 }; 20];
        let all = layout(&m, 150.0, &items, false);
        let half = layout(&m, 150.0, &items, true);
        assert!(half.items.len() < all.items.len());
        assert!(half.items.iter().all(|p| p.icon_x >= 500.0 + m.em));
        // The nearest are the same ones, in the same places.
        assert_eq!(half.items[..], all.items[..half.items.len()]);
        assert_eq!(half.clock_x, all.clock_x);
    }
}
