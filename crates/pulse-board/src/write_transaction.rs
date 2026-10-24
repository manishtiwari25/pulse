use crate::jobs::{Job, JobInput, add_job_conn};
use crate::posts::{Post, PostInput, create_post_conn};
use anyhow::Result;

pub struct WriteTransaction<'a> {
    s: &'a pulse_store::Store,
    tx: &'a pulse_store::Tx,
}
pub fn with_write_transaction<F>(s: &pulse_store::Store, f: F) -> Result<()>
where
    F: FnOnce(&WriteTransaction<'_>) -> Result<()>,
{
    let tx = s.begin()?;
    f(&WriteTransaction { s, tx: &tx })?;
    tx.commit()
}
impl WriteTransaction<'_> {
    pub fn add_job(&self, input: JobInput) -> Result<(Job, bool)> {
        add_job_conn(self.s, self.tx, input)
    }
    pub fn create_post(&self, input: PostInput) -> Result<Post> {
        create_post_conn(self.s, self.tx, input)
    }
}
