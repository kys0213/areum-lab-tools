use super::*;
use crate::commands::testutil::*;
use crate::output::ErrorKind;

#[tokio::test]
async fn run_wait_returns_immediately_when_first_poll_has_messages() {
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![message("10")])]);
    let sleeper = FakeSleeper::new();

    let payload = run_wait(&api, &sleeper, "c", None, 30, 5, 50)
        .await
        .unwrap();

    match payload {
        Payload::Wait(data) => {
            assert!(!data.timed_out);
            assert_eq!(data.count, 1);
            assert_eq!(data.cursor.as_deref(), Some("10"));
        }
        other => panic!("expected Wait, got {other:?}"),
    }
    assert_eq!(sleeper.calls.borrow().len(), 0);
}

#[tokio::test]
async fn run_wait_sorts_and_derives_cursor_same_as_read_when_found() {
    // Wait reuses sort_ascending_by_id/newest_cursor; verify the found
    // path applies the same ordering and cursor rule as `read`.
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![
        message("30"),
        message("10"),
        message("20"),
    ])]);
    let sleeper = FakeSleeper::new();

    let payload = run_wait(&api, &sleeper, "c", None, 30, 5, 50)
        .await
        .unwrap();

    match payload {
        Payload::Wait(data) => {
            let ids: Vec<&str> = data.messages.iter().map(|m| m.id.as_str()).collect();
            assert_eq!(ids, vec!["10", "20", "30"]);
            assert_eq!(data.count, 3);
            assert_eq!(data.cursor.as_deref(), Some("30"));
        }
        other => panic!("expected Wait, got {other:?}"),
    }
}

#[tokio::test]
async fn run_wait_sleeps_n_minus_1_times_before_nth_poll_has_messages() {
    let api =
        MockDiscordApi::with_get_responses(vec![Ok(vec![]), Ok(vec![]), Ok(vec![message("5")])]);
    let sleeper = FakeSleeper::new();

    let payload = run_wait(&api, &sleeper, "c", None, 20, 5, 50)
        .await
        .unwrap();

    match payload {
        Payload::Wait(data) => assert!(!data.timed_out),
        other => panic!("expected Wait, got {other:?}"),
    }
    assert_eq!(sleeper.calls.borrow().len(), 2);
}

#[tokio::test]
async fn run_wait_times_out_after_ceil_polls_all_empty() {
    // timeout 10 / interval 3 -> ceil(10/3) = 4 polls, 3 sleeps between them.
    let api =
        MockDiscordApi::with_get_responses(vec![Ok(vec![]), Ok(vec![]), Ok(vec![]), Ok(vec![])]);
    let sleeper = FakeSleeper::new();

    let payload = run_wait(&api, &sleeper, "c", Some("7"), 10, 3, 50)
        .await
        .unwrap();

    match payload {
        Payload::Wait(data) => {
            assert!(data.timed_out);
            assert_eq!(data.count, 0);
            assert!(data.messages.is_empty());
            assert_eq!(data.cursor.as_deref(), Some("7"));
        }
        other => panic!("expected Wait, got {other:?}"),
    }
    assert_eq!(api.get_calls.borrow().len(), 4);
    assert_eq!(sleeper.calls.borrow().len(), 3);
}

#[tokio::test]
async fn run_wait_poll_count_is_exact_when_timeout_divides_interval_evenly() {
    // timeout 10 / interval 5 -> ceil(10/5) = 2 polls, 1 sleep between them.
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![]), Ok(vec![])]);
    let sleeper = FakeSleeper::new();

    let payload = run_wait(&api, &sleeper, "c", None, 10, 5, 50)
        .await
        .unwrap();

    match payload {
        Payload::Wait(data) => assert!(data.timed_out),
        other => panic!("expected Wait, got {other:?}"),
    }
    assert_eq!(api.get_calls.borrow().len(), 2);
    assert_eq!(sleeper.calls.borrow().len(), 1);
}

#[tokio::test]
async fn run_wait_polls_at_least_once_when_timeout_is_zero() {
    // timeout 0 / interval 5 -> ceil(0/5) = 0, but max_polls floors at 1
    // poll so `wait` always checks at least once.
    let api = MockDiscordApi::with_get_responses(vec![Ok(vec![])]);
    let sleeper = FakeSleeper::new();

    let payload = run_wait(&api, &sleeper, "c", None, 0, 5, 50).await.unwrap();

    match payload {
        Payload::Wait(data) => assert!(data.timed_out),
        other => panic!("expected Wait, got {other:?}"),
    }
    assert_eq!(api.get_calls.borrow().len(), 1);
    assert_eq!(sleeper.calls.borrow().len(), 0);
}

#[tokio::test]
async fn run_wait_rejects_interval_zero() {
    let api = MockDiscordApi::new();
    let sleeper = FakeSleeper::new();

    let err = run_wait(&api, &sleeper, "c", None, 10, 0, 50)
        .await
        .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Usage);
}

#[tokio::test]
async fn run_wait_rejects_limit_out_of_range() {
    let api = MockDiscordApi::new();
    let sleeper = FakeSleeper::new();

    let err = run_wait(&api, &sleeper, "c", None, 10, 5, 0)
        .await
        .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Usage);
}

#[tokio::test]
async fn run_wait_propagates_api_error_immediately() {
    let api = MockDiscordApi::with_get_responses(vec![Err(AppError::new(ErrorKind::Api, "boom"))]);
    let sleeper = FakeSleeper::new();

    let err = run_wait(&api, &sleeper, "c", None, 30, 5, 50)
        .await
        .unwrap_err();

    assert_eq!(err.kind, ErrorKind::Api);
    assert_eq!(sleeper.calls.borrow().len(), 0);
}
