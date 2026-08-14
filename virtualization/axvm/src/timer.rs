//! AxVM-owned CPU-bucketed VM timer wheels.

#[cfg(test)]
use std::sync::{Mutex, MutexGuard};
#[cfg(test)]
use std::vec::Vec;
use std::{
    boxed::Box,
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use ax_std::os::arceos::{guard::PreemptGuard, modules::ax_task::IrqNotify, sync::IrqSafeMutex};
use ax_timer_list::{TimeValue, TimerEvent, TimerList};

#[cfg(not(test))]
use crate::host::{HostTime, default_host, task};

static TOKEN: AtomicUsize = AtomicUsize::new(0);
const TIMER_WORKER_STACK_SIZE: usize = 0x20_000;
const NO_PUBLISHED_DEADLINE: u64 = 0;
const DEFERRED_WORKER_BIT: u64 = 1 << 63;
const PUBLISHED_DEADLINE_MASK: u64 = DEFERRED_WORKER_BIT - 1;

/// Determines where an expired VM timer event is drained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VmTimerDispatch {
    /// A task-context worker must drain the event.
    DeferredWorker,
    /// The architecture drains the event from its host-IRQ return path.
    SynchronousIrqReturn,
}

/// Lock-free publication of one CPU's earliest AxVM timer deadline.
///
/// The host timer IRQ reads this value while selecting the next shared
/// hardware comparator deadline. AxVM wheel mutations publish before asking
/// the host timer arbiter to move the comparator earlier.
pub(crate) struct PublishedTimerDeadline {
    deadline_nanos: AtomicU64,
}

impl PublishedTimerDeadline {
    const fn new() -> Self {
        Self {
            deadline_nanos: AtomicU64::new(NO_PUBLISHED_DEADLINE),
        }
    }

    pub(crate) fn deadline_nanos(&self) -> Option<u64> {
        match self.deadline_nanos.load(Ordering::Acquire) & PUBLISHED_DEADLINE_MASK {
            NO_PUBLISHED_DEADLINE => None,
            deadline => Some(deadline),
        }
    }

    fn publish(&self, deadline: Option<(TimeValue, VmTimerDispatch)>) {
        let published = deadline.map_or(NO_PUBLISHED_DEADLINE, |(deadline, dispatch)| {
            let deadline =
                (deadline.as_nanos().min(u128::from(PUBLISHED_DEADLINE_MASK)) as u64).max(1);
            match dispatch {
                VmTimerDispatch::DeferredWorker => deadline | DEFERRED_WORKER_BIT,
                VmTimerDispatch::SynchronousIrqReturn => deadline,
            }
        });
        self.deadline_nanos.store(published, Ordering::Release);
    }

    /// Removes an elapsed publication before the common IRQ path rearms the
    /// shared host comparator. The AxVM worker republishes the next wheel
    /// deadline after consuming all expired events.
    pub(crate) fn clear_if_elapsed(&self, now_nanos: u64) -> Option<VmTimerDispatch> {
        let mut observed = self.deadline_nanos.load(Ordering::Acquire);
        loop {
            let deadline = observed & PUBLISHED_DEADLINE_MASK;
            if deadline == NO_PUBLISHED_DEADLINE
                || deadline > now_nanos.min(PUBLISHED_DEADLINE_MASK)
            {
                return None;
            }
            match self.deadline_nanos.compare_exchange_weak(
                observed,
                NO_PUBLISHED_DEADLINE,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(if observed & DEFERRED_WORKER_BIT != 0 {
                        VmTimerDispatch::DeferredWorker
                    } else {
                        VmTimerDispatch::SynchronousIrqReturn
                    });
                }
                Err(current) => observed = current,
            }
        }
    }
}

/// Owner-aware handle for one AxVM timer-wheel entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VmTimerHandle {
    token: usize,
    owner_cpu: usize,
}

/// Action taken after an architectural timer deadline expires.
///
/// A running vCPU is already forced out by the host timer IRQ, so sending an
/// IPI to it would duplicate the exit. A vCPU blocked in WFI still needs an
/// explicit runtime notification.
#[cfg(any(target_arch = "aarch64", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum VmTimerWake {
    RunningVcpu,
    WaitingVcpu { vm_id: usize, vcpu_id: usize },
}

