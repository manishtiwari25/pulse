//! PULSE: the HTTP API and SSE the console uses, with the embedded console (port of Go package internal/hub).
//!
//! One module per Go file (`internal/hub/<name>.go` -> `<name>.rs`); `web/hub/out` is embedded via rust-embed in server.rs.

#![allow(unused_imports)]

#[cfg(test)]
use std::alloc::{GlobalAlloc, Layout, System};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
struct CountingAllocator;

#[cfg(test)]
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[cfg(test)]
#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

#[cfg(test)]
pub(crate) fn allocated_bytes() -> usize {
    ALLOCATED_BYTES.load(Ordering::Relaxed)
}

pub mod additions;
pub mod board;
pub mod code_deliveries;
pub mod cred;
pub mod decisions;
pub mod doc;
pub mod evolution;
pub mod folders;
pub mod harness;
pub mod heldout;
pub mod httpx;
pub mod job_repair;
pub mod lifecycle;
pub mod privacy;
pub mod project_goal;
pub mod read;
pub mod register;
pub mod running;
pub mod server;
pub mod service;
pub mod store_views;

pub use additions::*;
pub use board::*;
pub use code_deliveries::*;
pub use cred::*;
pub use decisions::*;
pub use doc::*;
pub use evolution::*;
pub use folders::*;
pub use harness::*;
pub use heldout::*;
pub use job_repair::*;
pub use lifecycle::*;
pub use privacy::*;
pub use project_goal::*;
pub use read::*;
pub use register::*;
pub use running::*;
pub use server::*;
pub use service::*;
pub use store_views::*;

#[cfg(test)]
pub(crate) mod test_helpers;
#[cfg(test)]
pub(crate) mod test_helpers_f2;
#[cfg(test)]
pub(crate) mod test_helpers_f3;
