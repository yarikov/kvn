mod lists;

pub(crate) use lists::{context, list, select, source_rows};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ScrollPosition {
    pub start: usize,
    pub selected: usize,
}

pub(crate) fn scroll(
    rows: &[Option<usize>],
    visible: usize,
    mut position: ScrollPosition,
    delta: isize,
) -> ScrollPosition {
    if visible == 0 || rows.iter().all(Option::is_none) {
        return position;
    }
    let last_start = rows.len().saturating_sub(visible);
    position.start = position.start.min(last_start);
    let down = delta > 0;
    for _ in 0..delta.unsigned_abs().min(rows.len().saturating_add(visible)) {
        let next_start = position
            .start
            .saturating_add_signed(if down { 1 } else { -1 })
            .min(last_start);
        if next_start == position.start {
            let next = if down {
                rows.iter()
                    .flatten()
                    .copied()
                    .find(|index| *index > position.selected)
            } else {
                rows.iter()
                    .rev()
                    .flatten()
                    .copied()
                    .find(|index| *index < position.selected)
            };
            if let Some(next) = next {
                position.selected = next;
            }
        } else {
            position.start = next_start;
            let edge = if down {
                position.start
            } else {
                position.start + visible - 1
            };
            let first = rows.iter().position(|row| *row == Some(position.selected));
            let last = rows.iter().rposition(|row| *row == Some(position.selected));
            if down && last.is_none_or(|last| last < edge) {
                if let Some(selected) = rows[edge..(position.start + visible).min(rows.len())]
                    .iter()
                    .flatten()
                    .next()
                {
                    position.selected = *selected;
                }
            } else if !down
                && first.is_none_or(|first| first > edge)
                && let Some(selected) = rows[position.start..=edge.min(rows.len() - 1)]
                    .iter()
                    .rev()
                    .flatten()
                    .next()
            {
                position.selected = *selected;
            }
        }
    }
    position
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_moves_until_selection_reaches_edge_and_reverses_without_jumping() {
        let rows: Vec<_> = (0..40).map(Some).collect();
        let initial = ScrollPosition {
            start: 0,
            selected: 6,
        };
        let near_edge = scroll(&rows, 10, initial, 4);
        assert_eq!(
            near_edge,
            ScrollPosition {
                start: 4,
                selected: 6
            }
        );
        let pinned = scroll(&rows, 10, near_edge, 13);
        assert_eq!(
            pinned,
            ScrollPosition {
                start: 17,
                selected: 17
            }
        );
        assert_eq!(
            scroll(&rows, 10, pinned, -4),
            ScrollPosition {
                start: 13,
                selected: 17
            }
        );
        assert_eq!(
            scroll(&rows, 10, pinned, -17),
            ScrollPosition {
                start: 0,
                selected: 9
            }
        );
    }

    #[test]
    fn remaining_steps_move_selection_at_the_end_and_in_short_lists() {
        let rows: Vec<_> = (0..12).map(Some).collect();
        assert_eq!(
            scroll(
                &rows,
                10,
                ScrollPosition {
                    start: 0,
                    selected: 4
                },
                5
            ),
            ScrollPosition {
                start: 2,
                selected: 7
            }
        );
        assert_eq!(
            scroll(
                &rows[..3],
                10,
                ScrollPosition {
                    start: 0,
                    selected: 0
                },
                7
            ),
            ScrollPosition {
                start: 0,
                selected: 2
            }
        );
    }

    #[test]
    fn skips_nonselectable_rows_and_preserves_wrapped_records() {
        let rows = [
            Some(0),
            None,
            Some(1),
            None,
            Some(2),
            Some(2),
            Some(2),
            Some(3),
            Some(4),
            Some(5),
        ];
        assert_eq!(
            scroll(
                &rows,
                5,
                ScrollPosition {
                    start: 0,
                    selected: 1
                },
                3
            ),
            ScrollPosition {
                start: 3,
                selected: 2
            }
        );
        assert_eq!(
            scroll(
                &rows,
                5,
                ScrollPosition {
                    start: 3,
                    selected: 2
                },
                2
            ),
            ScrollPosition {
                start: 5,
                selected: 2
            }
        );
    }

    #[test]
    fn tiny_empty_and_resized_windows_are_bounded() {
        let rows = [Some(0), Some(1), Some(2)];
        assert_eq!(
            scroll(
                &rows,
                1,
                ScrollPosition {
                    start: 0,
                    selected: 0
                },
                1
            ),
            ScrollPosition {
                start: 1,
                selected: 1
            }
        );
        let position = ScrollPosition {
            start: 50,
            selected: 2,
        };
        assert_eq!(
            scroll(&rows, 10, position, -1),
            ScrollPosition {
                start: 0,
                selected: 1
            }
        );
        assert_eq!(scroll(&[], 10, position, 1), position);
        assert_eq!(scroll(&rows, 0, position, 1), position);
    }
}
