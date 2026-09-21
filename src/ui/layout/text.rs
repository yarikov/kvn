use unicode_width::UnicodeWidthChar;

/// Visual width of a string, counting Unicode characters according to their
/// display width.
pub(super) fn visual_width(s: &str) -> usize {
    s.chars().filter_map(UnicodeWidthChar::width).sum()
}

/// Truncate a string to fit within a visual width, appending "..." if truncated.
pub(super) fn truncate_to_visual_width(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }
    let total_width = visual_width(s);
    if total_width <= max_width {
        return s.to_string();
    }
    let mut result = String::new();
    let mut width = 0;
    let limit = max_width.saturating_sub(3);
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > limit {
            result.push_str("...");
            return result;
        }
        result.push(ch);
        width += cw;
    }
    result
}

/// Pad a string on the right with spaces up to the target visual width.
pub(super) fn pad_to_visual_width(s: &str, target: usize) -> String {
    let mut result = s.to_string();
    let width = visual_width(s);
    if width < target {
        result.push_str(&" ".repeat(target - width));
    }
    result
}

/// Truncate and pad a string to exactly the target visual width.
pub(super) fn fit_to_visual_width(s: &str, target: usize) -> String {
    pad_to_visual_width(&truncate_to_visual_width(s, target), target)
}

/// Left-align a string within a column that is centered in the target width.
pub(super) fn align_in_centered_column(s: &str, target: usize, column_width: usize) -> String {
    let text = truncate_to_visual_width(s, column_width);
    let left = target.saturating_sub(column_width) / 2;
    pad_to_visual_width(&format!("{}{}", " ".repeat(left), text), target)
}
