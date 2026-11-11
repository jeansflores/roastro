use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::{Client, Error, Reading};

/// Anything that can take a [`Reading`]. [`Client`] implements it; tests use fakes.
pub trait Reader: Send + Sync + 'static {
    /// Takes one reading.
    fn read(&self) -> impl Future<Output = Result<Reading, Error>> + Send;
}

impl Reader for Client {
    fn read(&self) -> impl Future<Output = Result<Reading, Error>> + Send {
        Client::read(self)
    }
}

impl<R: Reader> Reader for Arc<R> {
    fn read(&self) -> impl Future<Output = Result<Reading, Error>> + Send {
        R::read(self)
    }
}

/// The outcome of one poll attempt.
#[derive(Debug)]
pub struct Event {
    /// When the attempt started.
    pub at: Instant,
    /// The reading, or why there is none.
    pub result: Result<Reading, Error>,
    /// Consecutive failed attempts, this one included; 0 after a success.
    pub failures: u32,
    /// How long the poller waits, from `at`, before the next attempt.
    pub retry_in: Duration,
}

/// Reads at a fixed interval and backs off while reads fail.
#[derive(Debug, Clone, Copy)]
pub struct Poller {
    /// Between successful reads.
    pub interval: Duration,
    /// First wait after a failure.
    pub base_backoff: Duration,
    /// Cap on the wait.
    pub max_backoff: Duration,
}

impl Default for Poller {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(10),
        }
    }
}

impl Poller {
    /// Starts polling `reader` on the current Tokio runtime and delivers every
    /// attempt on the returned channel. Polling stops when the receiver is dropped.
    pub fn run<R: Reader>(self, reader: R) -> mpsc::Receiver<Event> {
        let (tx, rx) = mpsc::channel(1);
        tokio::spawn(async move {
            let mut failures = 0;
            loop {
                let at = Instant::now();
                let result = reader.read().await;
                let retry_in = if let Err(err) = &result {
                    failures += 1;
                    if failures == 1 {
                        tracing::warn!(%err, "plc disconnected");
                    }
                    let wait = backoff(failures, self.base_backoff, self.max_backoff);
                    tracing::debug!(failures, retry_in = ?wait, %err, "poll failed");
                    wait
                } else {
                    if failures > 0 {
                        tracing::info!(after_failures = failures, "plc reconnected");
                        failures = 0;
                    }
                    self.interval
                };
                if tx
                    .send(Event {
                        at,
                        result,
                        failures,
                        retry_in,
                    })
                    .await
                    .is_err()
                {
                    return; // nobody is listening any more
                }
                tokio::select! {
                    () = tokio::time::sleep_until(at + retry_in) => {}
                    () = tx.closed() => return,
                }
            }
        });
        rx
    }
}

/// The wait after `failures` consecutive failures: base, 2·base, 4·base, …
/// capped at `limit`.
#[must_use]
pub fn backoff(failures: u32, base: Duration, limit: Duration) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let factor = 2_u32.saturating_pow(failures - 1);
    base.saturating_mul(factor).min(limit)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn backs_off_exponentially_up_to_the_cap() {
        let (base, limit) = (Duration::from_secs(1), Duration::from_secs(10));
        let cases = [(0, 0), (1, 1), (2, 2), (3, 4), (4, 8), (5, 10), (50, 10)];
        for (failures, secs) in cases {
            assert_eq!(
                backoff(failures, base, limit),
                Duration::from_secs(secs),
                "failures={failures}"
            );
        }
    }

    /// Fails `failures` times, then reads bt = 200.
    struct Scripted {
        failures: usize,
        calls: AtomicUsize,
    }

    impl Reader for Scripted {
        fn read(&self) -> impl Future<Output = Result<Reading, Error>> + Send {
            let result = if self.calls.fetch_add(1, Ordering::SeqCst) < self.failures {
                Err(Error::Timeout { op: "read".into() })
            } else {
                Ok(Reading {
                    values: [("bt".to_owned(), 200.0)].into(),
                    ..Reading::default()
                })
            };
            std::future::ready(result)
        }
    }

    fn scripted(failures: usize) -> Scripted {
        Scripted {
            failures,
            calls: AtomicUsize::new(0),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn backs_off_and_recovers() {
        let ms = Duration::from_millis;
        let poller = Poller {
            interval: ms(1),
            base_backoff: ms(1),
            max_backoff: ms(3),
        };
        let mut events = poller.run(scripted(3));

        // (failures, retry_in, ok)
        let want = [
            (1, ms(1), false),
            (2, ms(2), false),
            (3, ms(3), false),
            (0, ms(1), true),
        ];
        for (i, (failures, retry_in, ok)) in want.into_iter().enumerate() {
            let ev = events.recv().await.expect("channel closed early");
            assert_eq!(
                (ev.failures, ev.retry_in, ev.result.is_ok()),
                (failures, retry_in, ok),
                "event {i}"
            );
        }
        let ev = events.recv().await.expect("event");
        let bt = ev.result.expect("reading").values["bt"];
        assert!((bt - 200.0).abs() < f64::EPSILON, "bt = {bt}");
    }

    #[tokio::test(start_paused = true)]
    async fn stops_when_the_receiver_is_dropped() {
        let reader = Arc::new(scripted(0));
        let poller = Poller {
            interval: Duration::from_secs(3600),
            ..Poller::default()
        };
        let mut events = poller.run(reader.clone());
        events.recv().await.expect("first event");
        drop(events);

        // The task held the only other reference; once it ends, ours is the last.
        while Arc::strong_count(&reader) > 1 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            reader.calls.load(Ordering::SeqCst),
            1,
            "polled again after the receiver was dropped"
        );
    }
}
