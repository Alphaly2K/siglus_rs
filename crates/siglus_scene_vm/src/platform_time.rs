//! Cross-platform time helpers.
//!
//! wasm32-unknown-unknown does not support std::time::Instant::now() or
//! std::time::SystemTime::now(). Use this module anywhere runtime code needs
//! wall-clock or monotonic time.

pub use std::time::Duration;

#[cfg(all(
    all(target_arch = "wasm32", target_os = "unknown"),
    not(any(feature = "virtual-clock", feature = "art3m1s-host-clock"))
))]
pub use web_time::Instant;

#[cfg(all(
    not(all(target_arch = "wasm32", target_os = "unknown")),
    not(any(feature = "virtual-clock", feature = "art3m1s-host-clock"))
))]
pub use std::time::Instant;

#[cfg(any(feature = "virtual-clock", feature = "art3m1s-host-clock"))]
pub use controlled_clock::Instant;
#[cfg(feature = "virtual-clock")]
pub use controlled_clock::advance_virtual_clock;
#[cfg(feature = "art3m1s-host-clock")]
pub use controlled_clock::with_host_time;

#[cfg(any(feature = "virtual-clock", feature = "art3m1s-host-clock"))]
mod controlled_clock {
    use std::ops::{Add, AddAssign, Sub, SubAssign};
    use std::time::Duration;

    // Keep small subtractions from now() valid at the beginning of a session.
    const EPOCH_NS: u64 = 86_400_000_000_000;

    #[cfg(feature = "virtual-clock")]
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(feature = "virtual-clock")]
    static NOW_NS: AtomicU64 = AtomicU64::new(EPOCH_NS);

    #[cfg(feature = "virtual-clock")]
    pub fn advance_virtual_clock(by: Duration) {
        NOW_NS.fetch_add(by.as_nanos() as u64, Ordering::SeqCst);
    }

    #[cfg(feature = "art3m1s-host-clock")]
    thread_local! {
        static HOST_NS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    }

    /// Run VM callbacks with the monotonic time elapsed in one Host session.
    #[cfg(feature = "art3m1s-host-clock")]
    pub fn with_host_time<T>(elapsed: Duration, run: impl FnOnce() -> T) -> T {
        struct Restore(Option<u64>);
        impl Drop for Restore {
            fn drop(&mut self) {
                HOST_NS.set(self.0);
            }
        }
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        let _restore = Restore(HOST_NS.replace(Some(EPOCH_NS.saturating_add(nanos))));
        run()
    }

    #[cfg(feature = "virtual-clock")]
    fn default_now() -> u64 {
        NOW_NS.load(Ordering::SeqCst)
    }

    #[cfg(not(feature = "virtual-clock"))]
    fn default_now() -> u64 {
        #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
        use std::time::Instant as SystemInstant;
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        use web_time::Instant as SystemInstant;

        static START: std::sync::OnceLock<SystemInstant> = std::sync::OnceLock::new();
        let nanos = START.get_or_init(SystemInstant::now).elapsed().as_nanos();
        EPOCH_NS.saturating_add(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Instant(u64);

    impl Instant {
        pub fn now() -> Self {
            #[cfg(feature = "art3m1s-host-clock")]
            if let Some(nanos) = HOST_NS.get() {
                return Self(nanos);
            }
            Self(default_now())
        }

        pub fn elapsed(&self) -> Duration {
            Self::now().saturating_duration_since(*self)
        }

        pub fn duration_since(&self, earlier: Self) -> Duration {
            self.saturating_duration_since(earlier)
        }

        pub fn saturating_duration_since(&self, earlier: Self) -> Duration {
            Duration::from_nanos(self.0.saturating_sub(earlier.0))
        }

        pub fn checked_duration_since(&self, earlier: Self) -> Option<Duration> {
            self.0.checked_sub(earlier.0).map(Duration::from_nanos)
        }

        pub fn checked_add(&self, d: Duration) -> Option<Self> {
            self.0.checked_add(d.as_nanos() as u64).map(Self)
        }

        pub fn checked_sub(&self, d: Duration) -> Option<Self> {
            self.0.checked_sub(d.as_nanos() as u64).map(Self)
        }
    }

    impl Add<Duration> for Instant {
        type Output = Self;
        fn add(self, d: Duration) -> Self {
            self.checked_add(d).expect("virtual instant overflow")
        }
    }

    impl AddAssign<Duration> for Instant {
        fn add_assign(&mut self, d: Duration) {
            *self = *self + d;
        }
    }

    impl Sub<Duration> for Instant {
        type Output = Self;
        fn sub(self, d: Duration) -> Self {
            self.checked_sub(d).expect("virtual instant underflow")
        }
    }

    impl SubAssign<Duration> for Instant {
        fn sub_assign(&mut self, d: Duration) {
            *self = *self - d;
        }
    }

    impl Sub<Instant> for Instant {
        type Output = Duration;
        fn sub(self, other: Instant) -> Duration {
            self.saturating_duration_since(other)
        }
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub fn unix_time_millis() -> u128 {
    let ms = js_sys::Date::now();
    if ms.is_finite() && ms > 0.0 {
        ms as u128
    } else {
        0
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub fn unix_time_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub fn unix_time_secs() -> u64 {
    (unix_time_millis() / 1000) as u64
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LocalTimeFields {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// 0 = Sunday, 6 = Saturday, matching Windows SYSTEMTIME.wDayOfWeek.
    pub weekday_sunday0: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millisecond: u32,
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub fn local_time_fields() -> LocalTimeFields {
    let now = js_sys::Date::new_0();
    LocalTimeFields {
        year: now.get_full_year() as i32,
        month: now.get_month() + 1,
        day: now.get_date(),
        weekday_sunday0: now.get_day(),
        hour: now.get_hours(),
        minute: now.get_minutes(),
        second: now.get_seconds(),
        millisecond: now.get_milliseconds(),
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub fn local_time_fields() -> LocalTimeFields {
    use chrono::{Datelike, Timelike};

    let now = chrono::Local::now();
    LocalTimeFields {
        year: now.year(),
        month: now.month(),
        day: now.day(),
        weekday_sunday0: now.weekday().num_days_from_sunday(),
        hour: now.hour(),
        minute: now.minute(),
        second: now.second(),
        millisecond: now.timestamp_subsec_millis(),
    }
}

pub fn local_log_timestamp() -> String {
    let t = local_time_fields();
    format!(
        "[{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}]",
        year = t.year,
        month = t.month,
        day = t.day,
        hour = t.hour,
        minute = t.minute,
        second = t.second,
    )
}
