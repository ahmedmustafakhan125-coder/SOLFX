//! A token bucket over RPC calls, shared by the poster and the keeper.

/// A token bucket over every RPC call one process makes.
///
/// Reducing the call count was necessary but not sufficient: six feeds starting at once still
/// burst well past a free tier's ceiling in the first instant of a pass, and the ceiling is on
/// the *rate*, not the total. Handing out evenly spaced slots turns the burst into a queue.
///
/// It is per **process**, which is the limit worth understanding. A poster throttled to 8/s and
/// an unthrottled keeper on one free-tier key still exceed it together, and the measurement
/// says so plainly: the poster alone ran 11 consecutive clean passes, while the same poster
/// beside a running keeper lost 491 feeds across 371 passes. Two processes sharing an endpoint
/// have to share its budget — set each one's ceiling so the sum fits.
///
/// This has to live above the `RpcClient`, not inside it. `solana-rpc-client`'s HTTP sender
/// reacts to a 429 by retrying five times and honouring `Retry-After` for up to 120 s each,
/// which against a 60-second staleness gate means one throttled call can cost the feed its
/// whole reason for existing. The point of the bucket is that the 429 never happens.
pub struct Throttle {
    /// When the next slot opens. Held under a mutex only long enough to claim one.
    next: tokio::sync::Mutex<std::time::Instant>,
    spacing: std::time::Duration,
}

impl Throttle {
    #[must_use]
    pub fn new(max_rps: u32) -> Self {
        // A zero would divide by zero and an unbounded rate is what we are here to prevent.
        let rps = u64::from(max_rps.max(1));
        Self {
            next: tokio::sync::Mutex::new(std::time::Instant::now()),
            spacing: std::time::Duration::from_nanos(1_000_000_000u64.saturating_div(rps)),
        }
    }

    /// Claim the next slot and wait for it. Callers are served in arrival order.
    pub async fn acquire(&self) {
        let wait = {
            let mut next = self.next.lock().await;
            let now = std::time::Instant::now();
            let at = if *next > now { *next } else { now };
            *next = at.checked_add(self.spacing).unwrap_or(at);
            at.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}
