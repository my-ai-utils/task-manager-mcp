use my_http_utils::macros::{MyHttpInput, MyHttpObjectStructure};
use serde::{Deserialize, Serialize};

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.

/// The one assignee that is not a person and has no user row.
///
/// A task carrying it is an agent's to do. Neutral on purpose — not the name of any particular model, so
/// it does not have to be renamed when whatever is doing the work changes.
///
/// Deliberately *not* a row in the roster: a row would have to be added to every project's membership to
/// be assignable, and disabling it would read as disabling a colleague. It is assignable on every board,
/// always, and `users_list` says so — that is how an agent learns it may put work on itself.
pub const ASSIGNEE_AI: &str = "AI";

/// Whether a value names the reserved assignee, whatever case it was written in.
///
/// Case-insensitive because it travels through MCP as free text: an agent writing `ai` means the same
/// thing as one writing `AI`, and silently creating an assignee nobody resolves would be the worse
/// reading.
pub fn is_ai_assignee(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case(ASSIGNEE_AI)
}

// One row of the roster.
//
// `admin` is the flag stored on the user. It is not the whole answer to "is this person an admin"
// — the admin list in the service settings is additive on top of it, which is what lets the very
// first person in on an empty database. `admin_from_settings` says the flag is not where this
// user's admin rights come from, so the UI can show it as granted-elsewhere rather than as an
// editable checkbox.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct UserResponse {
    pub email: String,
    pub name: String,
    pub disabled: bool,
    pub admin: bool,
    pub admin_from_settings: bool,
}

#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct UsersResponse {
    pub users: Vec<UserResponse>,
}

#[derive(MyHttpInput)]
pub struct CreateUserInputModel {
    #[http_body(name: "email", description: "Google account email", trim, to_lowercase)]
    pub email: String,
    #[http_body(name: "name", description: "Name shown on the board", trim)]
    pub name: String,
    #[http_body(name: "admin", description: "Grants access to every project and to setup")]
    pub admin: bool,
}

// The email is the identity and is not editable — a person whose address changed is a new row, and
// their old assignments deliberately keep pointing at the address that made them.
#[derive(MyHttpInput)]
pub struct UpdateUserInputModel {
    #[http_body(name: "email", description: "Google account email")]
    pub email: String,
    #[http_body(name: "name", description: "Name shown on the board", trim)]
    pub name: String,
    #[http_body(name: "admin", description: "Grants access to every project and to setup")]
    pub admin: bool,
    // Disabling denies sign-in. It does not touch the user's assignments or the authorship of
    // their comments — those are a record of what happened, not a live permission.
    #[http_body(name: "disabled", description: "Denies sign-in, keeps all history")]
    pub disabled: bool,
}
