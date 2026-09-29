#![allow(unused_variables, dead_code, unused_imports)]
use crate::util::Row;
use anyhow::Result;
use std::collections::BTreeMap;
pub(crate) static PROVENANCE_TRAILERS: &[&str] = &["Pulse-Agent", "Pulse-Job", "Pulse-Executor"];
/// Go: func agentEmail(agent string) string
pub(crate) fn agent_email(agent: &str) -> String {
    format!("{agent}@pulse.local")
}
/// Go: func makeRunKey(path, agent, runID string) (string, error)
pub(crate) fn make_run_key(path: &str, agent: &str, run_id: &str) -> Result<String> {
    todo!("runner::keys::make_run_key")
}
/// Go: func removeRunKey(path string)
pub(crate) fn remove_run_key(path: &str) {
    let _ = std::fs::remove_file(path);
}
/// Go: func provenanceHook(agent, jobID, executor string) string
pub(crate) fn provenance_hook(agent: &str, job_id: &str, executor: &str) -> String {
    todo!("runner::keys::provenance_hook")
}
pub(crate) const PRE_COMMIT_HOOK_UNSET: &str = "unset GIT_INDEX_FILE GIT_PREFIX GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_AUTHOR_DATE GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL GIT_COMMITTER_DATE";
/// Go: func preCommitHookScript(verifyCmd string, recipe *Recipe, pins []string, clone string) string
pub(crate) fn pre_commit_hook_script(
    verify_cmd: &str,
    recipe: Option<&crate::recipe::Recipe>,
    pins: &[String],
    clone: &str,
) -> String {
    todo!("runner::keys::pre_commit_hook_script")
}
/// Go: func configureCloneIdentity(clone, agent, keyPath, hookText string) error
pub(crate) fn configure_clone_identity(
    clone: &str,
    agent: &str,
    key_path: &str,
    hook_text: &str,
) -> Result<()> {
    todo!("runner::keys::configure_clone_identity")
}
/// Go: func writePreCommitHook(clone, script string) error
pub(crate) fn write_pre_commit_hook(clone: &str, script: &str) -> Result<()> {
    todo!("runner::keys::write_pre_commit_hook")
}
/// Go: func gitConfigSection(key string) string
pub(crate) fn git_config_section(key: &str) -> String {
    todo!("runner::keys::git_config_section")
}
/// Go: func gitConfigQuote(v string) string
pub(crate) fn git_config_quote(v: &str) -> String {
    todo!("runner::keys::git_config_quote")
}
/// Go: func SignersLine(agent, pubkey string) string
pub fn signers_line(agent: &str, pubkey: &str) -> String {
    format!("{} {}", agent_email(agent), pubkey)
}
/// Go: func SignersText(s *store.Store) string
pub fn signers_text(s: &pulse_store::Store) -> String {
    todo!("runner::keys::signers_text")
}
/// Go: func landingProblems(root, base, ref, agent, jobID, pubkey, signersPath string) []string
pub(crate) fn landing_problems(
    root: &str,
    base: &str,
    ref_: &str,
    agent: &str,
    job_id: &str,
    pubkey: &str,
    signers_path: &str,
) -> Vec<String> {
    todo!("runner::keys::landing_problems")
}
/// Go: func splitTrailer(s string) []string
pub(crate) fn split_trailer(s: &str) -> Vec<String> {
    s.splitn(2, ':').map(|s| s.trim().to_string()).collect()
}
/// Go: func reprobeDue(s *store.Store, interval int, now string) [][2]string
pub(crate) fn reprobe_due(s: &pulse_store::Store, interval: i32, now: &str) -> Vec<[String; 2]> {
    todo!("runner::keys::reprobe_due")
}
