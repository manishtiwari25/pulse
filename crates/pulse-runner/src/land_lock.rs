#![allow(unused_variables, dead_code, unused_imports)]

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::{LazyLock, RwLock};
use std::time::Duration;

pub(crate) const INDEX_LOCK_MINUTES: i32 = crate::land::LAND_LOCK_MINUTES;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AbandonedLock {
    pub from: String,
    pub to: String,
    #[serde(skip)]
    pub age: std::time::Duration,
    pub size: i64,
}

pub(crate) static LOCK_HOLDERS: LazyLock<
    RwLock<Box<dyn Fn(&str) -> (Vec<i32>, bool) + Send + Sync>>,
> = LazyLock::new(|| RwLock::new(Box::new(|path| todo!("runner::land_lock::lock_holders"))));

pub(crate) fn set_lock_holders(
    f: Box<dyn Fn(&str) -> (Vec<i32>, bool) + Send + Sync>,
) -> Box<dyn Fn(&str) -> (Vec<i32>, bool) + Send + Sync> {
    std::mem::replace(
        &mut *LOCK_HOLDERS.write().expect("lock_holders poisoned"),
        f,
    )
}

/// Go: func indexLockPath(root string) string
pub(crate) fn index_lock_path(root: String) -> String {
    todo!("runner::land_lock::index_lock_path")
}

/// Go: func freeIndexLock(root string, now time.Time) (note string, moved *abandonedLock)
pub(crate) fn free_index_lock(
    root: String,
    now: chrono::DateTime<chrono::Utc>,
) -> (String, Option<Box<AbandonedLock>>) {
    todo!("runner::land_lock::free_index_lock")
}

/// Go: func lockedCheckout(stderr string) bool
pub(crate) fn locked_checkout(stderr: String) -> bool {
    todo!("runner::land_lock::locked_checkout")
}

impl AbandonedLock {
    /// Go: func (a abandonedLock) detail() map[string]any
    pub(crate) fn detail(&self) -> BTreeMap<String, serde_json::Value> {
        todo!("runner::land_lock::detail")
    }
}
