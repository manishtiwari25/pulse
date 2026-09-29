//! Test helpers ported from internal/runner/token_test.go: the fake GitHub API every token test
//! answers from, and the wall-clock swap those tests use to age the source caches.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

/// Go: type fakeGitHub struct - answers GET /user and GET /user/repos; repos says how many
/// repositories the token reaches and status overrides the /user answer.
#[derive(Default)]
pub(crate) struct FakeGitHubState {
    pub(crate) repos: usize,
    pub(crate) status: u16,
    pub(crate) login: String,
    pub(crate) calls: Vec<String>,
    pub(crate) tokens: Vec<String>,
}

/// Go: `gh := &fakeGitHub{login: "hands-bot"}` - the state sits behind Go's `mu`, so tests read
/// `gh.lock().calls` where Go read `gh.calls`.
#[derive(Clone, Default)]
pub(crate) struct FakeGitHub {
    state: Arc<Mutex<FakeGitHubState>>,
}

impl FakeGitHub {
    pub(crate) fn new(state: FakeGitHubState) -> Self {
        FakeGitHub {
            state: Arc::new(Mutex::new(state)),
        }
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, FakeGitHubState> {
        self.state.lock().unwrap()
    }

    /// Go: func (g *fakeGitHub) install(t *testing.T)
    ///
    /// Swaps githubHTTP for the fake transport; dropping the guard is Go's t.Cleanup (the old
    /// transport back, the gh source cache forgotten).
    #[must_use = "hold the guard for the test's life; dropping it restores the GitHub transport"]
    pub(crate) fn install(&self) -> FakeGitHubInstall {
        let g = self.state.clone();
        let fake: Box<crate::executor::GithubHttpFn> = Box::new(move |token: &str, url: &str| {
            let mut g = g.lock().unwrap();
            // Go saw r.URL.Path and r.URL.RawQuery; the transport here gets the whole URL.
            let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
            let path_query = rest.find('/').map(|i| &rest[i..]).unwrap_or("/");
            let (path, raw_query) = path_query.split_once('?').unwrap_or((path_query, ""));
            g.calls.push(format!("{path}?{raw_query}"));
            // The Authorization header Go trimmed "Bearer " from carries exactly this token.
            g.tokens.push(token.to_string());
            let (mut body, mut status) = (String::new(), 200u16);
            match path {
                "/user" => {
                    let mut login = g.login.clone();
                    if login.is_empty() {
                        login = "hands-bot".to_string();
                    }
                    body = format!("{{\"login\":\"{login}\"}}");
                    if g.status != 0 {
                        status = g.status;
                    }
                }
                "/user/repos" => {
                    let items = (0..g.repos)
                        .map(|_| "{\"full_name\":\"acme/repo\"}")
                        .collect::<Vec<_>>();
                    body = format!("[{}]", items.join(","));
                }
                _ => status = 404,
            }
            let mut h = BTreeMap::new();
            h.insert(
                "github-authentication-token-expiration".to_string(),
                "2027-01-01 00:00:00 UTC".to_string(),
            );
            Ok((status, h, body.into_bytes()))
        });
        crate::token::GH_SOURCE.lock().unwrap().forget();
        let old = crate::executor::set_github_http(fake);
        FakeGitHubInstall { old: Some(old) }
    }

    /// Go: func (g *fakeGitHub) count() int
    pub(crate) fn count(&self) -> usize {
        self.state.lock().unwrap().calls.len()
    }
}

pub(crate) struct FakeGitHubInstall {
    old: Option<Box<crate::executor::GithubHttpFn>>,
}

impl Drop for FakeGitHubInstall {
    fn drop(&mut self) {
        if let Some(old) = self.old.take() {
            let _ = crate::executor::set_github_http(old);
        }
        crate::token::GH_SOURCE.lock().unwrap().forget();
    }
}

/// The instant Go's swapped `wallClock` closure returns; `WALL_CLOCK` is a fn pointer, so the
/// test's `clock` variable lives here and `advance_wall_clock` is Go's `clock = clock.Add(d)`.
static TEST_CLOCK: LazyLock<Mutex<SystemTime>> =
    LazyLock::new(|| Mutex::new(SystemTime::UNIX_EPOCH));

fn test_wall_clock() -> SystemTime {
    *TEST_CLOCK.lock().unwrap()
}

/// Go: `oldClock := wallClock; t.Cleanup(func() { wallClock = oldClock }); wallClock = func() time.Time { return clock }`
#[must_use = "hold the guard for the test's life; dropping it restores the wall clock"]
pub(crate) fn fixed_wall_clock(clock: SystemTime) -> WallClockGuard {
    let old = *crate::run::WALL_CLOCK.read().expect("wall clock");
    *TEST_CLOCK.lock().unwrap() = clock;
    *crate::run::WALL_CLOCK.write().expect("wall clock") = test_wall_clock;
    WallClockGuard { old }
}

/// Go: `clock = clock.Add(d)`
pub(crate) fn advance_wall_clock(d: Duration) {
    *TEST_CLOCK.lock().unwrap() += d;
}

pub(crate) struct WallClockGuard {
    old: crate::run::WallClockFn,
}

impl Drop for WallClockGuard {
    fn drop(&mut self) {
        *crate::run::WALL_CLOCK.write().expect("wall clock") = self.old;
    }
}

/// Go: time.Date(year, month, day, hour, min, sec, 0, time.UTC)
pub(crate) fn time_date(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
) -> SystemTime {
    let dt =
        chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, year, month, day, hour, min, sec).unwrap();
    SystemTime::UNIX_EPOCH + Duration::from_secs(dt.timestamp() as u64)
}
