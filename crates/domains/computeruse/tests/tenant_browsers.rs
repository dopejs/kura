//! 3.3: the tenant browser pool against the protocol-conformant fake worker,
//! started by a fake launcher the way the sandbox starts the real one.

use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use kura_computeruse::{
    AttachedWorker, BrowserLauncher, BrowserLimits, BrowserOp, DownloadTarget, LaunchedBrowser,
    TenantBrowsers,
};
use parking_lot::Mutex;

#[derive(Default)]
struct FakeLauncher {
    env: Vec<(String, String)>,
    fail: bool,
    children: Mutex<HashMap<String, Child>>,
    launched: Mutex<Vec<String>>,
    stopped: Mutex<Vec<String>>,
}

impl FakeLauncher {
    fn with(env: &[(&str, &str)], fail: bool) -> Self {
        FakeLauncher {
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            fail,
            ..FakeLauncher::default()
        }
    }
}

impl BrowserLauncher for FakeLauncher {
    fn launch(&self, tenant_id: &str) -> Result<LaunchedBrowser, String> {
        if self.fail {
            return Err("docker is down".to_string());
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_fake_browser_worker"))
            .envs(self.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|err| err.to_string())?;
        let worker = AttachedWorker::new(
            Box::new(child.stdin.take().unwrap()),
            Box::new(child.stdout.take().unwrap()),
            2_000,
        )?;
        let handle = format!("exec_{}_{}", tenant_id, self.launched.lock().len());
        self.launched.lock().push(handle.clone());
        self.children.lock().insert(handle.clone(), child);
        Ok(LaunchedBrowser { handle, worker })
    }

    fn stop(&self, handle: &str) {
        self.stopped.lock().push(handle.to_string());
        if let Some(mut child) = self.children.lock().remove(handle) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Kills whatever a test left running.
struct Reap(Arc<FakeLauncher>);

impl Drop for Reap {
    fn drop(&mut self) {
        for (_, mut child) in self.0.children.lock().drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn pool(launcher: &Arc<FakeLauncher>, max_sessions: usize) -> (Arc<TenantBrowsers>, Reap) {
    let browsers = TenantBrowsers::new(
        launcher.clone(),
        BrowserLimits {
            max_sessions,
            idle: Duration::from_secs(600),
        },
    );
    (browsers, Reap(launcher.clone()))
}

#[test]
fn a_tenant_has_one_browser_that_its_calls_reuse() {
    let launcher = Arc::new(FakeLauncher::default());
    let (browsers, _reap) = pool(&launcher, 4);
    let page = browsers
        .open("ten_a", "https://example.org/a")
        .expect("open");
    assert_eq!(page.url, "https://example.org/a");
    let page = browsers
        .open("ten_a", "https://example.org/b")
        .expect("navigate");
    assert_eq!(page.url, "https://example.org/b");
    let page = browsers.act("ten_a", &BrowserOp::Read).expect("read");
    assert_eq!(page.url, "https://example.org/b");
    browsers
        .act(
            "ten_a",
            &BrowserOp::Click {
                selector: "#go".to_string(),
            },
        )
        .expect("click");
    assert_eq!(
        launcher.launched.lock().len(),
        1,
        "one browser for the tenant"
    );
    assert_eq!(browsers.active(), 1);

    assert!(browsers.close("ten_a").expect("close"));
    assert_eq!(browsers.active(), 0);
    assert_eq!(
        *launcher.stopped.lock(),
        ["exec_ten_a_0"],
        "its container is removed"
    );
    assert!(!browsers.close("ten_a").expect("close again"));
    let err = browsers.act("ten_a", &BrowserOp::Read).expect_err("closed");
    assert!(err.contains("browser.open"), "{err}");
}

#[test]
fn the_daemon_wide_cap_refuses_a_new_browser_until_one_closes() {
    let launcher = Arc::new(FakeLauncher::default());
    let (browsers, _reap) = pool(&launcher, 1);
    browsers.open("ten_a", "https://example.org/").expect("a");
    let err = browsers
        .open("ten_b", "https://example.org/")
        .expect_err("b over the cap");
    assert!(err.contains("no browser is free"), "{err}");
    // The holder keeps using it.
    browsers
        .open("ten_a", "https://example.org/next")
        .expect("a again");
    browsers.close("ten_a").unwrap();
    browsers
        .open("ten_b", "https://example.org/")
        .expect("b after a closed");
    assert_eq!(browsers.active(), 1);
}

#[test]
fn only_web_pages_open_and_nothing_starts_for_the_rest() {
    let launcher = Arc::new(FakeLauncher::default());
    let (browsers, _reap) = pool(&launcher, 4);
    for url in [
        "file:///etc/passwd",
        "data:text/html,<h1>x</h1>",
        "javascript:alert(1)",
        "chrome://settings",
        "not a url",
        "http://",
    ] {
        assert!(browsers.open("ten_a", url).is_err(), "opened {url}");
    }
    assert!(
        browsers.open("", "https://example.org/").is_err(),
        "no tenant"
    );
    assert!(launcher.launched.lock().is_empty());
}

#[test]
fn an_idle_browser_is_closed_and_a_busy_one_is_not() {
    let launcher = Arc::new(FakeLauncher::default());
    let (browsers, _reap) = pool(&launcher, 4);
    browsers
        .open("ten_a", "https://example.org/")
        .expect("open");
    assert_eq!(browsers.reap_idle(Instant::now()), 0, "just used");
    let later = Instant::now() + Duration::from_secs(601);
    assert_eq!(browsers.reap_idle(later), 1);
    assert_eq!(browsers.active(), 0);
    assert_eq!(launcher.stopped.lock().len(), 1);
    // The next open starts a fresh one.
    browsers
        .open("ten_a", "https://example.org/")
        .expect("reopen");
    assert_eq!(launcher.launched.lock().len(), 2);
}

#[test]
fn a_browser_that_stops_answering_is_ended_and_replaced() {
    // The fake exits after answering start, navigate and the first snapshot.
    let launcher = Arc::new(FakeLauncher::with(
        &[("FAKE_WORKER_EXIT_AFTER", "3")],
        false,
    ));
    let (browsers, _reap) = pool(&launcher, 4);
    browsers
        .open("ten_a", "https://example.org/")
        .expect("open");
    let err = browsers
        .act("ten_a", &BrowserOp::Read)
        .expect_err("worker gone");
    assert!(!err.is_empty());
    assert_eq!(browsers.active(), 0, "the dead browser freed its place");
    assert_eq!(launcher.stopped.lock().len(), 1);
    browsers
        .open("ten_a", "https://example.org/")
        .expect("a fresh browser");
    assert_eq!(launcher.launched.lock().len(), 2);
}

#[test]
fn a_launch_that_fails_frees_its_place() {
    let launcher = Arc::new(FakeLauncher::with(&[], true));
    let (browsers, _reap) = pool(&launcher, 1);
    let err = browsers
        .open("ten_a", "https://example.org/")
        .expect_err("launch fails");
    assert!(err.contains("docker is down"), "{err}");
    assert_eq!(browsers.active(), 0);
}

#[test]
fn shutdown_closes_every_browser() {
    let launcher = Arc::new(FakeLauncher::default());
    let (browsers, _reap) = pool(&launcher, 4);
    browsers.open("ten_a", "https://example.org/").unwrap();
    browsers.open("ten_b", "https://example.org/").unwrap();
    browsers.close_all();
    assert_eq!(browsers.active(), 0);
    assert_eq!(launcher.stopped.lock().len(), 2);
}

#[test]
fn a_download_is_saved_to_the_download_dir_and_reported_by_path() {
    let dir = tempfile::tempdir().expect("dir");
    let downloads = dir.path().join("downloads");
    let launcher = Arc::new(FakeLauncher::with(
        &[("KURA_BROWSER_DOWNLOAD_DIR", downloads.to_str().unwrap())],
        false,
    ));
    let (browsers, _reap) = pool(&launcher, 4);

    // By selector there must be a page to click on.
    let err = browsers
        .download("ten_a", &DownloadTarget::Selector("#dl".to_string()))
        .expect_err("nothing open");
    assert!(err.contains("browser.open"), "{err}");
    assert!(launcher.launched.lock().is_empty());

    // By URL the browser starts if needed.
    let file = browsers
        .download(
            "ten_a",
            &DownloadTarget::Url("https://example.org/report.txt".to_string()),
        )
        .expect("download");
    assert!(
        file.path.starts_with(downloads.to_str().unwrap()),
        "{file:?}"
    );
    let on_disk = std::fs::metadata(&file.path).expect("saved");
    assert_eq!(on_disk.len() as i64, file.size_bytes);
    assert!(file.size_bytes > 0);
    assert_eq!(launcher.launched.lock().len(), 1);

    let file = browsers
        .download("ten_a", &DownloadTarget::Selector("#dl".to_string()))
        .expect("by selector on the open page");
    assert!(std::path::Path::new(&file.path).exists());

    let refused = browsers
        .download(
            "ten_a",
            &DownloadTarget::Url("file:///etc/passwd".to_string()),
        )
        .expect_err("not a web URL");
    assert!(!refused.is_empty());
}

/// A worker with no download directory would send the bytes back; the
/// tenant pool refuses that rather than drop them.
#[test]
fn a_worker_without_a_download_dir_cannot_save_for_a_tenant() {
    let launcher = Arc::new(FakeLauncher::default());
    let (browsers, _reap) = pool(&launcher, 4);
    let err = browsers
        .download(
            "ten_a",
            &DownloadTarget::Url("https://example.org/report.txt".to_string()),
        )
        .expect_err("no directory");
    assert!(err.contains("cannot save downloads"), "{err}");
}
