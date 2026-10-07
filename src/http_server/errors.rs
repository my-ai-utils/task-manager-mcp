use service_sdk::my_http_server::HttpFailResult;

/// Every script in this service reports business failures as `String`, and every one of them is the
/// caller's mistake rather than a fault: a prefix that does not exist, a column that is not on this
/// board, an update that would change nothing. So the whole family maps here, with the message passed
/// through — the messages are written to be read by whoever caused them.
pub fn bad_request(message: impl Into<String>) -> HttpFailResult {
    HttpFailResult::as_validation_error(message.into())
}

/// 404 — the thing asked for is not there.
pub fn not_found(message: impl Into<String>) -> HttpFailResult {
    HttpFailResult::as_not_found(message.into(), false)
}

/// 401 — no session, an expired one, or a person who is no longer allowed in.
pub fn unauthorized(message: &str) -> HttpFailResult {
    HttpFailResult::as_unauthorized(Some(message))
}

/// 403 — signed in, but this is not theirs. Kept separate from 401 so a client can tell "sign in
/// again" from "ask for access".
pub fn forbidden(message: &str) -> HttpFailResult {
    HttpFailResult::as_forbidden(Some(message.to_string()))
}
