use my_http_utils::macros::{MyHttpInput, MyHttpObjectStructure};
use serde::{Deserialize, Serialize};

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.

/// Where the templates export is served.
///
/// A constant rather than a string in two places, for the same reason the project export has one: this
/// route is spelled by BOTH sides — the server routes on it, the browser puts it in an `href` — and a
/// download is a navigation, so there is no request builder in between to keep them honest.
pub const EXPORT_TEMPLATES_ROUTE: &str = "/api/templates/v1/export";

/// The url that downloads every template on the instance.
///
/// No arguments: templates are not scoped to anything, so there is nothing to name. Kept as a function
/// beside the route anyway, so the browser side reads the same as the project one does.
pub fn export_templates_url() -> String {
    EXPORT_TEMPLATES_ROUTE.to_string()
}

/// The YAML goes up as the RAW body.
///
/// A templates file is a few kilobytes, so this is not about size the way the project import is — it is
/// about the file being the artefact. What the reader picked is what the server parses, byte for byte,
/// with nothing re-encoding it on the way, which is what makes "edit the yaml by hand and import it"
/// a thing that behaves the way it looks.
#[derive(MyHttpInput)]
pub struct ImportTemplatesInputModel {
    #[http_body_raw(description: "The templates export, as YAML")]
    pub content: Vec<u8>,
}

// One template in the file that was not applied, and why.
//
// Its own type rather than a reuse of the project import's: the two features are separate and have no
// reason to move together, and a shared response model would make a change to one silently rewrite the
// other's contract.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct SkippedTemplateResponse {
    pub name: String,
    pub reason: String,
}

// What a templates import did.
//
// Created and replaced are counted apart, and that is the number that matters: creating a template
// affects nothing, where replacing one changes every project that follows it in the same write. `notes`
// says how many projects that was, per template.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct ImportTemplatesResponse {
    pub column_templates_created: i32,
    pub column_templates_replaced: i32,
    pub kind_templates_created: i32,
    pub kind_templates_replaced: i32,
    #[serde(default)]
    pub skipped: Vec<SkippedTemplateResponse>,
    #[serde(default)]
    pub notes: Vec<String>,
}
