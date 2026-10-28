//! The hands-binary probe seam for the compute flow.
//!
//! `pulse compute request` runs here in pulse-tasks, but whether this host
//! can ship a linux/{arch} pulse binary to a new machine is a question about
//! the installed tooling (`PULSE_HANDS_BINARY`, a sibling `pulse-linux-{arch}`
//! file, or a cross-build toolchain) that only pulse-runner knows. The runner
//! registers [`set_hands_binary_probe`] from `compute::init()` — before any
//! command runs — so a request on a host that cannot produce the binary is
//! refused with the fix, and no provider call ever buys a machine that fails
//! the same check minutes later (CR-001/CR-002).

use std::sync::RwLock;

/// One line: where the linux/{arch} hands binary comes from, or what to
/// install. `None` means the binary can be shipped; `Some(line)` refuses the
/// compute request with the fix.
pub type HandsBinaryProbeFn = dyn Fn(&str) -> Option<String> + Send + Sync;

fn no_probe(_arch: &str) -> Option<String> {
    None
}

/// The registered probe. The default assumes the binary can be shipped so a
/// pulse-tasks caller without pulse-runner (never in practice) stays silent.
static HANDS_BINARY_PROBE: RwLock<Option<Box<HandsBinaryProbeFn>>> = RwLock::new(None);

/// Install the probe; returns the previous one so tests can restore it.
pub fn set_hands_binary_probe(
    f: Option<Box<HandsBinaryProbeFn>>,
) -> Option<Box<HandsBinaryProbeFn>> {
    std::mem::replace(
        &mut *HANDS_BINARY_PROBE
            .write()
            .expect("hands_binary_probe poisoned"),
        f,
    )
}

/// The line the probe would report for `arch`, or `None` when a hands binary
/// can be shipped. Cheap: never builds, never touches a provider.
pub fn hands_binary_probe_line(arch: &str) -> Option<String> {
    let guard = HANDS_BINARY_PROBE
        .read()
        .expect("hands_binary_probe poisoned");
    match guard.as_ref() {
        Some(f) => f(arch),
        None => no_probe(arch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_gate_round_trips() {
        let _serial = crate::test_helpers::serial();
        // No probe: silent.
        let old = set_hands_binary_probe(None);
        assert_eq!(hands_binary_probe_line("amd64"), None);
        // A registered probe refuses with its line, naming the arch.
        let _ = set_hands_binary_probe(Some(Box::new(|arch| {
            Some(format!(
                "hands binary linux/{arch}: none; install cargo-zigbuild"
            ))
        })));
        assert_eq!(
            hands_binary_probe_line("arm64").as_deref(),
            Some("hands binary linux/arm64: none; install cargo-zigbuild")
        );
        let _ = set_hands_binary_probe(old);
    }
}
