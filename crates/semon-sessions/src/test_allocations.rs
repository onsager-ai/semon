//! Shared synchronous Rust allocation evidence; excludes SQLite C and stacks.
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    static TRACK_RUST_ALLOCATIONS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static RUST_ALLOCATION_TOTALS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    static RUST_LIVE_BYTES: Cell<usize> = const { Cell::new(0) };
    static RUST_PEAK_BYTES: Cell<usize> = const { Cell::new(0) };
    static RUST_BASELINE_BYTES: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
struct TestCountingAllocator;

#[cfg(test)]
unsafe impl std::alloc::GlobalAlloc for TestCountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let pointer = unsafe {
            <std::alloc::System as std::alloc::GlobalAlloc>::alloc(&std::alloc::System, layout)
        };
        if !pointer.is_null() {
            record_test_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        release_test_allocation(layout.size());
        unsafe {
            <std::alloc::System as std::alloc::GlobalAlloc>::dealloc(
                &std::alloc::System,
                pointer,
                layout,
            )
        };
    }

    unsafe fn realloc(
        &self,
        pointer: *mut u8,
        layout: std::alloc::Layout,
        new_size: usize,
    ) -> *mut u8 {
        let next = unsafe {
            <std::alloc::System as std::alloc::GlobalAlloc>::realloc(
                &std::alloc::System,
                pointer,
                layout,
                new_size,
            )
        };
        if !next.is_null() {
            release_test_allocation(layout.size());
            record_test_allocation(new_size);
        }
        next
    }
}

#[cfg(test)]
#[global_allocator]
static TEST_COUNTING_ALLOCATOR: TestCountingAllocator = TestCountingAllocator;

#[cfg(test)]
fn record_test_allocation(bytes: usize) {
    let _ = RUST_LIVE_BYTES.try_with(|live| {
        let current = live.get().saturating_add(bytes);
        live.set(current);
    });
    if TRACK_RUST_ALLOCATIONS
        .try_with(std::cell::Cell::get)
        .unwrap_or(false)
    {
        let current = RUST_LIVE_BYTES.try_with(Cell::get).unwrap_or(0);
        let _ = RUST_PEAK_BYTES.try_with(|peak| peak.set(peak.get().max(current)));
        let _ = RUST_ALLOCATION_TOTALS.try_with(|totals| {
            let (calls, total_bytes) = totals.get();
            totals.set((calls.saturating_add(1), total_bytes.saturating_add(bytes)));
        });
    }
}

#[cfg(test)]
fn release_test_allocation(bytes: usize) {
    let _ = RUST_LIVE_BYTES.try_with(|live| live.set(live.get().saturating_sub(bytes)));
}

// Operation-owned Rust heap high water on the synchronous test thread. This
// excludes SQLite's C allocator and stack, just like the cumulative counter.
#[cfg(test)]
pub(crate) fn peak_bytes() -> usize {
    RUST_PEAK_BYTES
        .with(Cell::get)
        .saturating_sub(RUST_BASELINE_BYTES.with(Cell::get))
}

#[cfg(test)]
struct TestAllocationGuard;

#[cfg(test)]
impl Drop for TestAllocationGuard {
    fn drop(&mut self) {
        let _ = TRACK_RUST_ALLOCATIONS.try_with(|tracking| tracking.set(false));
    }
}

/// Count actual Rust allocations made synchronously by one tested operation.
/// SQLite's C allocator and external provider/proof I/O are intentionally outside
/// this measurement; SQLite instruction work is measured separately.
#[cfg(test)]
pub(crate) fn measure<T>(operation: impl FnOnce() -> T) -> (T, usize, usize) {
    RUST_ALLOCATION_TOTALS.with(|totals| totals.set((0, 0)));
    let baseline = RUST_LIVE_BYTES.with(Cell::get);
    RUST_BASELINE_BYTES.with(|value| value.set(baseline));
    RUST_PEAK_BYTES.with(|value| value.set(baseline));
    TRACK_RUST_ALLOCATIONS.with(|tracking| tracking.set(true));
    let guard = TestAllocationGuard;
    let value = operation();
    drop(guard);
    let (calls, bytes) = RUST_ALLOCATION_TOTALS.with(std::cell::Cell::get);
    (value, calls, bytes)
}
