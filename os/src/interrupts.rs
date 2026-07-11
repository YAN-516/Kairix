//! Interrupt accounting exposed through `/proc/interrupts`.

use alloc::string::String;
use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};

/// Keep the table bounded so interrupt handlers never allocate or take a lock.
const MAX_INTERRUPT_NUMBER: usize = 256;

#[cfg(target_arch = "riscv64")]
const TIMER_INTERRUPT_NUMBER: usize = 5;

#[cfg(target_arch = "loongarch64")]
const TIMER_INTERRUPT_NUMBER: usize = 11;

static TIMER_INTERRUPT_COUNT: AtomicUsize = AtomicUsize::new(0);
static EXTERNAL_INTERRUPT_COUNTS: [AtomicUsize; MAX_INTERRUPT_NUMBER] =
    [const { AtomicUsize::new(0) }; MAX_INTERRUPT_NUMBER];

/// Account for one handled timer interrupt.
#[inline]
pub fn record_timer_interrupt() {
    TIMER_INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Account for one handled external interrupt with the supplied IRQ number.
///
/// Unknown interrupt numbers outside the representable table are deliberately
/// ignored: accounting must never make an otherwise handled interrupt fail.
#[inline]
pub fn record_external_interrupt(interrupt_number: usize) {
    if let Some(counter) = EXTERNAL_INTERRUPT_COUNTS.get(interrupt_number) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// Render the current counters in the format required by `/proc/interrupts`.
pub fn render() -> String {
    let mut output = String::new();

    for (interrupt_number, external_count) in EXTERNAL_INTERRUPT_COUNTS.iter().enumerate() {
        let count = if interrupt_number == TIMER_INTERRUPT_NUMBER {
            TIMER_INTERRUPT_COUNT.load(Ordering::Relaxed)
        } else {
            external_count.load(Ordering::Relaxed)
        };

        // Keep the timer line visible even before the first tick.  Besides
        // matching procfs' representation of registered IRQs, this guarantees
        // that a successful early read is not indistinguishable from EOF.
        if count != 0 || interrupt_number == TIMER_INTERRUPT_NUMBER {
            // Writing to a String cannot fail.
            writeln!(&mut output, "{}:        {}", interrupt_number, count).unwrap();
        }
    }

    output
}