#[cfg(any(target_arch = "aarch64", test))]
impl VmTimerWake {
    pub(crate) const fn running_vcpu() -> Self {
        Self::RunningVcpu
    }

    pub(crate) const fn waiting_vcpu(vm_id: usize, vcpu_id: usize) -> Self {
        Self::WaitingVcpu { vm_id, vcpu_id }
    }

    pub(crate) const fn dispatch(self) -> VmTimerDispatch {
        match self {
            Self::RunningVcpu => VmTimerDispatch::SynchronousIrqReturn,
            Self::WaitingVcpu { .. } => VmTimerDispatch::DeferredWorker,
        }
    }

    #[cfg(target_arch = "aarch64")]
    pub(crate) fn notify(self) -> crate::AxVmResult {
        self.notify_with(crate::runtime::vcpus::notify_vcpu)
    }

    #[cfg(any(target_arch = "aarch64", test))]
    fn notify_with(
        self,
        notify_vcpu: impl FnOnce(usize, usize) -> crate::AxVmResult,
    ) -> crate::AxVmResult {
        match self {
            Self::RunningVcpu => Ok(()),
            Self::WaitingVcpu { vm_id, vcpu_id } => notify_vcpu(vm_id, vcpu_id),
        }
    }
}

struct VmTimerEvent {
    token: usize,
    callback: Box<dyn FnOnce(TimeValue) + Send + 'static>,
}

impl VmTimerEvent {
    fn new<F>(token: usize, callback: F) -> Self
    where
        F: FnOnce(TimeValue) + Send + 'static,
    {
        Self {
            token,
            callback: Box::new(callback),
        }
    }
}

impl TimerEvent for VmTimerEvent {
    fn callback(self, now: TimeValue) {
        trace!("handle VM timer event token {}", self.token);
        (self.callback)(now);
    }
}

#[derive(Clone, Copy)]
struct VmTimerRegistration {
    owner_cpu: usize,
    deadline: TimeValue,
    dispatch: VmTimerDispatch,
}

struct TimerWheels {
    wheels: BTreeMap<usize, TimerList<VmTimerEvent>>,
    registrations: BTreeMap<usize, VmTimerRegistration>,
    published_deadlines: BTreeMap<usize, Arc<PublishedTimerDeadline>>,
}

impl TimerWheels {
    fn new() -> Self {
        Self {
            wheels: BTreeMap::new(),
            registrations: BTreeMap::new(),
            published_deadlines: BTreeMap::new(),
        }
    }

    fn ensure_cpu(&mut self, cpu_id: usize) -> &mut TimerList<VmTimerEvent> {
        self.published_deadlines
            .entry(cpu_id)
            .or_insert_with(|| Arc::new(PublishedTimerDeadline::new()));
        self.wheels.entry(cpu_id).or_default()
    }

    fn published_deadline(&mut self, cpu_id: usize) -> Arc<PublishedTimerDeadline> {
        self.ensure_cpu(cpu_id);
        self.published_deadlines
            .get(&cpu_id)
            .expect("ensured AxVM timer CPU must have a published deadline")
            .clone()
    }

    fn publish_next_deadline(&self, cpu_id: usize, deadline: Option<TimeValue>) {
        let publication = deadline.map(|deadline| {
            let dispatch = if self.registrations.values().any(|registration| {
                registration.owner_cpu == cpu_id
                    && registration.deadline == deadline
                    && registration.dispatch == VmTimerDispatch::DeferredWorker
            }) {
                VmTimerDispatch::DeferredWorker
            } else {
                VmTimerDispatch::SynchronousIrqReturn
            };
            (deadline, dispatch)
        });
        self.published_deadlines
            .get(&cpu_id)
            .expect("AxVM timer wheel must publish only initialized CPUs")
            .publish(publication);
    }

