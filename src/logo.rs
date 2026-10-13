//! The brie mark for the terminal header, in braille: a briefcase with a wedge of brie on its floor.
//! Each braille character holds 2 by 4 dots, so the mark is 16 by 8 dots in 8 by 2 characters.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// The case, row by row. The wedge is the characters 1 to 6 of the bottom row (amber). The rest is the case.
const CASE: [&str; 2] = ["⣤⠤⠤⠯⠽⠤⠤⣤", "⣿⣠⣄⣶⣼⣯⣇⣿"];

/// The mark: two rows, 8 columns.
pub fn mark(t: &Theme) -> [Line<'static>; 2] {
    let row = |n: usize| {
        Line::from(
            CASE[n]
                .chars()
                .enumerate()
                .map(|(i, ch)| {
                    let wedge = n == 1 && (1..=6).contains(&i);
                    let color = if wedge { t.accent } else { t.text };
                    Span::styled(ch.to_string(), Style::new().fg(color))
                })
                .collect::<Vec<_>>(),
        )
    };
    [row(0), row(1)]
}
