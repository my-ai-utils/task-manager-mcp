use my_http_utils::macros::{MyHttpInput, MyHttpObjectStructure};
use serde::{Deserialize, Serialize};

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.

/// Where the export is served.
///
/// A constant rather than a string in two places, because this one route is spelled by BOTH sides and they
/// spell it differently: the server routes on it, and the browser puts it in an `href` — there is no request
/// builder in between to keep them honest, since a download is a navigation and not a `fetch`.
pub const EXPORT_PROJECT_ROUTE: &str = "/api/projects/v1/export";

/// The url that downloads one project's export.
///
/// A GET with the prefix in the query, so it can be an `<a href download>`: the session is an `HttpOnly`
/// cookie, which the browser attaches to a navigation exactly as it does to a `fetch`, so the link carries no
/// token and the file arrives without a line of JavaScript.
///
/// Nothing is encoded because nothing in a prefix can need it — a prefix is ASCII letters, digits and
/// underscores, all of which are literal in a query value. See `is_valid_prefix`.
pub fn export_project_url(prefix: &str) -> String {
    format!("{EXPORT_PROJECT_ROUTE}?project={prefix}")
}

#[derive(MyHttpInput)]
pub struct ExportProjectInputModel {
    #[http_query(name: "project", description: "Which project to export, by prefix — RMS")]
    pub project: String,
}

/// The zip goes up as the RAW body, with the project in the query string.
///
/// **Not base64 in a JSON body**, which is how a single document and a document archive travel on this API.
/// Those are one file somebody picked; this is a whole board — every task, every comment and every document
/// it owns — and base64 would cost a third more on the wire and hold the encoded copy, the decoded copy and
/// the JSON around them in memory at once, for a payload that is already the largest thing this API carries.
/// A `#[http_body_raw]` field takes the body verbatim, and `#[http_query]` beside it is what leaves room to
/// still say which project it is for.
#[derive(MyHttpInput)]
pub struct ImportProjectInputModel {
    // The project the file is poured INTO. Import never creates one: who may see a board is configuration a
    // person sets up, and a file is not entitled to hand out access.
    #[http_query(name: "project", description: "Which project to import INTO, by prefix — RMS")]
    pub project: String,
    #[http_body_raw(description: "The export zip, as raw bytes")]
    pub content: Vec<u8>,
}

// One thing in the file that was not written, and why.
//
// **A first-class half of the answer rather than an error**, for the same reason a zip upload reports its
// skips: an export carries whatever the source project had, and one entry that this board cannot take —
// a document under the reserved `github/` root, a comment on a task that is not in the file — must not
// cost the other four hundred.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct SkippedImportEntryResponse {
    pub name: String,
    pub reason: String,
}

// What an import did.
//
// Counts of what landed, the entries that did not, and `notes` — the things that landed but read differently
// here than they did on the board they came from. A status naming a column this project has no template for
// is the common one: the task arrives, keeps the value it was exported with, and reads as Todo until the
// column exists. That is not a failure and not a silent success either, so it is said.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct ImportProjectResponse {
    pub goals: i32,
    pub tasks: i32,
    pub comments: i32,
    pub documents: i32,
    // Defaulted where the four above are required, and only for the changeover: a browser tab loaded
    // before releases existed is answered by a server that knows them and ignores the field, but a tab
    // loaded AFTER must still read the answer of a server that has not been rolled yet.
    #[serde(default)]
    pub releases: i32,
    #[serde(default)]
    pub skipped: Vec<SkippedImportEntryResponse>,
    #[serde(default)]
    pub notes: Vec<String>,
}