    #[cfg(test)]
    fn register(
        &mut self,
        owner_cpu: usize,
        token: usize,
        deadline: TimeValue,
        event: VmTimerEvent,
    ) -> Option<TimeValue> {
        self.register_with_dispatch(
            owner_cpu,
            token,
            deadline,
            VmTimerDispatch::DeferredWorker,
            event,
        )
    }

    fn register_with_dispatch(
        &mut self,
        owner_cpu: usize,
        token: usize,
        deadline: TimeValue,
        dispatch: VmTimerDispatch,
        event: VmTimerEvent,
    ) -> Option<TimeValue> {
        self.registrations.insert(
            token,
            VmTimerRegistration {
                owner_cpu,
                deadline,
                dispatch,
            },
        );
        self.ensure_cpu(owner_cpu).set(deadline, event);
        let next_deadline = self.next_deadline(owner_cpu);
        self.publish_next_deadline(owner_cpu, next_deadline);
        next_deadline
    }

    fn handle(&self, token: usize) -> Option<VmTimerHandle> {
        self.registrations
            .get(&token)
            .map(|registration| VmTimerHandle {
                token,
                owner_cpu: registration.owner_cpu,
            })
    }

    fn cancel_handle(&mut self, handle: VmTimerHandle) -> Option<Option<TimeValue>> {
        if self
            .registrations
            .get(&handle.token)
            .map(|registration| registration.owner_cpu)
            != Some(handle.owner_cpu)
        {
            return None;
        }
        self.registrations.remove(&handle.token);
        let wheel = self.wheels.get_mut(&handle.owner_cpu)?;
        wheel.cancel(|event| event.token == handle.token);
        let next_deadline = wheel.next_deadline();
        self.publish_next_deadline(handle.owner_cpu, next_deadline);
        Some(next_deadline)
    }

    fn expire_one(
        &mut self,
        owner_cpu: usize,
        now: TimeValue,
    ) -> Option<(TimeValue, VmTimerEvent)> {
        let expired = self
            .wheels
            .get_mut(&owner_cpu)
            .and_then(|wheel| wheel.expire_one(now));
        if let Some((_, event)) = &expired {
            self.registrations.remove(&event.token);
        }
        self.publish_next_deadline(owner_cpu, self.next_deadline(owner_cpu));
        expired
    }

    fn next_deadline(&self, owner_cpu: usize) -> Option<TimeValue> {
        self.wheels
            .get(&owner_cpu)
            .and_then(TimerList::next_deadline)
    }
}

static TIMER_WHEELS: std::sync::OnceLock<IrqSafeMutex<TimerWheels>> = std::sync::OnceLock::new();

pub(crate) fn register_timer(
    deadline_ns: u64,
    callback: Box<dyn FnOnce(Duration) + Send + 'static>,
) -> usize {
    register_timer_handle(deadline_ns, callback).token
}

pub(crate) fn register_timer_handle(
    deadline_ns: u64,
    callback: Box<dyn FnOnce(Duration) + Send + 'static>,
) -> VmTimerHandle {
    register_timer_handle_with_dispatch(deadline_ns, VmTimerDispatch::DeferredWorker, callback)
}

pub(crate) fn register_timer_handle_with_dispatch(
    deadline_ns: u64,
    dispatch: VmTimerDispatch,
    callback: Box<dyn FnOnce(Duration) + Send + 'static>,
) -> VmTimerHandle {
    let token = TOKEN.fetch_add(1, Ordering::Relaxed);
    let (owner_cpu, next_deadline) = with_current_timer_wheels(|cpu_id, timer_wheels| {
        let next_deadline = timer_wheels.register_with_dispatch(
            cpu_id,
            token,
            TimeValue::from_nanos(deadline_ns),
            dispatch,
            VmTimerEvent::new(token, callback),
        );
        (cpu_id, next_deadline)
    });
    rearm_host_timer(next_deadline);
    VmTimerHandle { token, owner_cpu }
}

pub(crate) fn cancel_timer_handle(handle: VmTimerHandle) {
    let _guard = PreemptGuard::new();
    let current_cpu = current_cpu_id();
    let next_deadline = with_timer_wheels(|timer_wheels| timer_wheels.cancel_handle(handle));
    if let Some(next_deadline) = next_deadline {
        rearm_owner_host_timer(handle.owner_cpu, current_cpu, next_deadline);
    }
}

