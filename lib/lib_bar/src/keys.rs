//! The keys being pressed, as the bar shows them in its left half when the
//! user switched on "show key strokes": the newest at the right, a key pressed
//! again in a row counted (`a×3`), and all of them gone a moment after the
//! last one.
//!
//! The compositor names each press (`ToBar::Key`). This only keeps them.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long the keys stay after the last one.
pub const FADE: Duration = Duration::from_millis(2500);

/// More than any bar is wide enough for.
const MAX: usize = 64;

#[derive(Debug, Default)]
pub struct Keys {
    /// Oldest first, each with how many times it was pressed in a row.
    entries: VecDeque<(String, u32)>,
    last: Option<Instant>,
}

impl Keys {
    /// A key was pressed.
    pub fn push(&mut self, label: String, now: Instant) {
        self.last = Some(now);
        if let Some((last, count)) = self.entries.back_mut()
            && *last == label
        {
            *count += 1;
            return;
        }
        if self.entries.len() == MAX {
            self.entries.pop_front();
        }
        self.entries.push_back((label, 1));
    }

    /// Forget them once [`FADE`] has passed since the last key. True when
    /// that changed what is shown.
    pub fn expire(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) >= FADE)
        {
            return self.clear();
        }
        false
    }

    /// Forget them now. True when there were any.
    pub fn clear(&mut self) -> bool {
        self.last = None;
        let had = !self.entries.is_empty();
        self.entries.clear();
        had
    }

    /// What fits in `max_width`, as measured by `measure`: the newest keys,
    /// a space apart. The newest is always there, even when it is too wide.
    pub fn visible(&self, max_width: f32, measure: impl Fn(&str) -> f32) -> String {
        let mut shown = String::new();
        for (label, count) in self.entries.iter().rev() {
            let entry = if *count > 1 {
                format!("{label}\u{d7}{count}")
            } else {
                label.clone()
            };
            let next = if shown.is_empty() {
                entry
            } else {
                format!("{entry} {shown}")
            };
            if !shown.is_empty() && measure(&next) > max_width {
                break;
            }
            shown = next;
        }
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One pixel a character.
    fn chars(s: &str) -> f32 {
        s.chars().count() as f32
    }

    fn typed(labels: &[&str], now: Instant) -> Keys {
        let mut k = Keys::default();
        for l in labels {
            k.push((*l).to_owned(), now);
        }
        k
    }

    #[test]
    fn keys_show_in_order_and_a_repeat_is_counted() {
        let k = typed(&["a", "b", "b", "b", "Ctrl+C", "b"], Instant::now());
        assert_eq!(k.visible(100.0, chars), "a b×3 Ctrl+C b");
    }

    #[test]
    fn the_oldest_go_first_when_they_do_not_fit() {
        let k = typed(&["one", "two", "three"], Instant::now());
        assert_eq!(k.visible(9.0, chars), "two three");
        assert_eq!(k.visible(8.0, chars), "three");
        // The newest stays, however narrow.
        assert_eq!(k.visible(1.0, chars), "three");
        assert_eq!(Keys::default().visible(100.0, chars), "");
    }

    #[test]
    fn they_fade_a_while_after_the_last_key() {
        let t = Instant::now();
        let mut k = typed(&["a"], t);
        assert!(!k.expire(t + FADE / 2));
        k.push("b".into(), t + FADE / 2);
        // Counted from the last key, not the first.
        assert!(!k.expire(t + FADE));
        assert!(k.expire(t + FADE / 2 + FADE));
        assert_eq!(k.visible(100.0, chars), "");
        assert!(!k.expire(t + FADE * 4));
    }

    #[test]
    fn a_repeat_after_the_fade_starts_again() {
        let t = Instant::now();
        let mut k = typed(&["a", "a"], t);
        assert!(k.expire(t + FADE));
        k.push("a".into(), t + FADE);
        assert_eq!(k.visible(100.0, chars), "a");
    }

    #[test]
    fn only_so_many_are_kept() {
        let mut k = Keys::default();
        let t = Instant::now();
        for i in 0..MAX + 10 {
            k.push(i.to_string(), t);
        }
        assert_eq!(k.entries.len(), MAX);
        assert_eq!(k.entries.front().unwrap().0, "10");
        assert!(k.clear());
        assert!(!k.clear());
    }
}
