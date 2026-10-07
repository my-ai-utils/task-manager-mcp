use task_manager_shared::ws::{BoardSnapshot, ServerWsPayload};

/// What the server pushes down `/ws`.
///
/// **A change arrives as the board, not as a hint that it changed.** The client replaces what it is showing
/// with what came down the socket — no request, no `Loading…`, nothing on the screen moves except the cards
/// that are actually different. Re-reading was the previous design and it cost a spinner on every push, which
/// on a board an agent touches every few seconds is most of the time.
///
/// A snapshot rather than a delta: a delta is only correct if the client's copy is, and wholesale replacement
/// cannot drift. The board is one project's worth of tasks and is read in full by the REST call anyway, so
/// there is nothing to save by sending less.
///
/// `ProjectChanged` is what is left of the old protocol and is still honoured — the server sends it alongside
/// a snapshot, and on its own when it has no project in memory to build one from. It means "re-read", which
/// is the one thing that always works.
pub enum ServerWsMessage {
    BoardSnapshot(BoardSnapshot),
    ProjectChanged,
    Error(String),
    Unknown(String),
}

impl ServerWsMessage {
    pub fn parse(raw: &str) -> Self {
        let Ok(payload) = serde_json::from_str::<ServerWsPayload>(raw) else {
            return Self::Unknown(raw.to_string());
        };

        // The snapshot first: the server sends both keys on one message, and the snapshot is the one that
        // does not cost a round trip.
        if let Some(snapshot) = payload.board_snapshot {
            return Self::BoardSnapshot(snapshot);
        }

        if payload.project_changed.is_some() {
            return Self::ProjectChanged;
        }

        if let Some(error) = payload.error {
            return Self::Error(error);
        }

        Self::Unknown(raw.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One message carries both keys during the changeover. Reading the wrong one would put the spinner back.
    #[test]
    fn a_snapshot_wins_over_the_signal_beside_it() {
        let raw = r#"{"projectChanged":"RMS","boardSnapshot":{"project":"RMS","tasks":[]}}"#;

        match ServerWsMessage::parse(raw) {
            ServerWsMessage::BoardSnapshot(snapshot) => {
                assert_eq!(snapshot.project, "RMS");
                assert!(snapshot.tasks.is_empty());
            }
            _ => panic!("expected a snapshot"),
        }
    }

    /// A server with no project in memory sends the signal alone, and it still has to mean "re-read".
    #[test]
    fn the_signal_alone_is_still_understood() {
        assert!(matches!(
            ServerWsMessage::parse(r#"{"projectChanged":"RMS"}"#),
            ServerWsMessage::ProjectChanged
        ));
    }

    #[test]
    fn an_error_is_read_and_anything_else_is_not_guessed_at() {
        match ServerWsMessage::parse(r#"{"error":"no access to this project"}"#) {
            ServerWsMessage::Error(message) => assert_eq!(message, "no access to this project"),
            _ => panic!("expected an error"),
        }

        assert!(matches!(
            ServerWsMessage::parse("not json"),
            ServerWsMessage::Unknown(_)
        ));
        assert!(matches!(
            ServerWsMessage::parse("{}"),
            ServerWsMessage::Unknown(_)
        ));
    }
}