pub(crate) fn cancel_timer(token: usize) {
    let handle = {
        let _guard = PreemptGuard::new();
        with_timer_wheels(|timer_wheels| timer_wheels.handle(token))
    };
    if let Some(handle) = handle {
        cancel_timer_handle(handle);
    }
}

pub(crate) fn check_events() {
    loop {
        let now = current_host_time();
        let (expired, next_deadline) = with_current_timer_wheels(|cpu_id, timer_wheels| {
            let expired = timer_wheels.expire_one(cpu_id, now);
            let next_deadline = if expired.is_none() {
                timer_wheels.next_deadline(cpu_id)
            } else {
                None
            };
            (expired, next_deadline)
        });
        if let Some((deadline, event)) = expired {
            trace!("handle VM timer event scheduled at {deadline:#?}");
            event.callback(now);
        } else {
            rearm_host_timer(next_deadline);
            break;
        }
    }
}

#[cfg(not(test))]
fn current_host_time() -> TimeValue {
    default_host().monotonic_time()
}

#[cfg(test)]
fn current_host_time() -> TimeValue {
    TimeValue::from_nanos(TEST_NOW_NS.load(Ordering::Acquire))
}

fn rearm_owner_host_timer(owner_cpu: usize, current_cpu: usize, next_deadline: Option<TimeValue>) {
    if owner_cpu == current_cpu {
        rearm_host_timer(next_deadline);
    } else {
        rearm_remote_owner_host_timer(owner_cpu);
    }
}

fn rearm_current_host_timer_from_wheel() {
    let next_deadline =
        with_current_timer_wheels(|cpu_id, timer_wheels| timer_wheels.next_deadline(cpu_id));
    rearm_host_timer(next_deadline);
}

#[cfg(not(test))]
unsafe fn rearm_current_host_timer_from_wheel_thunk(_arg: *mut ()) {
    rearm_current_host_timer_from_wheel();
}

#[cfg(not(test))]
fn rearm_remote_owner_host_timer(owner_cpu: usize) {
    let result = task::run_on_cpu_sync(
        owner_cpu,
        rearm_current_host_timer_from_wheel_thunk,
        std::ptr::null_mut(),
    );
    if let Err(error) = result {
        warn!("failed to rearm AxVM timer on owner CPU {owner_cpu}: {error:?}; sending IPI");
        task::send_ipi(owner_cpu);
    }
}

#[cfg(not(test))]
fn rearm_host_timer(next_deadline: Option<TimeValue>) {
    if let Some(deadline) = next_deadline {
        default_host().request_timer_deadline(deadline.as_nanos() as u64);
    }
}

pub(crate) fn init_percpu() {
    info!("Initializing AxVM timer wheel...");
    let deadline_source =
        with_current_timer_wheels(|cpu_id, timer_wheels| timer_wheels.published_deadline(cpu_id));

    let cpu_id = current_cpu_id();
    let notify = Arc::new(IrqNotify::new());
    let worker_notify = notify.clone();
    let worker = crate::host::task::TaskInner::new(
        move || loop {
            worker_notify.wait();
            check_events();
        },
        std::format!("axvm-timer-{cpu_id}"),
        TIMER_WORKER_STACK_SIZE,
    );
    let cpu_bit = 1usize
        .checked_shl(cpu_id as u32)
        .expect("AxVM timer worker CPU ID must fit the host CPU mask");
    worker.set_cpumask(crate::host::task::cpu_mask_from_raw_bits(cpu_bit));
    crate::host::task::spawn_task(worker);
    crate::arch::register_timer_source(deadline_source, notify);
}

fn with_timer_wheels<R>(operation: impl FnOnce(&mut TimerWheels) -> R) -> R {
    let timer_wheels = TIMER_WHEELS.get_or_init(|| IrqSafeMutex::new(TimerWheels::new()));
    operation(&mut timer_wheels.lock())
}

