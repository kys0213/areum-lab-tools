use std::path::PathBuf;

use super::*;
use crate::commands::testutil::*;
use crate::config;
use crate::output::ErrorKind;

/// Unique, not-yet-existing directory under the OS temp dir so each test
/// can exercise the real "parent dir must be created" path without
/// clobbering other tests or runs.
fn unique_init_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "areum-discord-run-init-test-{}-{label}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn run_init_creates_fresh_config_with_token() {
    let dir = unique_init_dir("fresh");
    let path = dir.join("config.json");

    let payload = run_init(
        &path,
        Some("mytoken"),
        false,
        unreachable_stdin,
        || path.exists(),
        || config::load_config(&path),
        |cfg| config::write_config_file(&path, cfg),
    )
    .unwrap();

    match payload {
        Payload::Init(data) => {
            assert_eq!(data.path, path.display().to_string());
            assert!(data.created);
        }
        other => panic!("expected Init, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"token":"mytoken"}"#
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_init_rejects_existing_file_without_force_and_leaves_it_unchanged() {
    let dir = unique_init_dir("no-force");
    let path = dir.join("config.json");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, r#"{"token":"old"}"#).unwrap();

    let err = run_init(
        &path,
        Some("newtoken"),
        false,
        unreachable_stdin,
        || path.exists(),
        || config::load_config(&path),
        |cfg| config::write_config_file(&path, cfg),
    )
    .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Usage);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"token":"old"}"#
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_init_force_preserves_channels_and_replaces_token() {
    let dir = unique_init_dir("force-preserve");
    let path = dir.join("config.json");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, r#"{"token":"old","channels":{"general":"111"}}"#).unwrap();

    let payload = run_init(
        &path,
        Some("newtoken"),
        true,
        unreachable_stdin,
        || path.exists(),
        || config::load_config(&path),
        |cfg| config::write_config_file(&path, cfg),
    )
    .unwrap();

    match payload {
        Payload::Init(data) => assert!(!data.created),
        other => panic!("expected Init, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"token":"newtoken","channels":{"general":"111"}}"#
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_init_force_without_channels_keeps_minimal_form() {
    // A --force overwrite of a file that never had a `channels` key must
    // not introduce one — the minimal `{"token":...}` shape is preserved,
    // not just the "channels present" branch covered by the sibling test.
    let dir = unique_init_dir("force-no-channels");
    let path = dir.join("config.json");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, r#"{"token":"old"}"#).unwrap();

    let payload = run_init(
        &path,
        Some("newtoken"),
        true,
        unreachable_stdin,
        || path.exists(),
        || config::load_config(&path),
        |cfg| config::write_config_file(&path, cfg),
    )
    .unwrap();

    match payload {
        Payload::Init(data) => assert!(!data.created),
        other => panic!("expected Init, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"token":"newtoken"}"#
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_init_force_with_unparseable_existing_config_fails_fast() {
    let dir = unique_init_dir("force-malformed");
    let path = dir.join("config.json");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, "{not json").unwrap();

    let err = run_init(
        &path,
        Some("newtoken"),
        true,
        unreachable_stdin,
        || path.exists(),
        || config::load_config(&path),
        |cfg| config::write_config_file(&path, cfg),
    )
    .unwrap_err();

    // Malformed existing config must fail loudly, not be silently
    // discarded and overwritten.
    assert_eq!(err.kind, ErrorKind::Config);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_init_rejects_empty_token_flag() {
    let dir = unique_init_dir("empty-flag");
    let path = dir.join("config.json");

    let err = run_init(
        &path,
        Some("   "),
        false,
        unreachable_stdin,
        || path.exists(),
        || config::load_config(&path),
        |cfg| config::write_config_file(&path, cfg),
    )
    .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(!path.exists());
}

#[test]
fn run_init_rejects_empty_stdin_token() {
    let dir = unique_init_dir("empty-stdin");
    let path = dir.join("config.json");

    let err = run_init(
        &path,
        None,
        false,
        || Ok("   \n".to_owned()),
        || path.exists(),
        || config::load_config(&path),
        |cfg| config::write_config_file(&path, cfg),
    )
    .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(!path.exists());
}

#[test]
fn run_init_trims_trailing_newline_from_stdin_token() {
    let dir = unique_init_dir("stdin-trim");
    let path = dir.join("config.json");

    run_init(
        &path,
        None,
        false,
        || Ok("mytoken\n".to_owned()),
        || path.exists(),
        || config::load_config(&path),
        |cfg| config::write_config_file(&path, cfg),
    )
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"token":"mytoken"}"#
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_init_trims_leading_and_trailing_whitespace_from_stdin_token() {
    let dir = unique_init_dir("stdin-trim-both-ends");
    let path = dir.join("config.json");

    run_init(
        &path,
        None,
        false,
        || Ok("  mytoken  \n".to_owned()),
        || path.exists(),
        || config::load_config(&path),
        |cfg| config::write_config_file(&path, cfg),
    )
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"token":"mytoken"}"#
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_init_rejects_completely_empty_stdin_token() {
    let dir = unique_init_dir("stdin-empty");
    let path = dir.join("config.json");

    let err = run_init(
        &path,
        None,
        false,
        || Ok(String::new()),
        || path.exists(),
        || config::load_config(&path),
        |cfg| config::write_config_file(&path, cfg),
    )
    .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Usage);
    assert!(!path.exists());
}

#[test]
fn run_init_empty_token_check_happens_before_any_file_io() {
    // An invalid token must short-circuit before exists()/write() are
    // even invoked, so a bad call can never touch the filesystem.
    let err = run_init(
        Path::new("/should/never/be/touched/config.json"),
        Some(""),
        false,
        unreachable_stdin,
        || panic!("exists should not be called"),
        || panic!("load_existing should not be called"),
        |_: &Config| panic!("write should not be called"),
    )
    .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Usage);
}
