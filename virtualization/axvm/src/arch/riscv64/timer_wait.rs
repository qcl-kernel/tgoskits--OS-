//! RISC-V guest timer wakeups while an unbound vCPU waits after WFI.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use ax_std::os::arceos::sync::IrqSafeMutex;
use riscv_vcpu::RiscvTimerSnapshot;

use crate::timer::VmTimerHandle;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GuestTimerDeadline {
    Disabled,
    Expired,
    Future(u64),
}

pub(super) fn guest_timer_deadline_ns(
    snapshot: Option<RiscvTimerSnapshot>,
    now_ticks: u64,
    ticks_to_nanos: impl FnOnce(u64) -> u64,
) -> GuestTimerDeadline {
    let Some(deadline_ticks) = snapshot.and_then(RiscvTimerSnapshot::host_deadline_ticks) else {
        return GuestTimerDeadline::Disabled;
    };
    let remaining_ticks = deadline_ticks.wrapping_sub(now_ticks) as i64;
    if remaining_ticks <= 0 {
        return GuestTimerDeadline::Expired;
    }
    GuestTimerDeadline::Future(ticks_to_nanos(deadline_ticks))
}

pub(super) struct RiscvTimerWait {
    generation: AtomicU64,
    scheduled: IrqSafeMutex<Option<VmTimerHandle>>,
}

impl RiscvTimerWait {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            generation: AtomicU64::new(0),
            scheduled: IrqSafeMutex::new(None),
        })
    }

    pub(super) fn arm(self: &Arc<Self>, deadline_ns: u64, wake: impl FnOnce() + Send + 'static) {
        self.invalidate();
        let generation = self
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let wait = Arc::downgrade(self);
        let handle = crate::timer::register_timer_handle(
            deadline_ns,
            Box::new(move |_| {
                let Some(wait) = wait.upgrade() else {
                    return;
                };
                if wait
                    .generation
                    .compare_exchange(
                        generation,
                        generation.wrapping_add(1),
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_err()
                {
                    return;
                }
                wait.scheduled.lock().take();
                wake();
            }),
        );

        let (stale, previous) = {
            let mut scheduled = self.scheduled.lock();
            if self.generation.load(Ordering::Acquire) != generation {
                (true, None)
            } else {
                (false, scheduled.replace(handle))
            }
        };
        if stale {
            crate::timer::cancel_timer_handle(handle);
        } else if let Some(previous) = previous {
            crate::timer::cancel_timer_handle(previous);
        }
    }

    pub(super) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(handle) = self.scheduled.lock().take() {
            crate::timer::cancel_timer_handle(handle);
        }
    }
}

impl Drop for RiscvTimerWait {
    fn drop(&mut self) {
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn guest_deadline_uses_htimedelta_and_host_tick_conversion() {
        let snapshot = RiscvTimerSnapshot::new(1_250, 250, true);
        assert_eq!(
            guest_timer_deadline_ns(Some(snapshot), 900, |ticks| ticks * 100),
            GuestTimerDeadline::Future(100_000)
        );
    }

    #[test]
    fn expired_and_disabled_guest_timers_do_not_request_sleep_deadlines() {
        assert_eq!(
            guest_timer_deadline_ns(Some(RiscvTimerSnapshot::new(1_000, 0, true)), 1_000, |_| 0,),
            GuestTimerDeadline::Expired
        );
        assert_eq!(
            guest_timer_deadline_ns(
                Some(RiscvTimerSnapshot::new(u64::MAX, 0, true)),
                1_000,
                |_| 0,
            ),
            GuestTimerDeadline::Disabled
        );
        assert_eq!(
            guest_timer_deadline_ns(Some(RiscvTimerSnapshot::new(2_000, 0, false)), 1_000, |_| 0,),
            GuestTimerDeadline::Disabled
        );
    }

    #[test]
    fn future_guest_deadline_wakes_the_waiting_vcpu() {
        let _guard = crate::timer::lock_test_timer_state();
        crate::timer::reset_test_timer_state();
        let wake_count = Arc::new(AtomicUsize::new(0));
        let wait = RiscvTimerWait::new();
        let callback_count = wake_count.clone();

        wait.arm(200, move || {
            callback_count.fetch_add(1, Ordering::AcqRel);
        });
        crate::timer::set_test_timer_now_ns(200);
        crate::timer::check_events();

        assert_eq!(wake_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn reprogramming_invalidates_the_old_guest_timer_callback() {
        let _guard = crate::timer::lock_test_timer_state();
        crate::timer::reset_test_timer_state();
        let wake_count = Arc::new(AtomicUsize::new(0));
        let wait = RiscvTimerWait::new();
        let old_count = wake_count.clone();
        wait.arm(200, move || {
            old_count.fetch_add(100, Ordering::AcqRel);
        });
        let current_count = wake_count.clone();
        wait.arm(300, move || {
            current_count.fetch_add(1, Ordering::AcqRel);
        });

        crate::timer::set_test_timer_now_ns(200);
        crate::timer::check_events();
        assert_eq!(wake_count.load(Ordering::Acquire), 0);

        crate::timer::set_test_timer_now_ns(300);
        crate::timer::check_events();
        assert_eq!(wake_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn stop_or_reset_invalidates_an_already_dequeued_guest_timer_callback() {
        let _guard = crate::timer::lock_test_timer_state();
        crate::timer::reset_test_timer_state();
        let wake_count = Arc::new(AtomicUsize::new(0));
        let wait = RiscvTimerWait::new();
        let callback_count = wake_count.clone();
        wait.arm(200, move || {
            callback_count.fetch_add(1, Ordering::AcqRel);
        });

        let callback = crate::timer::take_test_timer_callback(200)
            .expect("guest timer callback should be ready for dispatch");
        wait.invalidate();
        callback.run(200);

        assert_eq!(wake_count.load(Ordering::Acquire), 0);
    }
}
