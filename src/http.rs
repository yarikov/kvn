use std::io::ErrorKind;
use std::time::{Duration, Instant};

pub fn retry_interrupted<T>(
    deadline: Instant,
    mut attempt: impl FnMut(Duration) -> Result<T, ureq::Error>,
) -> Result<T, ureq::Error> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(ureq::Error::Timeout(ureq::Timeout::Global));
        }
        match attempt(remaining) {
            Err(error) if is_interrupted(&error) => continue,
            result => return result,
        }
    }
}

fn is_interrupted(error: &ureq::Error) -> bool {
    matches!(error, ureq::Error::Io(io) if io.kind() == ErrorKind::Interrupted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interrupted() -> ureq::Error {
        ureq::Error::Io(std::io::Error::from(ErrorKind::Interrupted))
    }

    #[test]
    fn an_interrupted_attempt_is_retried_with_the_remaining_time() {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut budgets = Vec::new();

        let result = retry_interrupted(deadline, |remaining| {
            budgets.push(remaining);
            if budgets.len() < 3 {
                Err(interrupted())
            } else {
                Ok("answer")
            }
        });

        assert_eq!(result.unwrap(), "answer");
        assert_eq!(budgets.len(), 3);
        assert!(budgets.windows(2).all(|pair| pair[1] <= pair[0]));
        assert!(budgets[0] <= Duration::from_secs(60));
    }

    #[test]
    fn any_other_error_is_returned_without_a_retry() {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut attempts = 0;

        let result: Result<(), _> = retry_interrupted(deadline, |_| {
            attempts += 1;
            Err(ureq::Error::Io(std::io::Error::from(
                ErrorKind::ConnectionReset,
            )))
        });

        assert!(
            matches!(result, Err(ureq::Error::Io(io)) if io.kind() == ErrorKind::ConnectionReset)
        );
        assert_eq!(attempts, 1);
    }

    #[test]
    fn interruptions_past_the_deadline_end_in_a_global_timeout() {
        let deadline = Instant::now() + Duration::from_millis(50);

        let result: Result<(), _> = retry_interrupted(deadline, |_| Err(interrupted()));

        assert!(matches!(
            result,
            Err(ureq::Error::Timeout(ureq::Timeout::Global))
        ));
    }
}
