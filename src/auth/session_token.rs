use encryption::AesEncryptedDataOwned;
use encryption::aes::AesKey;
use rust_extensions::date_time::DateTimeAsMicroseconds;

/// How long a session token is good for.
/// How long a session lasts. `pub(crate)` because the cookie carrying it has to expire with it — a cookie
/// outliving its token leaves the browser sending a credential the server has stopped accepting.
pub(crate) const SESSION_TTL_HOURS: i64 = 12;

/// How long a sign-in may take between leaving for Google and coming back.
///
/// Half an hour, not the ten minutes this started at. Ten is plenty for somebody already signed into
/// Google — one tap on the account chooser — and far too little for the case that actually needs the
/// room: a first sign-in, where the person may have to sign into Google itself, pass 2FA, and read the
/// consent screen, often after switching out of an in-app browser into a real one. Those minutes are
/// spent on Google's side, and running out of them here surfaces as "expired" on a sign-in that never
/// did anything wrong. Nothing is protected by the window being short: replay is stopped by the `code`
/// being single-use, not by this.
const LOGIN_STATE_TTL_SECONDS: i64 = 1800;

/// A session, carried entirely inside the token.
///
/// Nothing is stored server-side: the token *is* the session, protobuf-encoded and encrypted with the
/// key from settings. Three things follow from that, and all three are the reason for it:
///
/// * a restart does not sign anyone out — the key outlives the process;
/// * there is no session table and no session map to keep in sync;
/// * a second instance would accept tokens issued by the first, so nothing here is what pins this
///   service to one instance.
///
/// The cost is that a token cannot be revoked server-side, so `logout` is the client dropping it. Two
/// things make that acceptable: the TTL is short, and every request re-reads the user row — so
/// disabling somebody locks them out immediately, which is the case revocation would actually be for.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct SessionToken {
    #[prost(string, tag = "1")]
    pub email: ::prost::alloc::string::String,
    /// Unix seconds. Checked on every parse.
    #[prost(int64, tag = "2")]
    pub expires: i64,
}

impl SessionToken {
    pub fn issue(email: String) -> Self {
        let mut expires = DateTimeAsMicroseconds::now();
        expires.add_hours(SESSION_TTL_HOURS);

        Self {
            email,
            expires: expires.unix_microseconds / 1_000_000,
        }
    }

    pub fn to_token(&self, aes_key: &AesKey) -> String {
        let mut as_bytes = Vec::new();
        prost::Message::encode(self, &mut as_bytes)
            .expect("session token: protobuf encode cannot fail");

        aes_key.encrypt(&as_bytes).as_base_64()
    }

    /// Read a token back.
    ///
    /// `None` for anything that is not a token this service issued and that is still valid — a garbled
    /// string, one encrypted with a different key, or an expired one. The caller turns all of them into
    /// the same 401: from the browser's side they all mean "sign in again".
    pub fn parse(token: &str, aes_key: &AesKey) -> Option<Self> {
        let encrypted = AesEncryptedDataOwned::from_base_64(token).ok()?;
        let decrypted = aes_key.decrypt(&encrypted).ok()?;
        let session: Self = prost::Message::decode(decrypted.as_slice()).ok()?;

        if session.email.trim().is_empty() {
            return None;
        }

        let now = DateTimeAsMicroseconds::now().unix_microseconds / 1_000_000;

        if session.expires <= now {
            return None;
        }

        Some(session)
    }
}

/// The CSRF `state` handed to Google, also carried entirely inside itself.
///
/// A `nonce` so two sign-ins started at the same second are different strings, and an expiry so a
/// state cannot be used a week later.
///
/// Honest limitation: a stateless state cannot detect a replay — there is no record of it having been
/// used. What actually stops a replayed callback is Google: the `code` it accompanies is single-use, so
/// a second attempt with the same pair fails at the token exchange. The state's job here is to prove
/// the callback belongs to a sign-in *this* deployment started, and to bound how long that is true for.
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct LoginState {
    #[prost(string, tag = "1")]
    pub nonce: ::prost::alloc::string::String,
    #[prost(int64, tag = "2")]
    pub expires: i64,
}

impl LoginState {
    pub fn issue() -> Self {
        let mut expires = DateTimeAsMicroseconds::now();
        expires.add_seconds(LOGIN_STATE_TTL_SECONDS);

        Self {
            nonce: uuid::Uuid::new_v4().to_string(),
            expires: expires.unix_microseconds / 1_000_000,
        }
    }

    pub fn to_token(&self, aes_key: &AesKey) -> String {
        let mut as_bytes = Vec::new();
        prost::Message::encode(self, &mut as_bytes)
            .expect("login state: protobuf encode cannot fail");

        aes_key.encrypt(&as_bytes).as_base_64()
    }

