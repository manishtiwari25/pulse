use std::sync::{Mutex, MutexGuard, OnceLock};

static SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
