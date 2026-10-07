use std::collections::HashSet;

use dioxus_utils::DataState;
use task_manager_shared::documents::{DocumentIndexEntryResponse, FindDocumentResponse};
use task_manager_shared::github::GithubConnectionResponse;
use task_manager_shared::projects::ProjectResponse;

/// The document browser, for one project at a time.
///
/// **Which document is SELECTED is deliberately not here: it lives in the url.** That is what makes a document
/// linkable — `/documents?selected=<id>` opens it, survives a reload and works with the back button — and it
/// is the pattern the file browser in `remote-development-mcp` uses, which this screen is modelled on.
///
/// What is below is the state that has no business being in an address: which project, which folders are
/// open, and the payload of whatever is being read.
#[derive(Default)]
pub struct DocumentsState {
    pub projects: DataState<Vec<ProjectResponse>>,
    /// Which project is being browsed, by PREFIX.
    ///
    /// The prefix rather than the id because everything this screen does with a project is spoken in one: the
    /// api calls take a prefix, and every raw url carries one. Translating an id at each of those would be a
    /// step to forget.
    pub selected_project: String,
    /// The project's whole index — paths, sizes and kinds, no payloads. One request rather than a fetch per
    /// folder: the server holds the index in memory, so the whole of it costs less than the round trips
    /// walking it one level at a time would.
    pub index: DataState<Vec<DocumentIndexEntryResponse>>,
    /// The project's connected GitHub repositories, with what each one's listing currently holds.
    ///
    /// **Separate from the index because a connection can exist while having no files**, and a folder in
    /// this product is derived from the paths of the documents in it — so a connection that is waiting
    /// for a key, still reading, or pointed at an empty folder produces no path and would be invisible.
    /// That is the most common state there is: a key lives only in the server's memory, so every deploy
    /// puts every private connection back into it.
    pub connections: DataState<Vec<GithubConnectionResponse>>,
    /// Which folders are drawn open, by full path. Mirrored into storage on every change.
    expanded: HashSet<String>,
    /// The folder the reader is WORKING IN — the last one they clicked, or the one holding the document they
    /// opened. Empty is the top level, which is also where somebody who has clicked nothing is.
    ///
    /// It exists for exactly one thing: an upload starts here rather than at the root. Nothing is drawn from
    /// it — a folder is not selectable, it opens and closes — so it is a memory of a gesture rather than a
    /// second selection competing with the one in the url.
    current_folder: String,
    /// The document being read. Only ever a text one — a file's bytes are fetched by the browser itself from
    /// the raw endpoint, and never pass through here.
    pub content: DataState<FindDocumentResponse>,
    /// Which document `content` holds. The selection lives in the url and can change without this state being
    /// touched, so the two are compared rather than assumed to agree — without it the pane would show the
    /// previous document under the new one's name.
    content_id: Option<String>,
    /// Markdown is shown rendered; this is the reader asking for the source it was rendered from. Kept across
    /// documents on purpose — somebody reading sources stays reading sources.
    pub show_source: bool,
}

impl DocumentsState {
    pub fn is_expanded(&self, path: &str) -> bool {
        self.expanded.contains(path)
    }

