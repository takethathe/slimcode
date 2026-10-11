//! Ticket 01's ABI contract tests: the C boundary itself — ABI version,
//! session creation from options JSON, read-only accessors, structured errors,
//! string ownership and handle lifetime. Turn-path behavior lives in the later
//! tickets' test files (`turn_stream.rs`, `tool_turn_accessors.rs`,
//! `lifecycle.rs`, `persistence.rs`, `compact_now.rs`).

mod common;

use std::ffi::{CStr, c_void};
use std::fs;
use std::path::Path;
use std::ptr;

use slimcode_ffi::abi::session as abi;

/// The complete ABI of `include/slimcode.h` (the spec's ABI contract section):
/// ticket 01 wrote the header once for every symbol, tickets 02–06 filled the
/// implementations in, and `the_abi_is_exactly_the_spec_symbol_set` now pins
/// the header and the source to this list in both directions.
const ABI_SYMBOLS: [&str; 13] = [
    "slimcode_abi_version",
    "slimcode_session_create",
    "slimcode_session_prompt",
    "slimcode_session_next_event",
    "slimcode_session_compact_now",
    "slimcode_session_abort",
    "slimcode_session_dispose",
    "slimcode_session_id",
    "slimcode_session_title",
    "slimcode_session_usage_json",
    "slimcode_session_messages_json",
    "slimcode_last_error",
    "slimcode_free_string",
];

#[test]
fn abi_version_is_one() {
    assert_eq!(common::abi_version(), 1);
    assert_eq!(unsafe { abi::slimcode_abi_version() }, abi::ABI_VERSION);
}

#[test]
fn create_with_empty_options_uses_the_slimcode_home_env() {
    // `{}` means literally no options, so the SDK resolves home itself: the
    // test pins SLIMCODE_HOME at the scratch home instead of passing one
    // (nothing may reach the developer's real ~/.slimcode).
    let (_guard, _home, _cwd) = common::test_env("empty-options");
    let handle = common::create("{}");
    assert!(
        !handle.is_null(),
        "create({{}}) failed: {:?}",
        common::last_error()
    );
    let id = common::session_id(handle).expect("a live handle yields its id");
    assert!(id.starts_with("slimcode-"), "unexpected session id {id:?}");
    assert_eq!(common::title(handle).as_deref(), Some(""));
    assert_eq!(common::last_error(), None, "a successful call clears it");
    common::dispose(handle);
}

#[test]
fn create_with_explicit_options_creates_a_session() {
    let (home, cwd) = common::scratch("explicit-options");
    let options = common::options(&home, &cwd);
    // The builder carries every documented field except the resume pair; the
    // JSON is the ABI's own shape, so assert it parses back as an object with
    // the pinned home.
    let parsed =
        serde_json::from_str::<serde_json::Value>(&options.to_json()).expect("options JSON");
    assert_eq!(parsed["home"], serde_json::json!(home.to_string_lossy()));

    let handle = common::create_ok(&options.to_json());
    let id = common::session_id(handle).expect("session id");
    assert!(id.starts_with("slimcode-"), "unexpected session id {id:?}");
    assert_eq!(common::title(handle).as_deref(), Some(""));
    common::dispose(handle);
    assert_eq!(common::last_error(), None, "dispose is not an error path");
}

#[test]
fn bad_options_json_is_a_null_handle_with_an_error() {
    for payload in ["{", "[]", "null", "42", "\"text\"", ""] {
        let handle = common::create(payload);
        assert!(handle.is_null(), "{payload:?} must not create a session");
        assert!(
            common::last_error().is_some(),
            "{payload:?} must set last_error"
        );
    }
}

