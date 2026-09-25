/// A compact 3x3 block font for the eight pairing digits.
///
/// Each glyph is three rows of three columns; digits are separated by one
/// space and the two code halves by three, matching the grouped code.
const GLYPHS: [[&str; 3]; 10] = [
    ["█▀█", "█ █", "▀▀▀"],
    [" █ ", "██ ", " █ "],
    ["▀▀█", "█▀▀", "▀▀▀"],
    ["▀▀█", " ▀█", "▀▀▀"],
    ["█ █", "▀▀█", "  █"],
    ["█▀▀", "▀▀█", "▀▀▀"],
    ["█▀▀", "█▀█", "▀▀▀"],
    ["▀▀█", "  █", "  █"],
    ["█▀█", "█▀█", "▀▀▀"],
    ["█▀█", "▀▀█", "▀▀▀"],
];

/// Number of rows in the large digit font.
pub(super) const ROWS: usize = 3;
/// Width in columns of one large digit.
pub(super) const WIDTH: usize = 3;

/// Returns one font row for the given digit characters.
///
/// Non-digit characters are skipped. An empty input returns empty rows so the
/// caller can fall back to a placeholder.
pub(super) fn row(digits: &[char], row: usize) -> String {
    let mut text = String::with_capacity(digits.len() * (WIDTH + 1));
    let mut rendered = 0;
    for digit in digits {
        let Some(value) = digit.to_digit(10) else {
            continue;
        };
        if rendered > 0 {
            text.push_str(if rendered == 4 { "   " } else { " " });
        }
        text.push_str(GLYPHS[usize::try_from(value).unwrap_or(0)][row.min(ROWS - 1)]);
        rendered += 1;
    }
    text
}

/// Returns the total width of `count` grouped digits.
pub(super) const fn width(count: usize) -> usize {
    count * WIDTH + count.saturating_sub(1) + if count >= 5 { 2 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::{ROWS, row, width};

    #[test]
    fn every_digit_renders_three_tight_rows() {
        for value in 0..=9 {
            let digit = char::from_digit(value, 10).unwrap();
            for row_index in 0..ROWS {
                assert_eq!(row(&[digit], row_index).chars().count(), 3);
            }
        }
    }

    #[test]
    fn grouping_adds_a_wide_gap_between_halves() {
        let digits: Vec<char> = "12345678".chars().collect();
        assert_eq!(row(&digits, 0).chars().count(), width(8));
        assert_eq!(width(8), 33);
    }
}
