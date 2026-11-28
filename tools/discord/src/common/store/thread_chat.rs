//! Discord thread -> Orca chat mapping. The daemon forwards the first message
//! of a conversation to a new Orca chat and records the chat here, so later
//! messages in the same thread continue that chat instead of opening another.

use rusqlite::{OptionalExtension, params};

use crate::common::error::AppError;

use super::{AskStore, map_sqlite_err, normalize_utc};

/// v3 DDL. The key is the Discord thread id; a thread created from a message
/// shares that message's id, so a top-level message id works as the key of
/// the thread that will grow from it.
pub(super) const CREATE_THREAD_CHATS_SQL: &str = "CREATE TABLE IF NOT EXISTS thread_chats (
    thread_id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL,
    updated_at TEXT NOT NULL
)";

impl AskStore {
    /// The Orca chat that continues `thread_id`, if one was recorded.
    pub(crate) fn thread_chat(&self, thread_id: &str) -> Result<Option<String>, AppError> {
        self.conn
            .query_row(
                "SELECT chat_id FROM thread_chats WHERE thread_id = ?1",
                params![thread_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite_err)
    }

    /// Records (or replaces) the Orca chat that continues `thread_id`.
    pub(crate) fn set_thread_chat(
        &self,
        thread_id: &str,
        chat_id: &str,
        now: &str,
    ) -> Result<(), AppError> {
        let updated_at = normalize_utc(&self.conn, now)?;
        self.conn
            .execute(
                "INSERT INTO thread_chats (thread_id, chat_id, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(thread_id) DO UPDATE SET
                    chat_id = excluded.chat_id,
                    updated_at = excluded.updated_at",
                params![thread_id, chat_id, updated_at],
            )
            .map_err(map_sqlite_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: &str = "2026-11-25T01:02:03Z";

    #[test]
    fn unknown_thread_has_no_chat() {
        let store = AskStore::open_in_memory().unwrap();
        assert_eq!(store.thread_chat("t1").unwrap(), None);
    }

    #[test]
    fn recorded_chat_is_returned_for_its_thread_only() {
        let store = AskStore::open_in_memory().unwrap();
        store.set_thread_chat("t1", "chat-a", NOW).unwrap();

        assert_eq!(store.thread_chat("t1").unwrap().as_deref(), Some("chat-a"));
        assert_eq!(store.thread_chat("t2").unwrap(), None);
    }

    #[test]
    fn recording_again_replaces_the_chat() {
        let store = AskStore::open_in_memory().unwrap();
        store.set_thread_chat("t1", "chat-a", NOW).unwrap();
        store.set_thread_chat("t1", "chat-b", NOW).unwrap();

        assert_eq!(store.thread_chat("t1").unwrap().as_deref(), Some("chat-b"));
    }

    #[test]
    fn mapping_survives_reopening_the_file() {
        let path = std::env::temp_dir().join(format!(
            "areum-discord-thread-chat-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        AskStore::open(&path)
            .unwrap()
            .set_thread_chat("t1", "chat-a", NOW)
            .unwrap();

        let reopened = AskStore::open(&path).unwrap();
        assert_eq!(
            reopened.thread_chat("t1").unwrap().as_deref(),
            Some("chat-a")
        );
        std::fs::remove_file(&path).ok();
    }
}
