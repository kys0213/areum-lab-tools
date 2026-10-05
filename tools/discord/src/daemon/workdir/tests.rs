use super::*;
use crate::common::config::parse_config;

/// Temp directory removed on drop so tests leave nothing behind.
struct TempDir(PathBuf);

impl std::ops::Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn unique_dir(label: &str) -> TempDir {
    let dir = std::env::temp_dir().join(format!(
        "areum-discord-workdir-{}-{label}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    TempDir(std::fs::canonicalize(&dir).unwrap())
}

fn workdirs(json: &str, home: Option<&Path>) -> Result<Workdirs, AppError> {
    Workdirs::from_config(&parse_config(json).unwrap(), home)
}

fn json_path(p: &Path) -> String {
    serde_json::to_string(p.to_str().unwrap()).unwrap()
}

#[test]
fn top_level_message_uses_its_channel_mapping() {
    let a = unique_dir("top-a");
    let d = unique_dir("top-default");
    let w = workdirs(
        &format!(
            r#"{{"workdirs":{{"c1":{}}},"default_workdir":{}}}"#,
            json_path(&a),
            json_path(&d)
        ),
        None,
    )
    .unwrap();
    assert_eq!(w.resolve("c1", None).unwrap(), a.to_str().unwrap());
}

#[test]
fn thread_message_uses_the_parent_channel_mapping() {
    let a = unique_dir("thread-a");
    let w = workdirs(
        &format!(r#"{{"workdirs":{{"c1":{}}}}}"#, json_path(&a)),
        None,
    )
    .unwrap();
    assert_eq!(w.resolve("t9", Some("c1")).unwrap(), a.to_str().unwrap());
}

#[test]
fn unmapped_channel_falls_back_to_the_default() {
    let d = unique_dir("fallback-default");
    let w = workdirs(&format!(r#"{{"default_workdir":{}}}"#, json_path(&d)), None).unwrap();
    assert_eq!(w.resolve("c2", None).unwrap(), d.to_str().unwrap());
}

#[test]
fn unmapped_channel_without_default_is_an_error() {
    let a = unique_dir("nodefault-a");
    let w = workdirs(
        &format!(r#"{{"workdirs":{{"c1":{}}}}}"#, json_path(&a)),
        None,
    )
    .unwrap();
    assert_eq!(w.resolve("c2", None).unwrap_err().kind, ErrorKind::Config);
}

#[test]
fn missing_mapped_directory_does_not_fall_back_to_the_default() {
    let d = unique_dir("missing-default");
    let gone =
        std::env::temp_dir().join(format!("areum-discord-workdir-{}-gone", std::process::id()));
    let _ = std::fs::remove_dir_all(&gone);
    let w = workdirs(
        &format!(
            r#"{{"workdirs":{{"c1":{}}},"default_workdir":{}}}"#,
            json_path(&gone),
            json_path(&d)
        ),
        None,
    )
    .unwrap();
    assert!(w.resolve("c1", None).is_err());
}

#[test]
fn a_file_is_not_a_directory() {
    let dir = unique_dir("file");
    let file = dir.join("f");
    std::fs::write(&file, "x").unwrap();
    let w = workdirs(
        &format!(r#"{{"default_workdir":{}}}"#, json_path(&file)),
        None,
    )
    .unwrap();
    assert!(w.resolve("c1", None).is_err());
}

#[test]
fn existence_is_rechecked_on_every_resolve() {
    let dir = unique_dir("recheck");
    let w = workdirs(
        &format!(r#"{{"default_workdir":{}}}"#, json_path(&dir)),
        None,
    )
    .unwrap();
    assert!(w.resolve("c1", None).is_ok());
    std::fs::remove_dir_all(&*dir).unwrap();
    assert!(w.resolve("c1", None).is_err());
}

#[test]
fn alias_keys_resolve_through_channels() {
    let a = unique_dir("alias");
    let w = workdirs(
        &format!(
            r#"{{"channels":{{"issues":"555"}},"workdirs":{{"issues":{}}}}}"#,
            json_path(&a)
        ),
        None,
    )
    .unwrap();
    assert_eq!(w.resolve("555", None).unwrap(), a.to_str().unwrap());
}

#[test]
fn tilde_slash_expands_to_home() {
    let home = unique_dir("home");
    std::fs::create_dir_all(home.join("proj")).unwrap();
    let w = workdirs(r#"{"default_workdir":"~/proj"}"#, Some(&*home)).unwrap();
    assert_eq!(
        w.resolve("c1", None).unwrap(),
        home.join("proj").to_str().unwrap()
    );
}

#[test]
fn tilde_without_home_is_rejected() {
    let err = workdirs(r#"{"default_workdir":"~/proj"}"#, None).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
}

#[test]
fn relative_and_tilde_user_paths_are_rejected() {
    for bad in ["rel/dir", "./x", "~user/x", "~", ""] {
        let quoted = serde_json::to_string(bad).unwrap();
        let default = format!(r#"{{"default_workdir":{quoted}}}"#);
        let mapped = format!(r#"{{"workdirs":{{"c1":{quoted}}}}}"#);
        for json in [default, mapped] {
            let err = workdirs(&json, Some(Path::new("/home/x"))).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Config, "{bad:?}");
        }
    }
}

#[test]
fn no_directory_settings_at_all_is_rejected() {
    let err = workdirs(r#"{"on_message":["/bin/hook"]}"#, None).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
}

#[test]
fn alias_and_id_keys_for_the_same_channel_are_rejected() {
    let err = workdirs(
        r#"{"channels":{"issues":"555"},"workdirs":{"issues":"/a","555":"/b"}}"#,
        None,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
    // "555" sorts before "issues", whatever order the map yields them in.
    assert_eq!(
        err.message,
        "workdirs[555] and workdirs[issues] both resolve to channel 555: keep only one"
    );
}

#[test]
fn duplicate_keys_are_rejected_even_when_directories_match() {
    let err = workdirs(
        r#"{"channels":{"delta":"7","alpha":"7","charlie":"7","bravo":"7"},"workdirs":{"delta":"/same","alpha":"/same","charlie":"/same","bravo":"/same"}}"#,
        None,
    )
    .unwrap_err();
    assert_eq!(err.kind, ErrorKind::Config);
    // The two smallest keys are reported, not whichever the map yields first.
    assert_eq!(
        err.message,
        "workdirs[alpha] and workdirs[bravo] both resolve to channel 7: keep only one"
    );
}