    /// Open or close one folder, and remember that this is where the reader is.
    ///
    /// Unlike the file browser this is modelled on, closing drops nothing: the whole index arrived in one
    /// response, so re-opening a folder costs no request and there is nothing to re-fetch.
    ///
    /// Closing marks the folder as current just as opening does — clicking a folder at all is the gesture
    /// that says "this one", and a reader who folds a tree back up to see it whole has not left it.
    pub fn toggle(&mut self, path: &str) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_string());
        }

        self.current_folder = path.to_string();
        self.persist_expanded();
    }

    /// Where an upload should start. See [`Self::current_folder`].
    ///
    /// **Never a mirrored folder.** Nothing can be written under the reserved root, so a reader who was
    /// last browsing a connected repository — which is exactly the reader about to press Sync — is
    /// offered the top level rather than a destination the server would refuse.
    pub fn current_folder(&self) -> &str {
        if task_manager_shared::github::is_github_path(&self.current_folder) {
            return "";
        }

        &self.current_folder
    }

    /// Adopt the folder of a document that was opened — including one arrived at by a link, which is the case
    /// no click can cover.
    pub fn enter_folder_of(&mut self, document_path: &str) {
        self.current_folder = match document_path.rsplit_once('/') {
            Some((folder, _)) => folder.to_string(),
            // A document at the top level. Not "leave it alone": the reader IS at the top level.
            None => String::new(),
        };
    }

    /// Switch projects, dropping everything that belonged to the old one — a path from one project names
    /// nothing in another. The open folders are not cleared but swapped for the ones this project was left
    /// with.
    pub fn select_project(&mut self, prefix: String) {
        if self.selected_project == prefix {
            return;
        }

        self.expanded = crate::web::storage::get_expanded_folders(&prefix);
        self.selected_project = prefix;
        // A folder from the old board names nothing on the new one.
        self.current_folder = String::new();
        self.index.reset();
        self.connections.reset();
        self.content.reset();
        self.content_id = None;
    }

    /// Adopt a project and the folders it was left open at, plus every folder on the way down to whatever the
    /// url points at — so a link to a document lands ON the document rather than on a collapsed tree with it
    /// somewhere inside.
    pub fn adopt_project(&mut self, prefix: String, selected_path: &str) {
        self.expanded = crate::web::storage::get_expanded_folders(&prefix);
        self.expanded.extend(ancestors_of(selected_path));
        self.selected_project = prefix;
    }

    /// Open every folder on the way down to a path, for a selection that arrived after the first render —
    /// following a reference from a task, say.
    pub fn reveal(&mut self, selected_path: &str) {
        let before = self.expanded.len();
        self.expanded.extend(ancestors_of(selected_path));

        if self.expanded.len() != before {
            self.persist_expanded();
        }
    }

    /// Point this screen at whatever a selection names, WITHOUT losing the folders somebody left open.
    ///
    /// **A reference carries its own board, and following one has to land on that board.** Before it did
    /// not: the screen opened on whichever project was remembered, so a document reference followed from a
    /// task on another one landed on a tree the document was not in — and the reader was told the document
    /// did not exist. Nothing about that was visible, because both halves looked right on their own.
    ///
    /// A selection naming no project — a bare id, which is what a reference stored before this vocabulary
    /// was one — leaves the project alone, since it names nothing better than what is already open.
    pub fn follow(&mut self, selected: &str) {
        let reference = task_manager_shared::documents::parse_document_reference(selected);

        if let Some(reference) = reference.as_ref()
            && !reference
                .project()
                .eq_ignore_ascii_case(&self.selected_project)
        {
            self.select_project(reference.project().to_string());
        }

        // A mirrored path is what the tree is drawn from, so revealing it is what opens the repository and
        // the folders inside it. An id reveals nothing and never did — a document of the project's own has
        // its folder opened by `enter_folder_of` once the read comes back and its path is known.
        match reference {
            Some(task_manager_shared::documents::DocumentReference::Mirror { path, .. }) => {
                self.reveal(&path)
            }
            _ => self.reveal(selected),
        }
    }

    /// Whether `content` holds this document rather than one read before it.
    pub fn content_is_for(&self, id: &str) -> bool {
        self.content_id.as_deref() == Some(id)
    }

    /// Claim `content` for a document and mark it in flight — both in one write, so no render sees the slot
    /// claimed while it still holds the previous document.
    pub fn begin_content_load(&mut self, id: &str) {
        self.content_id = Some(id.to_string());
        self.content.set_loading();
    }

    pub fn toggle_source(&mut self) {
        self.show_source = !self.show_source;
    }

    /// Re-read the index. The one refresh gesture the screen has: there is no WebSocket for documents — the
    /// socket carries the board, and payloads must not ride along — so a document an agent uploads while this
    /// is open does not appear on its own.
    pub fn refresh(&mut self) {
        self.index.reset();
        // Read again as well: a connection's state moves without any document changing — a listing
        // finishing, a key arriving, a repository going unreachable — and the tree draws that state.
        self.connections.reset();
        self.content.reset();
        self.content_id = None;
    }

    /// Re-read the connections only. What a pull asks for: the documents have not changed, the
    /// connection's state has.
    pub fn refresh_connections(&mut self) {
        self.connections.reset();
    }

    /// What the reserved root is CALLED on screen — `github@<account>` when every connection is that
    /// account's, and a plain `github` when they are not.
    ///
    /// **A label, never a path.** `github/` is the first segment of every mirrored path, of every
    /// mirrored id, of the guard that refuses writes there and of what the MCP tools promise an agent.
    /// Putting an account in it would rename several hundred documents every time somebody connected a
    /// second repository. So the tree says whose it is and the address stays what it was.
    ///
    /// Falls back to the bare name with more than one account rather than listing them: at that point
    /// the root does not name an account, and the row that does is each connection's own.
    /// The tag beside a connection's name, saying whose account it is — `github@mxtm-po`.
    ///
    /// **It replaced a label on a `github` root that no longer exists**, and it says more than that one
    /// could: the root had to hedge, falling back to the bare word whenever two connections came from
    /// different accounts, because one row cannot name two owners. A tag on the connection itself always
    /// names exactly the owner of that repository, so a project connecting one repository from a company
    /// account and another from somebody's own reads correctly on both rows.
    ///
    /// `None` when the connections have not loaded yet, or when the stored `owner/repo` has no owner in
    /// it — a tag reading `github@` would be a label with a hole in it.
    pub fn connection_tag(&self, name: &str) -> Option<String> {
        let owner = self.connection(name)?.repo.split('/').next()?.trim();

        match owner.is_empty() {
            true => None,
            false => Some(format!("{}@{owner}", task_manager_shared::github::GITHUB_ROOT)),
        }
    }

    /// One connection by name, or `None` while the list is still loading.
    pub fn connection(&self, name: &str) -> Option<&GithubConnectionResponse> {
        self.connections
            .as_ref()
            .try_unwrap_as_loaded()
            .and_then(|itm| itm.iter().find(|c| c.name == name))
    }

    fn persist_expanded(&self) {
        if !self.selected_project.is_empty() {
            crate::web::storage::set_expanded_folders(&self.selected_project, &self.expanded);
        }
    }
}

