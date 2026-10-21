use anyhow::Result;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static WRITE_MU: OnceLock<Mutex<()>> = OnceLock::new();

pub struct WriteLock {
    _guard: std::sync::MutexGuard<'static, ()>,
    file: File,
}

pub fn lock_writes(s: &pulse_store::Store) -> Result<WriteLock> {
    let guard = WRITE_MU
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("record write lock poisoned");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(Path::new(&s.home).join("records.lock"))?;
    lock_file(&file)?;
    Ok(WriteLock {
        _guard: guard,
        file,
    })
}

#[cfg(unix)]
fn lock_file(file: &File) -> Result<()> {
    fs2::FileExt::lock_exclusive(file)?;
    Ok(())
}

#[cfg(not(unix))]
fn lock_file(_file: &File) -> Result<()> {
    anyhow::bail!("record writes require an OS advisory lock; this build supports macOS and Linux")
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = fs2::FileExt::unlock(&self.file);
        }
    }
}
