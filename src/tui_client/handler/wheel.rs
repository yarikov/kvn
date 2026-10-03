use std::time::{Duration, Instant};

const WHEEL_STREAK_GAP: Duration = Duration::from_millis(50);
const WHEEL_STEPS: [usize; 9] = [1, 1, 2, 2, 3, 3, 5, 5, 7];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WheelDirection {
    Up,
    Down,
}

#[derive(Default)]
pub(super) struct WheelAccelerator {
    last: Option<(WheelDirection, Instant)>,
    streak: usize,
}

impl WheelAccelerator {
    pub(super) fn step(&mut self, direction: WheelDirection, now: Instant) -> usize {
        let continues_streak = self.last.is_some_and(|(previous, at)| {
            previous == direction && now.saturating_duration_since(at) <= WHEEL_STREAK_GAP
        });
        self.streak = if continues_streak { self.streak + 1 } else { 0 };
        self.last = Some((direction, now));
        WHEEL_STEPS[self.streak.min(WHEEL_STEPS.len() - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst(wheel: &mut WheelAccelerator, start: Instant, events: u32) -> Vec<usize> {
        (0..events)
            .map(|index| {
                wheel.step(
                    WheelDirection::Down,
                    start + Duration::from_millis(10) * index,
                )
            })
            .collect()
    }

    #[test]
    fn slow_events_move_one_row_each() {
        let mut wheel = WheelAccelerator::default();
        let start = Instant::now();
        let steps: Vec<_> = (0..5)
            .map(|index| {
                wheel.step(
                    WheelDirection::Down,
                    start + Duration::from_millis(200) * index,
                )
            })
            .collect();
        assert_eq!(steps, vec![1; 5]);
    }

    #[test]
    fn fast_burst_accelerates_up_to_the_cap() {
        let mut wheel = WheelAccelerator::default();
        let steps = burst(&mut wheel, Instant::now(), 40);
        assert_eq!(&steps[..11], &[1, 1, 2, 2, 3, 3, 5, 5, 7, 7, 7]);
        assert_eq!(steps.last(), WHEEL_STEPS.last());
    }

    #[test]
    fn reversing_or_pausing_resets_the_step() {
        let mut wheel = WheelAccelerator::default();
        let start = Instant::now();
        burst(&mut wheel, start, 10);
        let after_burst = start + Duration::from_millis(100);
        assert_eq!(wheel.step(WheelDirection::Up, after_burst), 1);

        burst(&mut wheel, start + Duration::from_secs(1), 10);
        assert_eq!(
            wheel.step(WheelDirection::Down, start + Duration::from_secs(3)),
            1
        );
    }
}
