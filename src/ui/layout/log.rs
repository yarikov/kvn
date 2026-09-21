pub(crate) mod navigation;

use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::app::model::Model;
use crate::ui::layout::text::visual_width;

use navigation::{LogNavigation, LogSelection, LogViewport};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LogDisplayRow {
    text: String,
    hard_break_after: bool,
    error: bool,
    log_index: usize,
    /// Character offset of this wrapped row in the complete formatted line.
    source_offset: usize,
    level: LogLevel,
    structured: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Other,
}

impl LogLevel {
    fn parse(value: &str) -> Self {
        match value {
            "TRACE" => Self::Trace,
            "DEBUG" => Self::Debug,
            "INFO" => Self::Info,
            "WARN" | "WARNING" => Self::Warn,
            "ERROR" | "FATAL" | "PANIC" => Self::Error,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FormattedLog {
    text: String,
    level: LogLevel,
    structured: bool,
}

/// Convert the tailer's `[source] HH:MM:SS LEVEL ...` representation into the
/// compact presentation used by the log pane. The connection context emitted
/// by sing-box (`[id duration]`) is useful in raw logs but too noisy for the TUI.
pub(super) fn format_log_for_display(line: &str) -> FormattedLog {
    let Some(close_source) = line.strip_prefix('[').and_then(|rest| rest.find(']')) else {
        return FormattedLog {
            text: line.to_string(),
            level: LogLevel::Other,
            structured: false,
        };
    };
    let source_end = close_source + 2;
    let source = &line[1..=close_source];
    let remainder = line[source_end..].trim_start();
    let Some((time, after_time)) = remainder.split_once(' ') else {
        return FormattedLog {
            text: line.to_string(),
            level: LogLevel::Other,
            structured: false,
        };
    };
    if time.len() != 8
        || time.as_bytes().get(2) != Some(&b':')
        || time.as_bytes().get(5) != Some(&b':')
    {
        return FormattedLog {
            text: line.to_string(),
            level: LogLevel::Other,
            structured: false,
        };
    }
    let Some((level_text, mut message)) = after_time.split_once(' ') else {
        return FormattedLog {
            text: line.to_string(),
            level: LogLevel::Other,
            structured: false,
        };
    };
    let level = LogLevel::parse(level_text);

    if source == "sb"
        && let Some(rest) = message.strip_prefix('[')
        && let Some(close) = rest.find(']')
        && is_singbox_connection_context(&rest[..close])
    {
        message = rest[close + 1..].trim_start();
    }

    let source = if source == "sb" { "sbx" } else { source };
    FormattedLog {
        text: format!("{time} [{source}] {level_text} {message}"),
        level,
        structured: true,
    }
}

pub(super) fn is_singbox_connection_context(value: &str) -> bool {
    let mut fields = value.split_whitespace();
    let Some(id) = fields.next() else {
        return false;
    };
    let Some(duration) = fields.next() else {
        return false;
    };

    fields.next().is_none()
        && id.chars().all(|character| character.is_ascii_digit())
        && is_go_duration(duration)
}

/// Recognize the representation produced by Go's `time.Duration.String`.
pub(super) fn is_go_duration(value: &str) -> bool {
    let mut remainder = value
        .strip_prefix('-')
        .or_else(|| value.strip_prefix('+'))
        .unwrap_or(value);
    let mut components = 0;

    while !remainder.is_empty() {
        let integer_len = remainder.bytes().take_while(u8::is_ascii_digit).count();
        if integer_len == 0 {
            return false;
        }
        remainder = &remainder[integer_len..];

        if let Some(fraction) = remainder.strip_prefix('.') {
            let fraction_len = fraction.bytes().take_while(u8::is_ascii_digit).count();
            if fraction_len == 0 {
                return false;
            }
            remainder = &fraction[fraction_len..];
        }

        let Some(unit) = ["ns", "us", "µs", "ms", "h", "m", "s"]
            .into_iter()
            .find(|unit| remainder.starts_with(unit))
        else {
            return false;
        };
        remainder = &remainder[unit.len()..];
        components += 1;
    }

    components > 0
}

pub(super) fn build_all_log_rows(model: &Model, width: usize) -> Vec<LogDisplayRow> {
    let mut rows = Vec::new();
    if width > 0 {
        for (log_index, line) in model.logs.iter().enumerate() {
            let formatted = format_log_for_display(line);
            let error = line.starts_with("[error]") || formatted.level == LogLevel::Error;
            let wrapped = wrap_log_line_with_offsets(&formatted.text, width);
            let last = wrapped.len().saturating_sub(1);
            rows.extend(
                wrapped
                    .into_iter()
                    .enumerate()
                    .map(|(index, (text, source_offset))| LogDisplayRow {
                        text,
                        hard_break_after: index == last,
                        error,
                        log_index,
                        source_offset: if formatted.structured {
                            source_offset
                        } else {
                            0
                        },
                        level: formatted.level,
                        structured: formatted.structured,
                    }),
            );
        }
    }
    rows
}

pub(super) fn build_log_viewport(
    model: &Model,
    area: Rect,
    navigation: Option<&LogNavigation>,
) -> LogViewport {
    let all_rows = build_all_log_rows(model, area.width as usize);
    let keep = area.height as usize;
    let auto_start = all_rows.len().saturating_sub(keep);
    let mut start = navigation
        .and_then(|nav| nav.cursor.map(|_| nav.scroll_top_log.unwrap_or(0)))
        .and_then(|top_log| all_rows.iter().position(|row| row.log_index == top_log))
        .unwrap_or(auto_start);

    if let Some(cursor) = navigation.and_then(LogNavigation::cursor)
        && let Some(first) = all_rows.iter().position(|row| row.log_index == cursor)
    {
        let last = all_rows
            .iter()
            .rposition(|row| row.log_index == cursor)
            .unwrap_or(first);
        if first < start {
            start = first;
        } else if last >= start.saturating_add(keep) {
            let candidate = last.saturating_add(1).saturating_sub(keep);
            // Prefer the first complete record boundary that still keeps
            // the cursor visible. This prevents a wrapped log from becoming
            // several keyboard navigation positions.
            start = (candidate..=first)
                .find(|&index| {
                    index == 0 || all_rows[index - 1].log_index != all_rows[index].log_index
                })
                .unwrap_or(first);
        }
    }

    start = start.min(all_rows.len().saturating_sub(keep));
    let rows = all_rows.into_iter().skip(start).take(keep).collect();
    LogViewport { area, rows }
}

pub(super) fn wrap_log_line_with_offsets(line: &str, width: usize) -> Vec<(String, usize)> {
    if line.is_empty() || width == 0 {
        return vec![(String::new(), 0)];
    }
    let mut rows = vec![(String::new(), 0)];
    let mut used = 0;
    for (char_index, character) in line.chars().enumerate() {
        let wrapped_row = rows.len() > 1;
        if wrapped_row
            && rows.last().is_some_and(|(text, _)| text.is_empty())
            && character.is_whitespace()
        {
            continue;
        }
        let char_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used > 0 && used + char_width > width {
            rows.push((String::new(), char_index));
            used = 0;
            if character.is_whitespace() {
                continue;
            }
        }
        let row = rows.last_mut().expect("one row exists");
        if row.0.is_empty() {
            row.1 = char_index;
        }
        row.0.push(character);
        used += char_width;
    }
    rows
}

pub(super) fn text_between_columns(text: &str, from: u16, to: u16) -> String {
    let mut result = String::new();
    let mut column = 0_u16;
    for character in text.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0) as u16;
        let end = column.saturating_add(width.saturating_sub(1));
        if end >= from && column <= to {
            result.push(character);
        }
        column = column.saturating_add(width);
    }
    result
}

pub(super) fn log_display_line(
    model: &Model,
    row: &LogDisplayRow,
    row_index: usize,
    content_width: usize,
    selection: Option<&LogSelection>,
    navigation: Option<&LogNavigation>,
) -> Line<'static> {
    let base = if row.error {
        model.theme.error()
    } else {
        model.theme.normal()
    };
    let row_selected = navigation.is_some_and(|nav| nav.contains(row.log_index));
    let mut column = 0_u16;
    let mut spans = Vec::with_capacity(row.text.chars().count() + 2);
    spans.push(Span::styled(
        " ",
        if row_selected {
            model.theme.selected()
        } else {
            base
        },
    ));
    spans.extend(row.text.chars().enumerate().map(|(index, character)| {
        let absolute_index = row.source_offset + index;
        let width = UnicodeWidthChar::width(character).unwrap_or(0) as u16;
        let selected = row_selected
            || selection.is_some_and(|selection| selection.contains(row_index, column, width));
        column = column.saturating_add(width);
        let style = if row_selected || selected {
            model.theme.selected()
        } else if row.structured && absolute_index < 8 {
            model.theme.normal()
        } else if row.structured && (9..14).contains(&absolute_index) {
            if row.text.as_bytes().get(10..13) == Some(b"app") {
                model.theme.app_log_source()
            } else {
                model.theme.log_source()
            }
        } else if row.structured && (15..19).contains(&absolute_index) {
            match row.level {
                LogLevel::Warn => model.theme.warning(),
                LogLevel::Error => model.theme.error(),
                LogLevel::Trace | LogLevel::Debug => model.theme.muted(),
                LogLevel::Info | LogLevel::Other => {
                    model.theme.normal().add_modifier(Modifier::BOLD)
                }
            }
        } else {
            base
        };
        Span::styled(character.to_string(), style)
    }));
    let trailing = content_width
        .saturating_sub(visual_width(&row.text))
        .saturating_add(1);
    spans.push(Span::styled(
        " ".repeat(trailing),
        if row_selected {
            model.theme.selected()
        } else {
            base
        },
    ));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_log_for_display_compacts_singbox_context_and_reorders_fields() {
        let formatted = format_log_for_display(
            "[sb] 09:25:39 INFO [2083199607 81ms] dns: exchanged OPT OPT PSEUDOSECTION: EDNS: version 0 flags: udp: 1232",
        );

        assert_eq!(
            formatted.text,
            "09:25:39 [sbx] INFO dns: exchanged OPT OPT PSEUDOSECTION: EDNS: version 0 flags: udp: 1232"
        );
        assert_eq!(formatted.level, LogLevel::Info);
        assert!(formatted.structured);
    }

