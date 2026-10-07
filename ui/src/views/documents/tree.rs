use dioxus::prelude::*;
use task_manager_shared::documents::{
    DocumentIndexEntryResponse, canonical_document_reference, document_file_name, render_size,
};
use task_manager_shared::github::{GITHUB_ROOT, GithubMirrorState, is_github_path};

use super::{
    DocumentsState, connection_note, file_icon, folder_icon, github_icon, remote_folder_icon,
};

/// How far one level is pushed in, in pixels.
const INDENT: usize = 14;

/// One node of the tree: a folder with children, or a document.
///
/// **Built from the index on every render, and stored nowhere.** That is not a shortcut, it is the model: a
/// folder does not exist on the server either — it is read off the paths of the documents in it. So an empty
/// folder cannot appear here, and the last document leaving one makes it vanish with nothing to clean up.
#[derive(Clone, PartialEq)]
pub enum DocumentNode {
    Folder {
        /// The folder's own name — the last segment, which is what a row shows.
        name: String,
        /// The full path from the root, which is what the open/closed set is keyed by: two folders can be
        /// called `design`, and folding one must not fold the other.
        path: String,
        children: Vec<DocumentNode>,
    },
    Document {
        name: String,
        entry: DocumentIndexEntryResponse,
    },
}

impl DocumentNode {
    /// Folders before documents, each half alphabetical.
    ///
    /// Folders first because that is what a file tree does everywhere and a reader scans for structure before
    /// contents; alphabetical within each half because any other order is one nobody can predict.
    fn sort_key(&self) -> (u8, String) {
        match self {
            Self::Folder { name, .. } => (0, name.to_lowercase()),
            Self::Document { name, .. } => (1, name.to_lowercase()),
        }
    }
}

/// Turn a flat index into the tree.
///
/// The entries arrive sorted by path — the server sorts them — but nothing here relies on it: each path is
/// walked segment by segment into the structure and every level is sorted afterwards, so an index in any order
/// draws the same tree.
pub fn build_tree(entries: &[DocumentIndexEntryResponse]) -> Vec<DocumentNode> {
    let mut roots: Vec<DocumentNode> = Vec::new();

    for entry in entries {
        let mut segments: Vec<&str> = entry
            .path
            .split('/')
            .filter(|itm| !itm.is_empty())
            .collect();

        // The file name comes off the end; what is left is the folders it sits in. A path with nothing in it is
        // skipped rather than drawn as a nameless row — the server refuses to store one, so this only guards
        // against a shape nobody predicted.
        if segments.pop().is_none() {
            continue;
        }

        // **A connected repository is ONE row, not a `github` folder with a repository inside it.** The
        // reserved segment is dropped from the WALK and handed to `insert` as the ground it starts from,
        // which is what collapses two levels into one without touching the path: `github/analytics/docs/a.md`
        // builds a node named `analytics` whose path is still `github/analytics`, and everything below it
        // carries on as normal.
        //
        // The path must not change, and that is the whole reason it is done this way rather than by rewriting
        // it: `github/<connection>/<file>` is what the server reads, writes and refuses by, what the open/closed
        // set is keyed by, and what `folder_connection` reads a connection's name back out of.
        let (folders, walked) = match segments.split_first() {
            Some((&first, rest)) if first == GITHUB_ROOT => (rest, GITHUB_ROOT),
            _ => (&segments[..], ""),
        };

        insert(&mut roots, folders, walked, entry);
    }

    sort_level(&mut roots);
    roots
}

/// Walk one document into the tree, one folder per call.
///
/// Recursive rather than a loop over a `&mut` cursor, which is what the borrow checker has to say about
/// descending into a vector you are also pushing to — and the recursion is bounded by the depth of a path.
fn insert(
    level: &mut Vec<DocumentNode>,
    folders: &[&str],
    walked: &str,
    entry: &DocumentIndexEntryResponse,
) {
    let Some((head, rest)) = folders.split_first() else {
        level.push(DocumentNode::Document {
            name: document_file_name(&entry.path).to_string(),
            entry: entry.clone(),
        });

        return;
    };

    let path = if walked.is_empty() {
        head.to_string()
    } else {
        format!("{walked}/{head}")
    };

    // Found or created, in that order: two documents under `docs/` must land in ONE `docs` node, or the tree
    // would draw the folder twice with half the files in each.
    let position = level.iter().position(|node| match node {
        DocumentNode::Folder { path: existing, .. } => existing == &path,
        DocumentNode::Document { .. } => false,
    });

    let position = match position {
        Some(position) => position,
        None => {
            level.push(DocumentNode::Folder {
                name: head.to_string(),
                path: path.clone(),
                children: Vec::new(),
            });

            level.len() - 1
        }
    };

    // `if let` rather than an unwrap: the entry was found or created as a folder one statement ago, and a
    // panic here would blank the whole screen over one path.
    if let DocumentNode::Folder { children, .. } = &mut level[position] {
        insert(children, rest, &path, entry);
    }
}

