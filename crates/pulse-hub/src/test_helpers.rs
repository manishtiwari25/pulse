#![allow(unused_variables, dead_code, unused_imports)]

use crate::httpx::{self, HandlerFn};
use crate::server::UiFs;
use pulse_bus::CancellationToken;
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::thread;

pub(crate) struct TestServer {
    pub(crate) url: String,
    ctx: CancellationToken,
    join: Option<thread::JoinHandle<anyhow::Result<()>>>,
}

impl TestServer {
    pub(crate) fn start(handler: HandlerFn) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let url = format!(
            "http://{}",
            listener.local_addr().expect("test server address")
        );
        let ctx = CancellationToken::new();
        let server_ctx = ctx.clone();
        let join = thread::spawn(move || httpx::serve(&server_ctx, listener, handler));
        Self {
            url,
            ctx,
            join: Some(join),
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.ctx.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub(crate) fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("test client")
}

pub(crate) fn test_main_setup() {
    todo!("hub::test_helpers::test_main_setup pulse_tasks::LOGIN_SOURCE_CHECK")
}

pub(crate) struct MapFs(pub(crate) BTreeMap<String, Vec<u8>>);

impl UiFs for MapFs {
    fn read_file(&self, name: &str) -> std::io::Result<Vec<u8>> {
        self.0
            .get(name)
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, name.to_string()))
    }
    fn is_dir(&self, name: &str) -> bool {
        let prefix = if name.is_empty() {
            String::new()
        } else {
            format!("{}/", name.trim_matches('/'))
        };
        self.0
            .keys()
            .any(|k| k.starts_with(&prefix) && k.len() > prefix.len())
    }
}

/// Shared Go test helpers skeletoned for fill agents:
/// server_test.go: confirmedRequest;
/// golden_test.go: normalizeGolden, readGolden, copySnapshot, jsonDifferences, rosterGolden, documentedDifference, threadEnrichmentGoldenField;
/// additions_test.go: additionsStore, additionsRequest, additionsProject, additionsRecord, additionsEventSubjects, additionsStreamEvents;
/// decisions_test.go: decisionsFixture, proposedADR, lastRevision;
/// folders_test.go: foldersRequest, folderNames;
/// job_repair_test.go: jobRepairHandler, jobRepairRequest, jobRepairStore, repairRowCount;
/// harness_test.go: registerHubFakeProvider, registerHubCacheProvider, harnessStore, fakeState;
/// heldout_test.go: heldoutRequest;
/// cred_test.go: credRequest, credUpstream, jsonBody;
/// starter_test.go: assertEvolutionFixtures, starterStore, templateStore;
/// read_test.go: projectReaderFixture; running_test.go: mustOutput; lifecycle_test.go: jsonString, mustJSON, lifecycleRepo; code_deliveries_test.go: codeDeliveryReport, seedCodeDeliveryEvent, strconvInt64; evolution_test.go: assertEvolutionShape; evolution_summary_status_truth_test.go: assertEvolutionSummaryStatusTruthShape.
pub(crate) fn confirmed_request() {
    todo!("hub::test_helpers::confirmed_request")
}
pub(crate) fn normalize_golden() {
    todo!("hub::test_helpers::normalize_golden")
}
pub(crate) fn read_golden() {
    todo!("hub::test_helpers::read_golden")
}
pub(crate) fn copy_snapshot() {
    todo!("hub::test_helpers::copy_snapshot")
}
pub(crate) fn json_differences() {
    todo!("hub::test_helpers::json_differences")
}
pub(crate) fn additions_store() {
    todo!("hub::test_helpers::additions_store")
}
pub(crate) fn additions_request() {
    todo!("hub::test_helpers::additions_request")
}
pub(crate) fn additions_project() {
    todo!("hub::test_helpers::additions_project")
}
pub(crate) fn additions_record() {
    todo!("hub::test_helpers::additions_record")
}
