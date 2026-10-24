//! Regression tests for the `nav-guard` webview navigation backstop
//! (ADR-0028). Guards the release-only blank-window bug: the bundled app is
//! served from the non-special `tauri://` custom protocol, whose opaque
//! `Url::origin()` is only ever equal to itself, so an origin-equality check
//! cancelled the initial page load and left an empty window.

use slimcode_gui::navigation_allowed;
use tauri::Url;

fn url(s: &str) -> Url {
    Url::parse(s).expect("test url parses")
}

// Dev server: a special (http) scheme with a comparable tuple origin.
const DEV: &str = "http://localhost:1420/";
// Bundled release app on macOS/Linux: a non-special custom-protocol scheme.
const RELEASE: &str = "tauri://localhost/";

#[test]
fn dev_same_origin_navigation_is_allowed() {
    assert!(navigation_allowed(Some(&url(DEV)), &url(DEV)));
}

#[test]
fn release_same_origin_navigation_is_allowed() {
    // The bug: this returned false in release, blanking the window.
    assert!(navigation_allowed(Some(&url(RELEASE)), &url(RELEASE)));
}

#[test]
fn old_origin_comparison_is_what_broke_release() {
    // Documents the mechanism: two parses of the same custom-protocol URL have
    // opaque origins that never compare equal, while the http tuple origins
    // do. This is why dev rendered and release did not.
    let a = url(RELEASE);
    let b = url(RELEASE);
    assert_ne!(a.origin(), b.origin(), "custom-protocol origins are opaque");

    let dev_a = url(DEV);
    let dev_b = url(DEV);
    assert_eq!(dev_a.origin(), dev_b.origin(), "http origins are tuples");
}

#[test]
fn external_and_cross_origin_navigation_is_blocked() {
    // An external link must not navigate the webview away from the app.
    assert!(!navigation_allowed(
        Some(&url(RELEASE)),
        &url("https://example.com/")
    ));
    assert!(!navigation_allowed(
        Some(&url(DEV)),
        &url("http://evil.test:1420/")
    ));
    assert!(!navigation_allowed(
        Some(&url(DEV)),
        &url("file:///etc/passwd")
    ));
}

#[test]
fn navigation_without_a_current_url_is_blocked() {
    assert!(!navigation_allowed(None, &url(RELEASE)));
}
