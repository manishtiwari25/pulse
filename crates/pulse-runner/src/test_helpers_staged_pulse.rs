use parking_lot::Mutex;
use std::sync::LazyLock;

#[allow(dead_code, reason = "shared test helper; not every one has a caller")]
static STAGED_LOCAL_PULSE: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

#[allow(dead_code, reason = "shared test helper; not every one has a caller")]
fn staged_local_pulse() -> String {
    STAGED_LOCAL_PULSE.lock().clone()
}

#[allow(dead_code, reason = "shared test helper; not every one has a caller")]
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

#[allow(dead_code, reason = "shared test helper; not every one has a caller")]
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
