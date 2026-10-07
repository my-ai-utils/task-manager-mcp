use my_http_utils::macros::MyHttpObjectStructure;
use serde::{Deserialize, Serialize};

// Never put `///` doc comments on fields of a struct deriving MyHttpObjectStructure: the macro's
// attribute parser panics with `Somehow we got Punct here: =`.

// What this deployment is configured with, for the Settings screen.
//
// Read-only by design. The Google credentials and the admin list live in the service settings rather
// than in the database — that is what lets the service authenticate on a cold start — so there is
// nothing here to edit. The screen exists because the first question when a sign-in fails is which
// client id was picked up and which redirect URI is expected, and reading that beats guessing from
// logs.
//
// `google_client_id` is returned in full: it is not a secret, and a truncated one cannot be compared
// against the Google console, which is the whole point. The client secret is never returned.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct DiagnosticsResponse {
    pub google_client_id: String,
    pub google_redirect_uri: String,
    pub google_configured: bool,
    pub admins_in_settings: i32,
    pub projects_amount: i32,
    pub users_amount: i32,
    pub connected_homes: i32,
}
