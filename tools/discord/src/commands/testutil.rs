use std::cell::RefCell;
use std::collections::VecDeque;
use std::time::Duration;

use crate::common::api::{
    Author, CreateThreadRequest, CreatedThread, DiscordApi, Message, SendRequest, SentMessage,
};
use crate::output::AppError;

use super::wait::Sleeper;

pub(crate) fn unreachable_read_file(_: &str) -> std::io::Result<Vec<u8>> {
    panic!("read_file should not be called")
}

pub(crate) fn unreachable_stdin() -> std::io::Result<String> {
    panic!("stdin should not be read")
}

pub(crate) type GetCall = (String, Option<String>, u8);

/// A recorded interaction-response / message-edit call: the ids involved plus
/// the JSON payload the daemon built, so tests assert the exact callback shape.
pub(crate) type InteractionCall = (String, String, serde_json::Value);

/// Scripted [`DiscordApi`]: each call pops the next queued response and
/// records the arguments it was invoked with.
pub(crate) struct MockDiscordApi {
    pub(crate) send_responses: RefCell<VecDeque<Result<SentMessage, AppError>>>,
    pub(crate) send_calls: RefCell<Vec<SendRequest>>,
    pub(crate) get_responses: RefCell<VecDeque<Result<Vec<Message>, AppError>>>,
    pub(crate) get_calls: RefCell<Vec<GetCall>>,
    pub(crate) thread_responses: RefCell<VecDeque<Result<CreatedThread, AppError>>>,
    pub(crate) thread_calls: RefCell<Vec<CreateThreadRequest>>,
    pub(crate) interaction_responses: RefCell<VecDeque<Result<(), AppError>>>,
    /// `(interaction_id, token, payload)` per `create_interaction_response`.
    pub(crate) interaction_calls: RefCell<Vec<InteractionCall>>,
    pub(crate) edit_components_responses: RefCell<VecDeque<Result<(), AppError>>>,
    /// `(channel_id, message_id, components)` per `edit_message_components`.
    pub(crate) edit_components_calls: RefCell<Vec<InteractionCall>>,
}

impl MockDiscordApi {
    pub(crate) fn new() -> Self {
        Self {
            send_responses: RefCell::new(VecDeque::new()),
            send_calls: RefCell::new(Vec::new()),
            get_responses: RefCell::new(VecDeque::new()),
            get_calls: RefCell::new(Vec::new()),
            thread_responses: RefCell::new(VecDeque::new()),
            thread_calls: RefCell::new(Vec::new()),
            interaction_responses: RefCell::new(VecDeque::new()),
            interaction_calls: RefCell::new(Vec::new()),
            edit_components_responses: RefCell::new(VecDeque::new()),
            edit_components_calls: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn with_get_responses(responses: Vec<Result<Vec<Message>, AppError>>) -> Self {
        let api = Self::new();
        *api.get_responses.borrow_mut() = responses.into_iter().collect();
        api
    }

    pub(crate) fn with_thread_responses(responses: Vec<Result<CreatedThread, AppError>>) -> Self {
        let api = Self::new();
        *api.thread_responses.borrow_mut() = responses.into_iter().collect();
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

    async fn create_thread(&self, req: &CreateThreadRequest) -> Result<CreatedThread, AppError> {
        self.thread_calls.borrow_mut().push(req.clone());
        self.thread_responses
            .borrow_mut()
            .pop_front()
            .expect("test must queue a thread response before calling create_thread")
    }

    async fn create_interaction_response(
        &self,
        interaction_id: &str,
        token: &str,
        payload: &serde_json::Value,
    ) -> Result<(), AppError> {
        self.interaction_calls.borrow_mut().push((
            interaction_id.to_owned(),
            token.to_owned(),
            payload.clone(),
        ));
        // Fire-and-forget callback: default to success so tests asserting only
        // the payload shape need not script an outcome for every call.
        self.interaction_responses
            .borrow_mut()
            .pop_front()
            .unwrap_or(Ok(()))
    }

    async fn edit_message_components(
        &self,
        channel_id: &str,
        message_id: &str,
        components: &serde_json::Value,
    ) -> Result<(), AppError> {
        self.edit_components_calls.borrow_mut().push((
            channel_id.to_owned(),
            message_id.to_owned(),
            components.clone(),
        ));
        self.edit_components_responses
            .borrow_mut()
            .pop_front()
            .unwrap_or(Ok(()))
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
