//! Test helpers ported from the Go test files owned by slice F1A (see files/RUNNER_TEST_WAVE.txt).
//! Other slices call these as `crate::test_helpers_f1a::<go_name_in_snake_case>`.

use parking_lot::Mutex;
use std::sync::LazyLock;

#[allow(dead_code, reason = "shared test helper consumed by other slices")]
static STAGED_LOCAL_PULSE: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

#[allow(dead_code, reason = "shared test helper consumed by other slices")]
fn staged_local_pulse() -> String {
    STAGED_LOCAL_PULSE.lock().clone()
}

#[allow(dead_code, reason = "shared test helper consumed by other slices")]
pub(crate) struct StageLocalPulseGuard {
    prev: String,
    prev_hook: Option<fn() -> String>,
}

impl Drop for StageLocalPulseGuard {
    fn drop(&mut self) {
        *STAGED_LOCAL_PULSE.lock() = self.prev.clone();
        *crate::run::STAGED_PULSE_PATH
            .write()
            .expect("staged pulse path") = self.prev_hook;
    }
}

/// Go: func stageLocalPulse(t *testing.T, bin string)
#[allow(dead_code, reason = "shared test helper consumed by other slices")]
pub(crate) fn stage_local_pulse(bin: &str) -> StageLocalPulseGuard {
    let prev_hook = *crate::run::STAGED_PULSE_PATH
        .read()
        .expect("staged pulse path");
    let prev = {
        let mut slot = STAGED_LOCAL_PULSE.lock();
        let prev = slot.clone();
        *slot = bin.to_string();
        prev
    };
    *crate::run::STAGED_PULSE_PATH
        .write()
        .expect("staged pulse path") = Some(staged_local_pulse);
    StageLocalPulseGuard { prev, prev_hook }
}
