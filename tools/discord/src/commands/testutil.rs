use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use crate::api::{Author, DiscordApi, Message, SendRequest, SentMessage};
use crate::output::AppError;

use super::wait::Sleeper;

pub(crate) fn unreachable_read_file(_: &str) -> std::io::Result<Vec<u8>> {
    panic!("read_file should not be called")
}

pub(crate) fn unreachable_stdin() -> std::io::Result<String> {
    panic!("stdin should not be read")
}

pub(crate) type GetCall = (String, Option<String>, u8);

/// Scripted [`DiscordApi`]: each call pops the next queued response and
/// records the arguments it was invoked with.
pub(crate) struct MockDiscordApi {
    pub(crate) send_responses: RefCell<VecDeque<Result<SentMessage, AppError>>>,
    pub(crate) send_calls: RefCell<Vec<SendRequest>>,
    pub(crate) get_responses: RefCell<VecDeque<Result<Vec<Message>, AppError>>>,
    pub(crate) get_calls: RefCell<Vec<GetCall>>,
}

impl MockDiscordApi {
    pub(crate) fn new() -> Self {
        Self {
            send_responses: RefCell::new(VecDeque::new()),
            send_calls: RefCell::new(Vec::new()),
            get_responses: RefCell::new(VecDeque::new()),
            get_calls: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn with_get_responses(responses: Vec<Result<Vec<Message>, AppError>>) -> Self {
        let api = Self::new();
        *api.get_responses.borrow_mut() = responses.into_iter().collect();
        api
    }
}

impl DiscordApi for MockDiscordApi {
    async fn send_message(&self, req: &SendRequest) -> Result<SentMessage, AppError> {
        self.send_calls.borrow_mut().push(req.clone());
        self.send_responses
            .borrow_mut()
            .pop_front()
            .expect("test must queue a send response before calling send_message")
    }

    async fn get_messages(
        &self,
        channel_id: &str,
        after: Option<&str>,
        limit: u8,
    ) -> Result<Vec<Message>, AppError> {
        self.get_calls
            .borrow_mut()
            .push((channel_id.to_owned(), after.map(str::to_owned), limit));
        self.get_responses
            .borrow_mut()
            .pop_front()
            .expect("test must queue a get_messages response before calling get_messages")
    }
}

/// Records sleep durations instead of actually waiting, so `wait` tests
/// run instantly and assert poll cadence deterministically.
pub(crate) struct FakeSleeper {
    pub(crate) calls: RefCell<Vec<Duration>>,
}

impl FakeSleeper {
    pub(crate) fn new() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Sleeper for FakeSleeper {
    async fn sleep(&self, dur: Duration) {
        self.calls.borrow_mut().push(dur);
    }
}

pub(crate) fn message(id: &str) -> Message {
    Message {
        id: id.to_owned(),
        channel_id: "c".into(),
        author: Author {
            id: "u1".into(),
            username: "alice".into(),
            bot: false,
        },
        content: "hi".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        attachments: vec![],
    }
}
