#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::Result;
use std::sync::{LazyLock, RwLock};

pub(crate) const MIN_TMP_SPACE: u64 = 512 << 20;
/// Go: func workerTmpDir(env []string) string
pub(crate) fn worker_tmp_dir(env: &[String]) -> String {
    env.iter()
        .rev()
        .find_map(|kv| kv.strip_prefix("TMPDIR=").map(ToOwned::to_owned))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::temp_dir().display().to_string())
}
/// Go: var freeSpace = func(dir string) (uint64, error)
pub(crate) fn free_space(dir: &str) -> Result<u64> {
    let st = nix::sys::statvfs::statvfs(std::path::Path::new(dir))?;
    Ok(u64::from(st.blocks_available()) * st.fragment_size())
}
/// Go: func preflightTmpSpace(dir string, min uint64) error
pub(crate) fn preflight_tmp_space(dir: &str, min: u64) -> Result<()> {
    let free = free_space(dir)?;
    if free < min {
        return Err(crate::util::fail(1, format!("setup-error: worker TMPDIR {dir} has {} free, below the {} the worker needs; the worker was not started so no work is lost", human_bytes(free), human_bytes(min))).into());
    }
    Ok(())
}
/// Go: func humanBytes(n uint64) string
pub(crate) fn human_bytes(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{:.1} GiB", n as f64 / (1u64 << 30) as f64)
    } else if n >= 1 << 20 {
        format!("{:.1} MiB", n as f64 / (1u64 << 20) as f64)
    } else {
        format!("{n} B")
    }
}