    /// Check a state that came back from Google.
    ///
    /// The two failure modes are kept apart rather than collapsed into one `false`. They are the same
    /// 401 to the caller, but they mean opposite things when somebody reports a sign-in they could not
    /// complete: `Expired` is a flow that took too long or a link that had been sitting around, while
    /// `NotOurs` is a state that never decrypted — a mangled query parameter, a different deployment's
    /// key, or an invention. Without the distinction the only way to tell them apart is to guess.
    pub fn check(token: &str, aes_key: &AesKey) -> Result<(), LoginStateProblem> {
        let Ok(encrypted) = AesEncryptedDataOwned::from_base_64(token) else {
            return Err(LoginStateProblem::NotOurs);
        };

        let Ok(decrypted) = aes_key.decrypt(&encrypted) else {
            return Err(LoginStateProblem::NotOurs);
        };

        let Ok(state) = <Self as prost::Message>::decode(decrypted.as_slice()) else {
            return Err(LoginStateProblem::NotOurs);
        };

        if state.nonce.is_empty() {
            return Err(LoginStateProblem::NotOurs);
        }

        let now = DateTimeAsMicroseconds::now().unix_microseconds / 1_000_000;

        if state.expires <= now {
            return Err(LoginStateProblem::Expired);
        }

        Ok(())
    }
}

/// Why a callback's `state` was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginStateProblem {
    /// Did not read back with our key at all.
    NotOurs,
    /// Ours, but issued more than [`LOGIN_STATE_TTL_SECONDS`] ago.
    Expired,
}

impl LoginStateProblem {
    /// What to say to the person. Two different sentences, because the thing they should do next is
    /// different: one is "you were too slow", the other is "the link you used was not a sign-in".
    pub fn as_message(&self) -> &'static str {
        match self {
            Self::NotOurs => {
                "That sign-in link was not one we issued — start again from the sign-in page"
            }
            Self::Expired => "The sign-in took too long — start again",
        }
    }

    /// What to put in the log.
    pub fn as_log_reason(&self) -> &'static str {
        match self {
            Self::NotOurs => "state did not decrypt",
            Self::Expired => "state expired",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `AesKey::new` panics on any other length — 48 bytes exactly.
    fn key() -> AesKey {
        AesKey::new(b"0123456789abcdef0123456789abcdef0123456789abcdef")
    }

    #[test]
    fn a_session_round_trips_through_its_token() {
        let issued = SessionToken::issue("yuri@mxtm.ai".to_string());
        let token = issued.to_token(&key());

        let parsed = SessionToken::parse(&token, &key()).expect("should parse");

        assert_eq!(parsed.email, "yuri@mxtm.ai");
        assert_eq!(parsed.expires, issued.expires);
    }

    /// The whole security of a stateless token rests on this: a token encrypted with another key must
    /// not read back. If it did, anyone could mint themselves a session for any email.
    #[test]
    fn a_token_from_another_key_does_not_read() {
        let token = SessionToken::issue("yuri@mxtm.ai".to_string()).to_token(&key());
        let other = AesKey::new(b"fedcba9876543210fedcba9876543210fedcba9876543210");

        assert!(SessionToken::parse(&token, &other).is_none());
    }

    #[test]
    fn an_expired_session_does_not_read() {
        let expired = SessionToken {
            email: "yuri@mxtm.ai".to_string(),
            expires: 1,
        };

        let token = expired.to_token(&key());

        assert!(SessionToken::parse(&token, &key()).is_none());
    }

    #[test]
    fn garbage_does_not_read() {
        for token in ["", "not-base64!!", "aGVsbG8="] {
            assert!(
                SessionToken::parse(token, &key()).is_none(),
                "{token:?} should not read"
            );
        }
    }

    #[test]
    fn a_login_state_round_trips_and_an_expired_one_is_refused() {
        let token = LoginState::issue().to_token(&key());
        assert!(LoginState::check(&token, &key()).is_ok());

        let expired = LoginState {
            nonce: "n".to_string(),
            expires: 1,
        }
        .to_token(&key());
        assert_eq!(
            LoginState::check(&expired, &key()),
            Err(LoginStateProblem::Expired)
        );

        assert_eq!(
            LoginState::check("nonsense", &key()),
            Err(LoginStateProblem::NotOurs)
        );
    }

    /// The distinction is the whole point of the enum: a state encrypted with somebody else's key must
    /// not be reported as "expired", or the next person to report a failed sign-in gets told to hurry
    /// up when the real problem is that their state never was ours.
    #[test]
    fn a_state_from_another_key_is_not_reported_as_expired() {
        let token = LoginState::issue().to_token(&key());
        let other = AesKey::new(b"fedcba9876543210fedcba9876543210fedcba9876543210");

        assert_eq!(
            LoginState::check(&token, &other),
            Err(LoginStateProblem::NotOurs)
        );
    }

    /// A first sign-in has to survive signing into Google, 2FA and the consent screen. Ten minutes did
    /// not cover that, which is what this pins.
    #[test]
    fn the_login_window_is_half_an_hour() {
        let issued = LoginState::issue();
        let now = DateTimeAsMicroseconds::now().unix_microseconds / 1_000_000;

        assert!(issued.expires - now >= 1790);
    }

    /// Two sign-ins started in the same second must produce different states, or one browser's callback
    /// would validate against another's.
    #[test]
    fn two_login_states_differ() {
        assert_ne!(LoginState::issue().nonce, LoginState::issue().nonce);
    }
}