/// Every folder on the way down to a document — `docs` and `docs/design` for `docs/design/system.md`. The
/// document itself is not one of them, and neither is the root, which is always drawn open.
fn ancestors_of(path: &str) -> Vec<String> {
    path.char_indices()
        .filter(|(_, character)| *character == '/')
        .map(|(at, _)| path[..at].to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_names_every_folder_above_it() {
        assert_eq!(
            ancestors_of("docs/design/system.md"),
            vec!["docs", "docs/design"]
        );
    }

    /// Where an upload starts. Clicking a folder is the gesture that says "this one", and folding it back up
    /// is not leaving it.
    #[test]
    fn clicking_a_folder_is_being_in_it() {
        let mut state = DocumentsState::default();

        state.toggle("docs/design");
        assert_eq!(state.current_folder(), "docs/design");

        state.toggle("docs/design");
        assert_eq!(state.current_folder(), "docs/design");
    }

    /// The other half: a document that was opened — by a click or by a link — puts the reader in ITS folder.
    #[test]
    fn opening_a_document_is_being_in_its_folder() {
        let mut state = DocumentsState::default();

        state.enter_folder_of("docs/design/system.md");
        assert_eq!(state.current_folder(), "docs/design");

        // A document at the top level says the top level, rather than leaving the previous folder standing.
        state.enter_folder_of("readme.md");
        assert_eq!(state.current_folder(), "");
    }

    #[test]
    fn a_document_at_the_root_names_none() {
        assert!(ancestors_of("notes.md").is_empty());
        assert!(ancestors_of("").is_empty());
    }
}
