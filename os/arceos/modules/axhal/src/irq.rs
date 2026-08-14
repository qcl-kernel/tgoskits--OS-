//! Interrupt management.

use core::sync::atomic::{AtomicUsize, Ordering};

use ax_cpu::trap::set_irq_handler;
#[cfg(feature = "smp")]
pub use ax_plat::irq::init_secondary_boot_irqs;
pub use ax_plat::irq::{
    AARCH64_GIC_DOMAIN, AcpiGsiController, AcpiGsiRoute, AcpiIrqPolarity, AcpiIrqTrigger,
    AutoEnable, BoxedIrqHandler, CPU_LOCAL_IRQ_DOMAIN, CpuId, CpuMask, HwIrq, IrqAffinity,
    IrqContext, IrqDomainId, IrqError, IrqExecution, IrqHandle, IrqId, IrqNumber, IrqOutcome,
    IrqRequest, IrqReturn, IrqScope, IrqSource, IrqStatus, IrqTrigger, LEGACY_IRQ_DOMAIN,
    LOONGARCH_EIOINTC_DOMAIN, LOONGARCH_PCH_PIC_DOMAIN, RISCV_PLIC_DOMAIN, ShareMode, TrapVector,
    X86_IOAPIC_DOMAIN, X86_LAPIC_DOMAIN, cpu_online, disable_irq, dispatch_irq, enable_irq,
    free_irq, handle, in_irq_context, init_boot_irqs, irq_status, is_cpu_online, legacy_irq,
    legacy_irq_raw, prepare_irq_context, request_irq, request_percpu_irq, request_shared_irq,
    resolve_irq_source, resolve_percpu_irq, run_on_cpu_sync, set_enable, set_run_on_cpu_sync,
    set_trigger, synchronize_irq, try_legacy_irq,
};
#[cfg(feature = "ipi")]
pub use ax_plat::irq::{IpiTarget, send_ipi};

/// Scheduler hook that runs after IRQ dispatch and preemption restoration, but
/// before the outer IRQ-save guard restores the interrupted IRQ state.
pub type IrqExitHandler = fn();

static IRQ_EXIT_HANDLER: AtomicUsize = AtomicUsize::new(0);

/// Installs the scheduler-owned IRQ-exit hook.
///
/// Reinstalling the same handler is accepted so scheduler initialization stays
/// idempotent in host tests. A conflicting handler is rejected.
#[doc(hidden)]
pub fn install_irq_exit_handler(handler: IrqExitHandler) -> bool {
    let address = handler as usize;
    match IRQ_EXIT_HANDLER.compare_exchange(0, address, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => true,
        Err(installed) => installed == address,
    }
}

fn run_irq_exit_handler() {
    let address = IRQ_EXIT_HANDLER.load(Ordering::Acquire);
    if address == 0 {
        return;
    }
    // SAFETY: install_irq_exit_handler only publishes function pointers with
    // the exact IrqExitHandler signature, and they have static lifetime.
    let handler = unsafe { core::mem::transmute::<usize, IrqExitHandler>(address) };
    handler();
}

/// Returns the platform IRQ id used for inter-processor interrupts.
#[cfg(feature = "ipi")]
pub fn ipi_irq() -> IrqId {
    ax_plat::irq::ipi_irq()
}

/// IRQ handler.
///
/// Normalizes both hardware-trap and hypervisor VM-exit callers to the same
/// local-IRQ-disabled entry contract. A hypervisor may restore the host IRQ
/// state before forwarding a deferred external interrupt.
///
/// # Warning
///
/// Make sure called in an interrupt context or hypervisor VM exit handler.
pub fn handle_irq(vector: usize) -> bool {
    with_irq_entry(
        || prepare_irq_context(TrapVector(vector)),
        || handle(TrapVector(vector)).is_some(),
    )
}

fn with_irq_entry<T>(prepare: impl FnOnce(), dispatch: impl FnOnce() -> T) -> T {
    with_observed_irq_entry(prepare, dispatch, run_irq_exit_handler)
}

/// Runs an IRQ action whose controller acknowledgement was already completed
/// by an architecture-specific exception entry.
///
/// Hypervisor vectors can consume the interrupt-controller token before Rust
/// regains control. They must still use the common IRQ guard and exit hook so
/// scheduler work published by the action is observed before returning to an
/// idle context.
#[doc(hidden)]
pub fn with_acknowledged_irq_entry<T>(dispatch: impl FnOnce() -> T) -> T {
    with_irq_entry(|| {}, dispatch)
}

fn with_observed_irq_entry<T>(
    prepare: impl FnOnce(),
    dispatch: impl FnOnce() -> T,
    after_preempt_release: impl FnOnce(),
) -> T {
    // Keep IRQs disabled until the preemption guard has handed any pending
    // reschedule back to the IRQ-return path. Hardware traps already enter in
    // this state; IrqSave also covers deferred VM-exit dispatchers.
    let irq_guard = ax_sync::IrqSaveGuard::new();
    prepare();
    let preempt_guard = ax_sync::PreemptGuard::new();
    let result = dispatch();

    drop(preempt_guard); // rescheduling may occur when preemption is re-enabled.
    after_preempt_release();
    drop(irq_guard);
    result
}

/// Installs the default ArceOS IRQ dispatcher into `ax-cpu`'s runtime hook.
///
/// This is intended for runtimes that dispatch traps through
/// [`ax_cpu::trap::dispatch_irq`] instead of relying on the `#[irq_handler]`
/// link-time override path.
pub fn init_common_irq_handler() {
    let _ = set_irq_handler(handle_irq);
}

#[cfg(axtest)]
pub(crate) struct IrqEntryStateObservation {
    pub(crate) dispatch_irqs_enabled: bool,
    pub(crate) after_preempt_release_irqs_enabled: bool,
    pub(crate) return_irqs_enabled: bool,
}

#[cfg(axtest)]
pub(crate) fn observe_irq_entry_state_for_test() -> IrqEntryStateObservation {
    let mut after_preempt_release_irqs_enabled = false;
    let dispatch_irqs_enabled = with_observed_irq_entry(
        || {},
        crate::asm::irqs_enabled,
        || after_preempt_release_irqs_enabled = crate::asm::irqs_enabled(),
    );

    IrqEntryStateObservation {
        dispatch_irqs_enabled,
        after_preempt_release_irqs_enabled,
        return_irqs_enabled: crate::asm::irqs_enabled(),
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::{install_irq_exit_handler, with_acknowledged_irq_entry};

    static ACKNOWLEDGED_EXIT_COUNT: AtomicUsize = AtomicUsize::new(0);

    fn count_acknowledged_exit() {
        ACKNOWLEDGED_EXIT_COUNT.fetch_add(1, Ordering::AcqRel);
    }

    #[test]
    fn acknowledged_irq_entry_runs_the_common_exit_hook() {
        ACKNOWLEDGED_EXIT_COUNT.store(0, Ordering::Release);
        assert!(install_irq_exit_handler(count_acknowledged_exit));

        assert_eq!(with_acknowledged_irq_entry(|| 37), 37);
        assert_eq!(ACKNOWLEDGED_EXIT_COUNT.load(Ordering::Acquire), 1);
    }
}