fn sort_level(level: &mut Vec<DocumentNode>) {
    level.sort_by_key(|node| node.sort_key());

    for node in level.iter_mut() {
        if let DocumentNode::Folder { children, .. } = node {
            sort_level(children);
        }
    }
}

/// Put a folder in the tree for every connected repository, whether or not it has files.
///
/// **Without this a connection is invisible in exactly the states somebody needs to see it.** Folders
/// here come from the paths of the documents in them, so a connection with an empty listing produces
/// nothing — and "waiting for a key", "still reading" and "the folder you named has nothing in it" all
/// have empty listings. The reader would press Refresh, see no change, and have nothing to go on.
///
/// A connection that DOES have files is already in the tree, built from its paths; this only adds what
/// is missing, so nothing is drawn twice.
///
/// **At the TOP level, beside the project's own folders**, because a connected repository is a place work
/// happens rather than an entry in a directory of repositories. There is no `github` node any more: it was
/// a level that carried one word and cost every reader a click to get past, and what it used to say — which
/// account this is — now sits on the connection's own row where it names one repository instead of hedging
/// across all of them.
pub fn merge_github_connections(nodes: &mut Vec<DocumentNode>, connections: &[String]) {
    if connections.is_empty() {
        return;
    }

    for name in connections {
        let path = format!("{GITHUB_ROOT}/{name}");

        let present = nodes.iter().any(|node| match node {
            DocumentNode::Folder { path: existing, .. } => existing == &path,
            DocumentNode::Document { .. } => false,
        });

        if !present {
            nodes.push(DocumentNode::Folder {
                name: name.clone(),
                path,
                children: Vec::new(),
            });
        }
    }

    sort_level(nodes);
}

/// The connection a folder path names, when it names one — `github/<name>` and nothing deeper.
///
/// Derived from the path rather than carried on the node, because the path already says it: a node two
/// segments under the reserved root IS a connection, and a third field agreeing with the path would only
/// be a thing to keep in step.
pub fn folder_connection(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(GITHUB_ROOT)?.strip_prefix('/')?;

    if rest.is_empty() || rest.contains('/') {
        return None;
    }

    Some(rest)
}

/// Every folder path in a tree, outermost first, for offering as a place to put something.
///
/// Off the tree rather than off the raw paths, so it is exactly the set of folders the reader can see — an
/// upload dialog offering a folder that is not drawn would be offering a folder that does not exist.
pub fn all_folder_paths(nodes: &[DocumentNode]) -> Vec<String> {
    let mut paths = Vec::new();
    collect_folders(nodes, &mut paths);
    paths
}

fn collect_folders(nodes: &[DocumentNode], into: &mut Vec<String>) {
    for node in nodes {
        if let DocumentNode::Folder { path, children, .. } = node {
            into.push(path.clone());
            collect_folders(children, into);
        }
    }
}

/// The document the url is pointing at, or `""` when it points at none.
fn selected_id() -> String {
    match use_route::<crate::AppRoute>() {
        crate::AppRoute::Documents { selected } => selected,
        _ => String::new(),
    }
}

/// One level of the tree.
///
/// Split from [`DocumentTreeRow`] so the recursion goes through two components rather than one calling itself
/// — the same shape the file browser this screen is modelled on uses.
#[component]
pub fn DocumentNodes(
    nodes: Vec<DocumentNode>,
    depth: usize,
    cs: Signal<DocumentsState>,
) -> Element {
    rsx! {
        for node in nodes.iter() {
            DocumentTreeRow {
                key: "{node_key(node)}",
                node: node.clone(),
                depth,
                cs,
            }
        }
    }
}

/// A key that survives a rebuild: a folder by its path, a document by its id.
fn node_key(node: &DocumentNode) -> String {
    match node {
        DocumentNode::Folder { path, .. } => format!("folder:{path}"),
        DocumentNode::Document { entry, .. } => format!("doc:{}", entry.id),
    }
}