    #[test]
    fn format_log_for_display_compacts_context_for_every_singbox_level() {
        for (level, duration, expected_level) in [
            ("TRACE", "900ns", LogLevel::Trace),
            ("DEBUG", "12.5µs", LogLevel::Debug),
            ("INFO", "81ms", LogLevel::Info),
            ("WARN", "1.25s", LogLevel::Warn),
            ("WARNING", "1m2.5s", LogLevel::Warn),
            ("ERROR", "5.0s", LogLevel::Error),
            ("FATAL", "2m3s", LogLevel::Error),
            ("PANIC", "1h2m3s", LogLevel::Error),
        ] {
            let formatted = format_log_for_display(&format!(
                "[sb] 07:45:08 {level} [1541259397 {duration}] dns: exchange failed"
            ));

            assert_eq!(
                formatted.text,
                format!("07:45:08 [sbx] {level} dns: exchange failed")
            );
            assert_eq!(formatted.level, expected_level);
            assert!(formatted.structured);
        }
    }

    #[test]
    fn format_log_for_display_preserves_non_context_brackets() {
        for (line, expected) in [
            (
                "[sb] 07:45:08 ERROR [router] failed",
                "07:45:08 [sbx] ERROR [router] failed",
            ),
            (
                "[sb] 07:45:08 ERROR [1541259397 recently] failed",
                "07:45:08 [sbx] ERROR [1541259397 recently] failed",
            ),
        ] {
            let formatted = format_log_for_display(line);
            assert_eq!(formatted.text, expected);
        }
    }

    #[test]
    fn format_log_for_display_preserves_unstructured_lines() {
        let formatted = format_log_for_display("[app] Connected to 🇳🇱 Netherlands");

        assert_eq!(formatted.text, "[app] Connected to 🇳🇱 Netherlands");
        assert_eq!(formatted.level, LogLevel::Other);
        assert!(!formatted.structured);
    }

    #[test]
    fn wrapped_log_rows_do_not_start_with_whitespace() {
        assert_eq!(
            wrap_log_line_with_offsets("abc def ghi", 3),
            vec![("abc".into(), 0), ("def".into(), 4), ("ghi".into(), 8)]
        );
    }
}
