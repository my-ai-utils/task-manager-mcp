use my_http_utils::macros::{MyHttpInput, MyHttpObjectStructure};
use serde::{Deserialize, Serialize};

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.

// One column of a template, on the wire.
//
// Same three fields a board column has always had. `order` places it between Todo and Done, which are
// not in the list — they exist in every project by definition.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct ColumnTemplateColumn {
    pub id: String,
    pub name: String,
    pub description: String,
    pub order: i32,
}

// A named set of columns, defined once and assigned to any number of projects.
//
// Columns are configured HERE rather than per project. Two reasons it is worth the indirection: the
// projects on one board mostly share a workflow, so per-project columns meant typing the same four
// columns into every project and having them drift; and a template is a thing you can change in one
// place and have every project follow.
//
// `used_by` is how many projects currently point at this template. Derived, never stored — it is the
// number that says whether editing this is a small change or a large one.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct ColumnTemplateResponse {
    pub id: String,
    pub name: String,
    pub description: String,
    pub columns: Vec<ColumnTemplateColumn>,
    pub used_by: i32,
}

#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct ColumnTemplatesResponse {
    pub templates: Vec<ColumnTemplateResponse>,
}

// The whole template in one call — a SNAPSHOT, not a delta.
//
// There is deliberately no add-column / remove-column endpoint. The dialog that edits this builds the
// complete new list client-side and sends it, which means the server never has to reconcile a sequence
// of small writes, a half-finished edit is never visible to anyone, and Cancel costs nothing because
// nothing was sent.
//
// `id` empty creates; `id` naming an existing template replaces it.
#[derive(MyHttpInput)]
pub struct SaveColumnTemplateInputModel {
    #[http_body(name: "id", description: "Template id. Empty creates a new one", trim, to_lowercase)]
    pub id: String,
    #[http_body(name: "name", description: "Template name", trim)]
    pub name: String,
    #[http_body(name: "description", description: "What this set of columns is for", trim)]
    pub description: String,
    #[http_body(name: "columns", description: "The complete list of columns between Todo and Done")]
    pub columns: Vec<ColumnTemplateColumn>,
}

// Refused while any project still points at the template: a project whose template vanished would
// silently lose its board's middle columns, and every task sitting in one would read as Todo. Unassign
// them first, which makes the consequence something you chose rather than something you discovered.
#[derive(MyHttpInput)]
pub struct DeleteColumnTemplateInputModel {
    #[http_body(name: "id", description: "Template id")]
    pub id: String,
}