/// One row — a folder that opens and closes, or a document that can be selected.
#[component]
pub fn DocumentTreeRow(
    node: DocumentNode,
    depth: usize,
    cs: Signal<DocumentsState>,
) -> Element {
    let mut cs = cs;

    // Read off the url rather than out of the state: the url is what says which document is open, and reading
    // it here also means a row re-draws when the selection moves — including when it moves by the back button.
    //
    // Read into a variable first and not behind a short-circuit: it is a hook, and skipping it on a folder row
    // would make the hook order depend on what the row happens to be.
    let selected = selected_id();

    let indent = format!("padding-left: {}px", depth * INDENT + 6);

    match node {
        DocumentNode::Folder {
            name,
            path,
            children,
        } => {
            let expanded = cs.read().is_expanded(&path);

            // A folder inside the reserved root is somebody else's, and is drawn as such at every depth —
            // not only at the top. A reader three levels into a mirrored repository is exactly the reader
            // who has forgotten which tree they are in.
            let remote = is_github_path(&path);

            // Owned before the rsx rather than read inside it: `connection_note` and `connection_tag`
            // borrow out of the signal's read guard, and a guard cannot outlive the expression that made
            // it. `Some` here means this row IS a connection rather than a folder inside one.
            let connection = folder_connection(&path).map(str::to_string);

            let icon = match (&connection, remote) {
                // The repository's own row: GitHub's mark, because this is where somebody else's
                // repository starts rather than a folder of this project's.
                (Some(_), _) => github_icon(),
                (None, true) => remote_folder_icon(),
                (None, false) => folder_icon(expanded),
            };

            let row_class = if remote {
                "tree-row remote"
            } else {
                "tree-row"
            };

            let title = if remote {
                "A connected GitHub repository — a real clone on the server, and read-only: nothing here or on the tools writes into it, because refreshing the connection deletes the folder and clones the repository again. Sync from GitHub copies files out of it into this project as documents of its own, which are yours to edit."
            } else {
                ""
            };

            let for_toggle = path.clone();

            // What a refresh started HERE would read: this connection, and only on its own row. A folder
            // INSIDE a repository is not a thing that refreshes on its own, and there is no root row left
            // to hang "refresh all of them" on — each repository is now its own top-level row, so the
            // button sits on every one of them rather than once above the lot.
            let refresh: Option<Vec<String>> = connection.as_ref().map(|name| vec![name.clone()]);

            let refresh_title = "Read this repository from GitHub again";

            // The note, whether it is the one a person can act on, and whose account this is. `needs a key`
            // is the only state with a ten-second fix, so it is the only tag that becomes a button — the
            // rest are facts to read, and a control that does nothing is worse than a label.
            //
            // One read guard for all three: three separate `cs.read()` calls would be three borrows of the
            // same signal in one render for no gain.
            let (note, needs_key, owner_tag) = match connection.as_ref() {
                Some(name) => {
                    let cs_ra = cs.read();
                    let found = cs_ra.connection(name);

                    (
                        found.and_then(connection_note),
                        found
                            .map(|itm| itm.state == GithubMirrorState::NEEDS_KEY)
                            .unwrap_or(false),
                        cs_ra.connection_tag(name),
                    )
                }
                None => (None, false, None),
            };

            let label = name.clone();

            let children_amount = children.len();

            rsx! {
                div {
                    class: "{row_class}",
                    style: "{indent}",
                    title: "{title}",
                    onclick: move |_| cs.write().toggle(&for_toggle),

                    img { class: "tree-icon", src: "{icon}" }
                    span { class: "tree-name truncate", "{label}" }
                    // Whose account it is, on the repository's own row. It is what the `github` level used
                    // to say before it was folded away — said once, about one repository, instead of once
                    // about all of them.
                    if let Some(tag) = owner_tag {
                        span { class: "tree-tag", "{tag}" }
                    }

                    if let Some(note) = note {
                        if needs_key {
                            button {
                                class: "tree-tag actionable",
                                title: "Give this repository a key",
                                onclick: {
                                    let connection = connection.clone().unwrap_or_default();

                                    move |event: Event<MouseData>| {
                                        event.stop_propagation();

                                        let project = cs.read().selected_project.clone();

                                        crate::dialogs::open(crate::dialogs::DialogState::GithubKey {
                                            project,
                                            connection: connection.clone(),
                                            on_saved: EventHandler::new(move |_| {
                                                cs.write().refresh_connections()
                                            }),
                                        });
                                    }
                                },
                                "{note}"
                            }
                        } else {
                            span { class: "tree-tag", "{note}" }
                        }
                    }

                    if let Some(refresh) = refresh {
                        button {
                            class: "tree-action",
                            title: "{refresh_title}",
                            onclick: move |event: Event<MouseData>| {
                                // The row itself folds the tree when clicked, and a press of this
                                // button is not a press of the row.
                                event.stop_propagation();

                                let project = cs.read().selected_project.clone();

                                crate::dialogs::open(crate::dialogs::DialogState::RefreshGithub {
                                    project,
                                    connections: refresh.clone(),
                                    // Nothing is running yet — the dialog is opened at the question,
                                    // and pressing Refresh in it is what starts anything.
                                    attempt: 0,
                                    runs: None,
                                    // The whole index, not just the connections: a listing that
                                    // finished may hold different files, and those are rows in this
                                    // tree.
                                    on_finished: EventHandler::new(move |_| cs.write().refresh()),
                                });
                            },
                            "↻"
                        }
                    } else {
                        span { class: "tree-size dim", "{children_amount}" }
                    }
                }

                // Only mounted while open, so collapsing a folder stops its children rendering at all.
                if expanded {
                    DocumentNodes {
                        nodes: children.clone(),
                        depth: depth + 1,
                        cs,
                    }
                }
            }
        }
        DocumentNode::Document { name, entry } => {
            let icon = file_icon(&name);

            // **Compared as references rather than as strings**, because the url may spell this row's
            // document any of the ways one can be spelled: a reference followed from a task, the bare id
            // this tree used to navigate by, or the `github:` id a mirrored file carries in the index.
            // All of them are the same document, and a row that failed to highlight for one of them would
            // leave the reader looking at a document the tree says they are not on.
            let reference = canonical_document_reference(&entry.project, &entry.id);

            let is_selected = !selected.is_empty()
                && canonical_document_reference(&entry.project, &selected) == reference;

            let row_class = if is_selected {
                "tree-row selected"
            } else {
                "tree-row"
            };

            let size = render_size(entry.size);

            // The path, and under it what the document says — which is the whole of why a brief is written.
            // A tooltip rather than a line in the tree: the tree is a tree, and a hundred rows carrying two
            // lines each stops being one.
            let title = match entry.brief.is_empty() {
                true => entry.path.clone(),
                false => format!("{}\n\n{}", entry.path, entry.brief),
            };

            rsx! {
                div {
                    class: "{row_class}",
                    style: "{indent}",
                    title: "{title}",
                    // Selecting is navigating: the reference goes into the url and the viewer follows from
                    // there. Nothing about the selection is written to the state — which is what makes a
                    // document linkable and the back button work.
                    //
                    // The REFERENCE rather than the bare id, so that the url somebody copies out of the
                    // address bar is the same string a task stores and an agent can be handed. It also
                    // names the board, which is what makes such a link open on the right one.
                    onclick: move |_| {
                        navigator().push(crate::AppRoute::Documents { selected: reference.clone() });
                    },

                    img { class: "tree-icon", src: "{icon}" }
                    span { class: "tree-name truncate", "{name}" }
                    span { class: "tree-size dim", "{size}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> DocumentIndexEntryResponse {
        DocumentIndexEntryResponse {
            id: format!("id-{path}"),
            project: "P".to_string(),
            path: path.to_string(),
            content_type: "text/markdown".to_string(),
            is_binary: false,
            brief: String::new(),
            version: 1,
            size: 0,
            updated_unix_seconds: 0,
            updated_by: "AI".to_string(),
        }
    }

    fn names(nodes: &[DocumentNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| match node {
                DocumentNode::Folder { name, children, .. } => {
                    format!("{name}/({})", names(children).join(","))
                }
                DocumentNode::Document { name, .. } => name.clone(),
            })
            .collect()
    }

    #[test]
    fn a_flat_list_stays_flat() {
        let tree = build_tree(&[entry("b.md"), entry("a.md")]);
        assert_eq!(names(&tree), vec!["a.md", "b.md"]);
    }

    /// The whole point: the folders in the tree came out of the paths, and nothing else said they exist.
    #[test]
    fn folders_come_out_of_the_paths() {
        let tree = build_tree(&[
            entry("docs/design/system.md"),
            entry("docs/notes.md"),
            entry("readme.md"),
        ]);

        assert_eq!(
            names(&tree),
            vec!["docs/(design/(system.md),notes.md)", "readme.md"]
        );
    }

    /// Two documents in one folder must land in ONE node, or the folder is drawn twice with half its files in
    /// each — which is what a naive append does.
    #[test]
    fn documents_of_one_folder_share_its_node() {
        let tree = build_tree(&[entry("docs/a.md"), entry("docs/b.md")]);

        assert_eq!(tree.len(), 1, "one folder, not two");
        assert_eq!(names(&tree), vec!["docs/(a.md,b.md)"]);
    }

    /// Order in, same tree out. The server sorts, but a tree that depended on that would break silently the
    /// day anything else fed it.
    #[test]
    fn the_order_it_arrives_in_does_not_matter() {
        let sorted = build_tree(&[entry("docs/a.md"), entry("docs/z/b.md"), entry("top.md")]);
        let shuffled = build_tree(&[entry("top.md"), entry("docs/z/b.md"), entry("docs/a.md")]);

        assert_eq!(names(&sorted), names(&shuffled));
    }

    /// Folders before documents at every level: a reader scans for structure first.
    #[test]
    fn folders_sort_before_documents() {
        let tree = build_tree(&[entry("zz.md"), entry("aa/one.md")]);
        assert_eq!(names(&tree), vec!["aa/(one.md)", "zz.md"]);
    }

    /// **A connected repository is ONE row, and its path still has both segments.** Those two facts have to
    /// hold together: the row a person sees is `analytics`, sitting beside the project's own folders with no
    /// `github` level above it — and the node's path is `github/analytics`, because that is what the server
    /// reads, writes and refuses by, what the open/closed set is keyed by, and what `folder_connection`
    /// reads the connection's name back out of. A change that flattened the PATH would pass the eye and
    /// break every one of those.
    #[test]
    fn a_connected_repository_is_one_row_and_keeps_its_full_path() {
        let tree = build_tree(&[
            entry("github/analytics/docs/a.md"),
            entry("github/analytics/readme.md"),
            entry("docs/own.md"),
        ]);

        // Two top-level rows, not a `github` node with a repository inside it — and the repository sorts
        // among the project's own folders rather than under a level of its own.
        assert_eq!(
            names(&tree),
            vec!["analytics/(docs/(a.md),readme.md)", "docs/(own.md)"]
        );

        let DocumentNode::Folder { path, children, .. } = &tree[0] else {
            panic!("expected the repository's folder");
        };

        assert_eq!(path, "github/analytics", "the path keeps the reserved root");
        assert_eq!(folder_connection(path), Some("analytics"));
        assert_eq!(node_key(&tree[0]), "folder:github/analytics");

        // And the depth below it is untouched: a folder inside the repository carries the whole path too.
        let DocumentNode::Folder { path, .. } = &children[0] else {
            panic!("expected a folder inside the repository");
        };

        assert_eq!(path, "github/analytics/docs");
    }

    /// A connection with no files still gets a row, at the top level — the state somebody most needs to see
    /// is the empty one, because "needs a key" and "still reading" both list nothing.
    #[test]
    fn a_connection_with_nothing_in_it_is_still_a_top_level_row() {
        let mut tree = build_tree(&[entry("docs/own.md")]);

        merge_github_connections(&mut tree, &["analytics".to_string()]);

        assert_eq!(names(&tree), vec!["analytics/()", "docs/(own.md)"]);

        // Merging again must not draw it twice — a connection that HAS files is already in the tree.
        merge_github_connections(&mut tree, &["analytics".to_string()]);
        assert_eq!(names(&tree), vec!["analytics/()", "docs/(own.md)"]);
    }

    /// A folder is keyed by its path and a document by its id, so a repaint does not swap rows around.
    #[test]
    fn every_node_has_a_key_of_its_own() {
        let tree = build_tree(&[entry("docs/a.md")]);

        assert_eq!(node_key(&tree[0]), "folder:docs");

        if let DocumentNode::Folder { children, .. } = &tree[0] {
            assert_eq!(node_key(&children[0]), "doc:id-docs/a.md");
        } else {
            panic!("expected a folder");
        }
    }
}