fn with_current_timer_wheels<R>(operation: impl FnOnce(usize, &mut TimerWheels) -> R) -> R {
    let _guard = PreemptGuard::new();
    let cpu_id = current_cpu_id();
    with_timer_wheels(|timer_wheels| operation(cpu_id, timer_wheels))
}

#[cfg(not(test))]
fn current_cpu_id() -> usize {
    use crate::host::HostCpu;

    default_host().this_cpu_id()
}

#[cfg(test)]
static TEST_CURRENT_CPU: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static TEST_STATE_LOCK: Mutex<()> = Mutex::new(());
#[cfg(test)]
static TEST_REARMS: Mutex<Vec<(usize, Option<TimeValue>)>> = Mutex::new(Vec::new());
#[cfg(test)]
static TEST_REMOTE_REARMS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
#[cfg(test)]
static TEST_NOW_NS: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
fn current_cpu_id() -> usize {
    TEST_CURRENT_CPU.load(Ordering::Acquire)
}

#[cfg(test)]
fn lock_test_mutex<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().expect("AxVM timer test mutex poisoned")
}

#[cfg(test)]
pub(crate) fn lock_test_timer_state() -> MutexGuard<'static, ()> {
    lock_test_mutex(&TEST_STATE_LOCK)
}

#[cfg(test)]
pub(crate) fn reset_test_timer_state() {
    with_timer_wheels(|timer_wheels| {
        *timer_wheels = TimerWheels::new();
        timer_wheels.ensure_cpu(0);
    });
    lock_test_mutex(&TEST_REARMS).clear();
    lock_test_mutex(&TEST_REMOTE_REARMS).clear();
    TEST_CURRENT_CPU.store(0, Ordering::Release);
    TEST_NOW_NS.store(0, Ordering::Release);
}

#[cfg(test)]
pub(crate) fn set_test_timer_now_ns(now_ns: u64) {
    TEST_NOW_NS.store(now_ns, Ordering::Release);
}

#[cfg(test)]
pub(crate) struct TestTimerCallback(VmTimerEvent);

#[cfg(test)]
impl TestTimerCallback {
    pub(crate) fn run(self, now_ns: u64) {
        self.0.callback(TimeValue::from_nanos(now_ns));
    }
}

#[cfg(test)]
pub(crate) fn take_test_timer_callback(now_ns: u64) -> Option<TestTimerCallback> {
    let event = with_current_timer_wheels(|cpu_id, timer_wheels| {
        timer_wheels
            .expire_one(cpu_id, TimeValue::from_nanos(now_ns))
            .map(|(_, event)| event)
    });
    event.map(TestTimerCallback)
}

#[cfg(test)]
fn rearm_host_timer(next_deadline: Option<TimeValue>) {
    lock_test_mutex(&TEST_REARMS).push((current_cpu_id(), next_deadline));
}

