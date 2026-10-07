use my_http_utils::macros::{MyHttpInput, MyHttpObjectStructure};
use serde::{Deserialize, Serialize};

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.

// One task type of a template, on the wire.
//
// Called a "task type" on screen and a `kind` in the code, the API and the MCP tools. Deliberate: the
// wire name is a contract with every agent already calling `tasks_create` with `kind`.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct KindTemplateKind {
    pub id: String,
    pub name: String,
    pub description: String,
    pub color: String,
    // The name of an icon, without extension — `bug`, `tech-debt`. Empty for none.
    //
    // A plain string, not an enum, for the same reason a colour is one: the icons are files in the UI
    // bundle, so a name this build has never heard of must render as no icon rather than fail a read of
    // the whole board.
    #[serde(default)]
    pub icon: String,
}

// A named set of task types, defined once and assigned to any number of projects.
//
// Exactly the shape column templates have, for the same reason: the projects on one board mostly share a
// vocabulary of work, so per-project types meant retyping "bug, feature, chore" into every project and
// watching the descriptions drift. A template is a thing you change once and have every project follow.
//
// `used_by` is how many projects currently point at this template. Derived, never stored — it is the
// number that says whether editing this is a small change or a large one.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct KindTemplateResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kinds: Vec<KindTemplateKind>,
    pub used_by: i32,
}

#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct KindTemplatesResponse {
    pub templates: Vec<KindTemplateResponse>,
}

// The whole template in one call — a SNAPSHOT, not a delta.
//
// No add-type / remove-type endpoint: the dialog builds the complete list client-side and sends it, so
// the server never reconciles a sequence of small writes, a half-finished edit is never visible, and
// Cancel costs nothing because nothing was sent.
//
// `id` empty creates; `id` naming an existing template replaces it.
#[derive(MyHttpInput)]
pub struct SaveKindTemplateInputModel {
    #[http_body(name: "id", description: "Template id. Empty creates a new one", trim, to_lowercase)]
    pub id: String,
    #[http_body(name: "name", description: "Template name", trim)]
    pub name: String,
    #[http_body(name: "description", description: "What this set of task types is for", trim)]
    pub description: String,
    #[http_body(name: "kinds", description: "The complete list of task types")]
    pub kinds: Vec<KindTemplateKind>,
}

// Refused while any project still points at the template. Unlike a column, a task type vanishing is not
// destructive — a task pointing at one that is gone simply reads as having no type — but it is still a
// change to every project at once, so it is made deliberately rather than discovered.
#[derive(MyHttpInput)]
pub struct DeleteKindTemplateInputModel {
    #[http_body(name: "id", description: "Template id")]
    pub id: String,
}
