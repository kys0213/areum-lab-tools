use super::*;
use crate::commands::testutil::*;
use crate::output::ErrorKind;

#[tokio::test]
async fn run_read_sorts_descending_input_to_ascending_output() {
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![
        message("30"),
        message("10"),
        message("20"),
    ])]);

    let payload = run_read(&api, "c", None, 50).await.unwrap();

    match payload {
        Payload::Read(data) => {
            let ids: Vec<&str> = data.messages.iter().map(|m| m.id.as_str()).collect();
            assert_eq!(ids, vec!["10", "20", "30"]);
            assert_eq!(data.count, 3);
            assert_eq!(data.cursor.as_deref(), Some("30"));
        }
        other => panic!("expected Read, got {other:?}"),
    }
}

#[tokio::test]
async fn run_read_cursor_is_numeric_max_not_lexical_max() {
    // Lexical comparison would pick "9" over "10"; numeric must pick "10".
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![message("9"), message("10")])]);

    let payload = run_read(&api, "c", None, 50).await.unwrap();

    match payload {
        Payload::Read(data) => {
            let ids: Vec<&str> = data.messages.iter().map(|m| m.id.as_str()).collect();
            assert_eq!(ids, vec!["9", "10"]);
            assert_eq!(data.cursor.as_deref(), Some("10"));
        }
        other => panic!("expected Read, got {other:?}"),
    }
}

#[tokio::test]
async fn run_read_empty_result_echoes_input_after_when_some() {
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![])]);

    let payload = run_read(&api, "c", Some("42"), 50).await.unwrap();

    match payload {
        Payload::Read(data) => {
            assert_eq!(data.count, 0);
            assert_eq!(data.cursor.as_deref(), Some("42"));
        }
        other => panic!("expected Read, got {other:?}"),
    }
}

#[tokio::test]
async fn run_read_empty_result_with_no_after_has_none_cursor() {
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![])]);

    let payload = run_read(&api, "c", None, 50).await.unwrap();

    match payload {
        Payload::Read(data) => assert_eq!(data.cursor, None),
        other => panic!("expected Read, got {other:?}"),
    }
}

#[tokio::test]
async fn run_read_rejects_limit_zero() {
    let api = MockDiscordApi::new();
    let err = run_read(&api, "c", None, 0).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[tokio::test]
async fn run_read_rejects_limit_over_100() {
    let api = MockDiscordApi::new();
    let err = run_read(&api, "c", None, 101).await.unwrap_err();
    assert_eq!(err.kind, ErrorKind::Usage);
}

#[tokio::test]
async fn run_read_accepts_limit_boundaries_1_and_100() {
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![]), Ok(vec![])]);

    assert!(run_read(&api, "c", None, 1).await.is_ok());
    assert!(run_read(&api, "c", None, 100).await.is_ok());
}

#[tokio::test]
async fn run_read_rejects_non_numeric_message_id_as_api_error() {
    // A non-snowflake id in the response is an API contract violation,
    // not a client-side usage mistake.
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![message("not-a-number")])]);

    let err = run_read(&api, "c", None, 50).await.unwrap_err();

    assert_eq!(err.kind, ErrorKind::Api);
}

#[tokio::test]
async fn run_read_passes_channel_after_and_limit_to_api() {
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![])]);

    run_read(&api, "chan-1", Some("42"), 7).await.unwrap();

    let calls = api.get_calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], ("chan-1".to_owned(), Some("42".to_owned()), 7));
}