#[cfg(test)]
fn rearm_remote_owner_host_timer(owner_cpu: usize) {
    lock_test_mutex(&TEST_REMOTE_REARMS).push(owner_cpu);
    let previous_cpu = TEST_CURRENT_CPU.swap(owner_cpu, Ordering::AcqRel);
    rearm_current_host_timer_from_wheel();
    TEST_CURRENT_CPU.store(previous_cpu, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_global_timer_state() {
        reset_test_timer_state();
    }

    fn set_current_cpu_for_test(cpu_id: usize) {
        TEST_CURRENT_CPU.store(cpu_id, Ordering::Release);
    }

    static TEST_CALLBACK_NOW_NS: AtomicU64 = AtomicU64::new(0);

    fn event(token: usize) -> VmTimerEvent {
        VmTimerEvent::new(token, |_| {})
    }

    #[test]
    fn host_timer_callback_path_dispatches_registered_event_once() {
        let _guard = lock_test_timer_state();
        reset_global_timer_state();
        TEST_CALLBACK_NOW_NS.store(0, Ordering::Release);

        set_current_cpu_for_test(0);
        TEST_NOW_NS.store(1_000_000, Ordering::Release);
        let token = register_timer(
            10_000_000,
            Box::new(|now| {
                TEST_CALLBACK_NOW_NS.store(now.as_nanos() as u64, Ordering::Release);
            }),
        );

        check_events();
        assert_eq!(TEST_CALLBACK_NOW_NS.load(Ordering::Acquire), 0);
        assert_eq!(
            lock_test_mutex(&TEST_REARMS).last().copied(),
            Some((0, Some(Duration::from_nanos(10_000_000))))
        );

        TEST_NOW_NS.store(10_000_000, Ordering::Release);
        check_events();
        assert_eq!(TEST_CALLBACK_NOW_NS.load(Ordering::Acquire), 10_000_000);
        assert_eq!(
            with_timer_wheels(|timer_wheels| timer_wheels.handle(token)),
            None
        );
    }

    #[test]
    fn architectural_deadline_notifies_only_a_vcpu_blocked_in_wfi() {
        let _guard = lock_test_timer_state();
        reset_global_timer_state();
        static NOTIFY_COUNT: AtomicUsize = AtomicUsize::new(0);
        static NOTIFIED_VM: AtomicUsize = AtomicUsize::new(usize::MAX);
        static NOTIFIED_VCPU: AtomicUsize = AtomicUsize::new(usize::MAX);

        NOTIFY_COUNT.store(0, Ordering::Release);
        NOTIFIED_VM.store(usize::MAX, Ordering::Release);
        NOTIFIED_VCPU.store(usize::MAX, Ordering::Release);

        let running = VmTimerWake::running_vcpu();
        register_timer_handle_with_dispatch(
            10_000_000,
            running.dispatch(),
            Box::new(move |_| {
                running
                    .notify_with(|_, _| {
                        NOTIFY_COUNT.fetch_add(1, Ordering::AcqRel);
                        Ok(())
                    })
                    .unwrap();
            }),
        );
        take_test_timer_callback(10_000_000)
            .expect("running-vCPU deadline should expire")
            .run(10_000_000);
        assert_eq!(NOTIFY_COUNT.load(Ordering::Acquire), 0);

        let waiting = VmTimerWake::waiting_vcpu(7, 3);
        register_timer_handle_with_dispatch(
            20_000_000,
            waiting.dispatch(),
            Box::new(move |_| {
                waiting
                    .notify_with(|vm_id, vcpu_id| {
                        NOTIFIED_VM.store(vm_id, Ordering::Release);
                        NOTIFIED_VCPU.store(vcpu_id, Ordering::Release);
                        NOTIFY_COUNT.fetch_add(1, Ordering::AcqRel);
                        Ok(())
                    })
                    .unwrap();
            }),
        );
        take_test_timer_callback(20_000_000)
            .expect("WFI deadline should expire")
            .run(20_000_000);

        assert_eq!(NOTIFY_COUNT.load(Ordering::Acquire), 1);
        assert_eq!(NOTIFIED_VM.load(Ordering::Acquire), 7);
        assert_eq!(NOTIFIED_VCPU.load(Ordering::Acquire), 3);
    }

    #[test]
    fn cancel_removes_event_from_original_cpu_wheel() {
        let mut timer_wheels = TimerWheels::new();
        let deadline = Duration::from_secs(60);

        assert_eq!(
            timer_wheels.register(0, 7, deadline, event(7)),
            Some(deadline)
        );
        assert_eq!(timer_wheels.next_deadline(0), Some(deadline));
        assert_eq!(timer_wheels.next_deadline(1), None);

        assert_eq!(
            timer_wheels.cancel_handle(VmTimerHandle {
                token: 7,
                owner_cpu: 0,
            }),
            Some(None)
        );
        assert_eq!(timer_wheels.next_deadline(0), None);
        assert_eq!(timer_wheels.handle(7), None);
    }

    #[test]
    fn cancel_rearms_to_remaining_owner_deadline() {
        let mut timer_wheels = TimerWheels::new();
        let early = Duration::from_secs(10);
        let late = Duration::from_secs(20);

        timer_wheels.register(1, 11, early, event(11));
        timer_wheels.register(1, 12, late, event(12));

        assert_eq!(
            timer_wheels.cancel_handle(VmTimerHandle {
                token: 11,
                owner_cpu: 1,
            }),
            Some(Some(late))
        );
        assert_eq!(timer_wheels.next_deadline(1), Some(late));
    }

    #[test]
    fn migration_reprogramming_deletes_stale_original_cpu_deadline() {
        let mut timer_wheels = TimerWheels::new();
        let stale_deadline = Duration::from_secs(60);
        let migrated_deadline = Duration::from_millis(10);

        assert_eq!(
            timer_wheels.register(0, 31, stale_deadline, event(31)),
            Some(stale_deadline)
        );
        assert_eq!(
            timer_wheels.cancel_handle(VmTimerHandle {
                token: 31,
                owner_cpu: 0,
            }),
            Some(None)
        );
        assert_eq!(
            timer_wheels.register(1, 32, migrated_deadline, event(32)),
            Some(migrated_deadline)
        );

        assert!(timer_wheels.expire_one(0, stale_deadline).is_none());
        let (deadline, migrated_event) = timer_wheels
            .expire_one(1, migrated_deadline)
            .expect("migrated timer event should expire on the new owner CPU");
        assert_eq!(deadline, migrated_deadline);
        assert_eq!(migrated_event.token, 32);
        assert_eq!(timer_wheels.handle(32), None);
    }

    #[test]
    fn expiring_event_forgets_owner_token() {
        let mut timer_wheels = TimerWheels::new();
        let deadline = Duration::from_millis(5);

        timer_wheels.register(2, 21, deadline, event(21));
        let expired = timer_wheels.expire_one(2, deadline);

        assert!(expired.is_some());
        assert_eq!(timer_wheels.handle(21), None);
    }

    #[test]
    fn published_deadline_tracks_registration_cancellation_and_expiry() {
        let mut timer_wheels = TimerWheels::new();
        let early = Duration::from_millis(5);
        let late = Duration::from_millis(10);
        let source = timer_wheels.published_deadline(0);

        timer_wheels.register(0, 51, early, event(51));
        timer_wheels.register(0, 52, late, event(52));
        assert_eq!(source.deadline_nanos(), Some(5_000_000));

        timer_wheels.cancel_handle(VmTimerHandle {
            token: 51,
            owner_cpu: 0,
        });
        assert_eq!(source.deadline_nanos(), Some(10_000_000));

        timer_wheels.expire_one(0, late);
        assert_eq!(source.deadline_nanos(), None);
    }

    #[test]
    fn timer_irq_clears_only_an_elapsed_publication() {
        let source = PublishedTimerDeadline::new();
        source.publish(Some((
            Duration::from_nanos(20),
            VmTimerDispatch::DeferredWorker,
        )));

        assert_eq!(source.clear_if_elapsed(19), None);
        assert_eq!(source.deadline_nanos(), Some(20));

        assert_eq!(
            source.clear_if_elapsed(20),
            Some(VmTimerDispatch::DeferredWorker)
        );
        assert_eq!(source.deadline_nanos(), None);

        assert_eq!(source.clear_if_elapsed(21), None);
    }

    #[test]
    fn running_and_wfi_deadlines_publish_distinct_irq_dispatch() {
        let mut timer_wheels = TimerWheels::new();
        let running_deadline = Duration::from_millis(5);
        let wfi_deadline = Duration::from_millis(10);
        let source = timer_wheels.published_deadline(0);

        timer_wheels.register_with_dispatch(
            0,
            61,
            running_deadline,
            VmTimerDispatch::SynchronousIrqReturn,
            event(61),
        );
        timer_wheels.register_with_dispatch(
            0,
            62,
            wfi_deadline,
            VmTimerDispatch::DeferredWorker,
            event(62),
        );

        assert_eq!(
            source.clear_if_elapsed(5_000_000),
            Some(VmTimerDispatch::SynchronousIrqReturn)
        );
        timer_wheels.expire_one(0, running_deadline);
        assert_eq!(source.deadline_nanos(), Some(10_000_000));
        assert_eq!(
            source.clear_if_elapsed(10_000_000),
            Some(VmTimerDispatch::DeferredWorker)
        );
    }

    #[test]
    fn a_deferred_event_wins_when_deadlines_are_equal() {
        let mut timer_wheels = TimerWheels::new();
        let deadline = Duration::from_millis(5);
        let source = timer_wheels.published_deadline(0);

        timer_wheels.register_with_dispatch(
            0,
            71,
            deadline,
            VmTimerDispatch::SynchronousIrqReturn,
            event(71),
        );
        timer_wheels.register_with_dispatch(
            0,
            72,
            deadline,
            VmTimerDispatch::DeferredWorker,
            event(72),
        );

        assert_eq!(
            source.clear_if_elapsed(5_000_000),
            Some(VmTimerDispatch::DeferredWorker)
        );
    }

    #[test]
    fn cancelling_a_wfi_deadline_republishes_synchronous_successor() {
        let mut timer_wheels = TimerWheels::new();
        let wfi_deadline = Duration::from_millis(5);
        let running_deadline = Duration::from_millis(10);
        let source = timer_wheels.published_deadline(0);

        timer_wheels.register_with_dispatch(
            0,
            81,
            wfi_deadline,
            VmTimerDispatch::DeferredWorker,
            event(81),
        );
        timer_wheels.register_with_dispatch(
            0,
            82,
            running_deadline,
            VmTimerDispatch::SynchronousIrqReturn,
            event(82),
        );
        timer_wheels.cancel_handle(VmTimerHandle {
            token: 81,
            owner_cpu: 0,
        });

        assert_eq!(source.deadline_nanos(), Some(10_000_000));
        assert_eq!(
            source.clear_if_elapsed(10_000_000),
            Some(VmTimerDispatch::SynchronousIrqReturn)
        );
    }

    #[test]
    fn remote_cancel_reprograms_owner_cpu_timer() {
        let _guard = lock_test_timer_state();
        reset_global_timer_state();

        set_current_cpu_for_test(0);
        let early_token = register_timer(10_000_000, Box::new(|_| {}));
        let late_token = register_timer(20_000_000, Box::new(|_| {}));
        assert_eq!(lock_test_mutex(&TEST_REARMS).len(), 2);

        lock_test_mutex(&TEST_REARMS).clear();
        set_current_cpu_for_test(1);
        cancel_timer(early_token);

        assert_eq!(lock_test_mutex(&TEST_REMOTE_REARMS).as_slice(), &[0]);
        assert_eq!(
            lock_test_mutex(&TEST_REARMS).as_slice(),
            &[(0, Some(Duration::from_nanos(20_000_000)))]
        );

        lock_test_mutex(&TEST_REARMS).clear();
        cancel_timer(late_token);

        assert_eq!(lock_test_mutex(&TEST_REMOTE_REARMS).as_slice(), &[0, 0]);
        assert_eq!(lock_test_mutex(&TEST_REARMS).as_slice(), &[(0, None)]);
    }

    #[test]
    fn owner_aware_handle_rejects_a_stale_cpu_identity() {
        let mut timer_wheels = TimerWheels::new();
        let deadline = Duration::from_secs(1);
        timer_wheels.register(2, 41, deadline, event(41));

        assert_eq!(
            timer_wheels.cancel_handle(VmTimerHandle {
                token: 41,
                owner_cpu: 1,
            }),
            None
        );
        assert_eq!(timer_wheels.next_deadline(2), Some(deadline));
        assert_eq!(
            timer_wheels.cancel_handle(VmTimerHandle {
                token: 41,
                owner_cpu: 2,
            }),
            Some(None)
        );
    }

    #[test]
    fn remote_handle_cancel_reprograms_the_recorded_owner_cpu() {
        let _guard = lock_test_timer_state();
        reset_global_timer_state();

        set_current_cpu_for_test(2);
        let handle = register_timer_handle(20_000_000, Box::new(|_| {}));
        lock_test_mutex(&TEST_REARMS).clear();

        set_current_cpu_for_test(0);
        cancel_timer_handle(handle);

        assert_eq!(lock_test_mutex(&TEST_REMOTE_REARMS).as_slice(), &[2]);
        assert_eq!(lock_test_mutex(&TEST_REARMS).as_slice(), &[(2, None)]);
    }
}