#[test]
fn an_unknown_option_field_is_rejected_by_name() {
    let handle = common::create(r#"{"api_key":"sk-test","api_ky":"sk-typo"}"#);
    assert!(handle.is_null(), "an unknown field must fail creation");
    let message = common::last_error().expect("last_error");
    assert!(
        message.contains("api_ky"),
        "the error should name the offending field: {message}"
    );
}

#[test]
fn null_options_json_is_rejected() {
    let handle = unsafe { abi::slimcode_session_create(ptr::null()) };
    assert!(handle.is_null());
    assert!(common::last_error().is_some());
}

#[test]
fn persist_false_with_a_session_id_fails_through_the_sdk() {
    // An SDK-level creation failure (resuming needs the store) must surface as
    // NULL + last_error, not a panic or a half-built handle.
    let (home, cwd) = common::scratch("no-store-resume");
    let options = common::options(&home, &cwd)
        .persist(false)
        .session_id("slimcode-1-2-3");
    let handle = common::create(&options.to_json());
    assert!(handle.is_null(), "resume without a store must fail");
    let message = common::last_error().expect("last_error");
    assert!(
        message.contains("persist"),
        "the SDK reason should come through: {message}"
    );
}

#[test]
fn a_new_session_title_is_an_empty_string_not_null() {
    let (home, cwd) = common::scratch("empty-title");
    let handle = common::create_ok(&common::options(&home, &cwd).to_json());
    let raw = unsafe { abi::slimcode_session_title(handle) };
    assert!(!raw.is_null(), "title is never NULL for a live handle");
    assert_eq!(unsafe { CStr::from_ptr(raw) }.to_str().unwrap(), "");
    common::free_string(raw);
    common::dispose(handle);
}

#[test]
fn free_string_null_is_a_no_op() {
    common::free_string(ptr::null_mut());
}

#[test]
fn null_handle_accessors_error_while_dispose_is_silent() {
    // A dangling, never-dereferenced pointer stands in for "not a handle":
    // the registry only keys on the address.
    let unknown = ptr::dangling_mut::<c_void>();
    for pointer in [ptr::null_mut::<c_void>(), unknown] {
        assert!(common::session_id(pointer).is_none());
        assert!(
            common::last_error().is_some(),
            "id on {pointer:?} must error"
        );
        assert!(common::title(pointer).is_none());
        assert!(
            common::last_error().is_some(),
            "title on {pointer:?} must error"
        );
    }

    // `dispose` is the one silent entry point: NULL / unknown handles are a
    // no-op, and it touches neither side of the thread's error state.
    let marker = common::last_error().expect("the accessors set an error");
    common::dispose(ptr::null_mut());
    common::dispose(unknown);
    assert_eq!(
        common::last_error().as_deref(),
        Some(marker.as_str()),
        "dispose must not touch last_error"
    );
}

#[test]
fn dispose_invalidates_the_handle() {
    let (home, cwd) = common::scratch("dispose-invalidates");
    let handle = common::create_ok(&common::options(&home, &cwd).to_json());
    assert_eq!(common::last_error(), None);

    common::dispose(handle);
    assert!(
        common::session_id(handle).is_none(),
        "a disposed handle must not answer"
    );
    assert!(common::last_error().is_some());

    // A second dispose of the same pointer is a no-op, not a crash.
    common::dispose(handle);
}

#[test]
fn last_error_is_thread_local() {
    assert!(common::create("{").is_null());
    let main_error = common::last_error().expect("the main thread has an error");

    let worker = std::thread::spawn(|| {
        // A fresh thread starts clean: the main thread's message must not leak.
        assert_eq!(
            common::last_error(),
            None,
            "last_error leaked into a fresh thread"
        );
        assert!(common::create("{").is_null());
        common::last_error().expect("the worker thread has its own error")
    })
    .join()
    .expect("worker thread");

    assert!(!worker.is_empty());
    assert_eq!(
        common::last_error().as_deref(),
        Some(main_error.as_str()),
        "the main thread's error survived the worker's call"
    );
}

/// The header is complete: every spec symbol is declared. The reverse
/// direction (every declared symbol is implemented) is asserted by
/// `the_abi_is_exactly_the_spec_symbol_set` below.
#[test]
fn the_header_declares_the_whole_spec_abi() {
    let header =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("include/slimcode.h"))
            .expect("read include/slimcode.h");
    let missing: Vec<&str> = ABI_SYMBOLS
        .iter()
        .filter(|symbol| !header.contains(&format!("{symbol}(")))
        .copied()
        .collect();
    assert!(
        missing.is_empty(),
        "include/slimcode.h is missing: {missing:?}"
    );
    assert!(
        header.contains("SLIMCODE_H"),
        "the header needs a include guard so it can be included twice"
    );
}

/// Ticket 01 could only assert the forward direction (every implemented symbol
/// is declared) because tickets 02–06 had not landed yet. With all five ABI
/// files filled in, the three sets must coincide exactly: the spec list, the
/// symbols the header declares, and the `slimcode_*` symbols the source
/// defines. A symbol implemented but never documented, or documented but never
/// implemented, now fails here.
#[test]
fn the_abi_is_exactly_the_spec_symbol_set() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let header =
        fs::read_to_string(root.join("include/slimcode.h")).expect("read include/slimcode.h");

    let spec = sorted_set(ABI_SYMBOLS);
    let declared = sorted_set(declared_symbols(&header));
    let implemented = sorted_set(
        common::implemented_symbols(&root.join("src"))
            .into_iter()
            .map(|(symbol, _)| symbol),
    );

    assert_eq!(
        declared, spec,
        "include/slimcode.h must declare exactly the spec symbols"
    );
    assert_eq!(
        implemented, spec,
        "src/**/*.rs must implement exactly the spec symbols"
    );
    assert_eq!(
        declared, implemented,
        "every declared symbol must be implemented, and vice versa"
    );
}

/// The names the header *declares*. Only declaration lines qualify (not a
/// comment, not blank, ends with `;`), so the many `slimcode_*` mentions in the
/// header's prose and examples cannot leak into the set.
fn declared_symbols(header: &str) -> Vec<String> {
    header
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('*') && !line.starts_with('/') && line.ends_with(';'))
        .filter_map(|line| {
            let start = line.find("slimcode_")?;
            let name = line[start + "slimcode_".len()..].split('(').next()?;
            Some(format!("slimcode_{name}"))
        })
        .collect()
}

/// One deduplicated, sorted set, so equality does not depend on file order.
fn sorted_set(names: impl IntoIterator<Item = impl Into<String>>) -> Vec<String> {
    let mut set: Vec<String> = names.into_iter().map(Into::into).collect();
    set.sort();
    set.dedup();
    set
}
