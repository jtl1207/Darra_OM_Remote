// 限频日志:同一调用点每个 interval 至多输出一条,并附带被抑制条数。
// 收编自原 hbb_common::log_throttle,调用面 = throttled_log! 宏。
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct LogThrottle {
    interval: Duration,
    state: Mutex<ThrottleState>,
}

struct ThrottleState {
    suppressed: u64,
    last: Option<Instant>,
}

impl LogThrottle {
    pub const fn new(interval: Duration) -> Self {
        Self {
            interval,
            state: Mutex::new(ThrottleState {
                suppressed: 0,
                last: None,
            }),
        }
    }

    /// 记一次发生。到点返回应报告的次数(含本次),未到点返回 None。
    pub fn due(&self) -> Option<u64> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.suppressed += 1;
        let now = Instant::now();
        let emit = match state.last {
            None => true,
            Some(last) => now.duration_since(last) >= self.interval,
        };
        if emit {
            let n = state.suppressed;
            state.suppressed = 0;
            state.last = Some(now);
            Some(n)
        } else {
            None
        }
    }
}

/// 用法: throttled_log!(Duration::from_secs(5), warn, "...");
#[macro_export]
macro_rules! throttled_log {
    ($interval:expr, $level:ident, $($arg:tt)+) => {{
        static THROTTLE: $crate::log_throttle::LogThrottle =
            $crate::log_throttle::LogThrottle::new($interval);
        if let Some(n) = THROTTLE.due() {
            if n > 1 {
                $crate::log::$level!("{} (x{})", format_args!($($arg)+), n);
            } else {
                $crate::log::$level!("{}", format_args!($($arg)+));
            }
        }
    }};
}
