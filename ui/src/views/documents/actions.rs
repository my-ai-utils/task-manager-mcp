use dioxus::prelude::*;
use dioxus_utils::RenderState;
use task_manager_shared::documents::{DocumentIndexEntryResponse, FindDocumentResponse};
use task_manager_shared::github::{
    GithubConnectionResponse, GithubMirrorState, is_github_path, parse_github_mirror_path,
};

use crate::dialogs::MirrorChoice;

use super::DocumentsState;

/// The icons that actually exist under `public/assets/file-types`, keyed by extension.
///
/// An explicit list rather than "try `<ext>.svg` and hope": a missing file renders as a broken-image glyph,
/// which looks like a bug in the tree rather than like an extension nobody has drawn yet. Adding an icon is
/// adding its file and one line here.
const FILE_ICONS: [&str; 6] = ["css", "html", "md", "pdf", "png", "toml"];

const ICON_DIR: &str = "/assets/file-types";

pub fn file_icon(name: &str) -> String {
    match extension(name) {
        Some(extension) if FILE_ICONS.contains(&extension.as_str()) => {
            format!("{ICON_DIR}/{extension}.svg")
        }
        _ => format!("{ICON_DIR}/file.svg"),
    }
}

pub fn folder_icon(expanded: bool) -> String {
    if expanded {
        format!("{ICON_DIR}/folder-open.svg")
    } else {
        format!("{ICON_DIR}/folder.svg")
    }
}

/// The icon for a folder that is a window onto a repository rather than a folder of this project's.
///
/// **It keeps the remote icon whether it is open or closed, unlike an ordinary folder.** An open folder
/// looks like an open folder, and that is exactly the moment somebody is most likely to try to upload
/// into one — the mark has to survive being opened, or it only warns you while you are not looking.
pub fn remote_folder_icon() -> String {
    format!("{ICON_DIR}/folder-remote.svg")
}

/// The icon on a connected repository's OWN row — GitHub's mark rather than a folder of any kind.
///
/// That row is not a folder of this project's: it is where somebody else's repository starts, and it is
/// the one place in the tree where saying so decides what a reader expects of everything under it. It
/// lives beside the images rather than under `file-types`, because it is a brand mark and not a file kind.
///
/// Folders INSIDE a repository keep [`remote_folder_icon`], which is the same point made again at every
/// depth — see the note there on why the mark has to survive being opened.
pub fn github_icon() -> String {
    "/assets/images/github.svg".to_string()
}

fn extension(name: &str) -> Option<String> {
    let (stem, extension) = name.rsplit_once('.')?;

    // `.gitignore` is a name, not an extension of nothing.
    if stem.is_empty() {
        return None;
    }

    Some(extension.to_lowercase())
}

/// The project's whole index, loaded the first time it is asked for.
///
/// One request for the lot, unlike the file browser this screen is modelled on: that one walks a real
/// filesystem a folder at a time, where this one is answered from an index the server holds in memory. What is
/// NOT in it is the payloads — that is the whole point of the split.
pub fn get_index<'s>(
    mut cs: Signal<DocumentsState>,
    cs_ra: &'s DocumentsState,
) -> Result<&'s [DocumentIndexEntryResponse], Element> {
    if cs_ra.selected_project.is_empty() {
        return Ok(&[]);
    }

    match cs_ra.index.as_ref() {
        RenderState::None => {
            let project = cs_ra.selected_project.clone();

            spawn(async move {
                cs.write().index.set_loading();

                match crate::api::get_documents(&project).await {
                    Ok(response) => cs.write().index.set_loaded(response.documents),
                    Err(err) => cs.write().index.set_error(err.message),
                }
            });

            Err(render_tree_note("loading…", false))
        }
        RenderState::Loading => Err(render_tree_note("loading…", false)),
        RenderState::Loaded(index) => Ok(index.as_slice()),
        RenderState::Error(err) => Err(render_tree_note(err.as_str(), true)),
    }
}

/// The selected document, text included.
///
/// Which document that is comes from the url, so the state can be holding a different one — the check is
/// against the id rather than against "has anything been loaded", or a click on a second document would show
/// the first one's text under the second one's name.
///
/// A BINARY document is fetched too, and comes back with no content: the response carries its type and size,
/// and the viewer points an `<iframe>` or an `<img>` at the raw endpoint. Nothing here ever holds bytes.
pub fn get_content<'s>(
    mut cs: Signal<DocumentsState>,
    cs_ra: &'s DocumentsState,
    project: &str,
    id: &str,
) -> Result<&'s FindDocumentResponse, Element> {
    if !cs_ra.content_is_for(id) {
        let project = project.to_string();
        let id = id.to_string();

        spawn(async move {
            cs.write().begin_content_load(&id);

            match crate::api::get_document(&project, &id).await {
                Ok(found) => {
                    let mut write = cs.write();

                    // Opening a document is how a reader says where they are — and this is the one place that
                    // knows the PATH, since the tree row navigates by id and a link carries nothing else. It
                    // is what an upload started from here is offered as its folder.
                    if let Some(document) = found.document.as_ref() {
                        write.enter_folder_of(&document.path);
                    }

                    write.content.set_loaded(found);
                }
                Err(err) => cs.write().content.set_error(err.message),
            }
        });

        return Err(render_viewer_note("loading…", false));
    }

    match cs_ra.content.as_ref() {
        RenderState::None | RenderState::Loading => Err(render_viewer_note("loading…", false)),
        RenderState::Loaded(found) => Ok(found),
        RenderState::Error(err) => Err(render_viewer_note(err.as_str(), true)),
    }
}

/// The project's connected repositories, with the state of each one's listing.
///
/// **A second request beside the index, and it earns its keep in the states where the index is empty.**
/// A folder here is derived from the paths of the documents in it, so a connection that is waiting for a
/// key, still reading, or pointed at a folder with nothing in it produces no path at all — and would be
/// invisible on this screen with nothing to explain why. That is not a rare corner: a key lives only in
/// the server's memory, so every deploy puts every private connection back into it.
pub fn get_connections<'s>(
    mut cs: Signal<DocumentsState>,
    cs_ra: &'s DocumentsState,
) -> &'s [GithubConnectionResponse] {
    if cs_ra.selected_project.is_empty() {
        return &[];
    }

    match cs_ra.connections.as_ref() {
        RenderState::None => {
            let project = cs_ra.selected_project.clone();

            spawn(async move {
                cs.write().connections.set_loading();

                match crate::api::get_github_connections(&project).await {
                    Ok(response) => cs.write().connections.set_loaded(response.connections),
                    // Drawn as none rather than as an error: this is decoration beside the tree, and a
                    // project whose connections cannot be read still has documents worth showing.
                    Err(_) => cs.write().connections.set_loaded(Vec::new()),
                }
            });

            &[]
        }
        RenderState::Loaded(connections) => connections.as_slice(),
        _ => &[],
    }
}

/// What a connection's state says in one short phrase, for the row in the tree.
///
/// `None` when there is nothing worth saying — the connection is ready and its files are right there to
/// be counted, which says more than any word would.
pub fn connection_note(connection: &GithubConnectionResponse) -> Option<String> {
    match connection.state.as_str() {
        GithubMirrorState::READY if connection.files_amount > 0 => None,
        GithubMirrorState::READY => Some("empty".to_string()),
        GithubMirrorState::PULLING => Some("reading…".to_string()),
        GithubMirrorState::PENDING => Some("not read yet".to_string()),
        GithubMirrorState::NEEDS_KEY => Some("needs a key".to_string()),
        GithubMirrorState::FAILED => Some("failed".to_string()),
        other => Some(other.to_string()),
    }
}

/// The connected repositories, read back out of the index the screen already loaded.
///
/// **No request of its own, and that is the point.** The mirrors travel in `documents_list` like every
/// other folder — that is what "served like your own folders" buys — so the sync dialog can be handed
/// exactly what the reader is looking at rather than a second answer that may have moved on.
///
/// Ordered by connection name, and each file list in the order the index arrived in, which is by path.
pub fn mirror_choices(entries: &[DocumentIndexEntryResponse]) -> Vec<MirrorChoice> {
    let mut choices: Vec<MirrorChoice> = Vec::new();

    for entry in entries {
        let Some((connection, relative)) = parse_github_mirror_path(&entry.path) else {
            continue;
        };

        match choices.iter_mut().find(|itm| itm.connection == connection) {
            Some(choice) => choice.paths.push(relative.to_string()),
            None => choices.push(MirrorChoice {
                connection: connection.to_string(),
                paths: vec![relative.to_string()],
            }),
        }
    }

    choices.sort_by(|left, right| left.connection.cmp(&right.connection));
    choices
}

/// The folders a document may be put INTO — every folder in the tree except the mirrors.
///
/// The reserved root is left out because nothing can be written there: offering it would be offering a
/// destination the server refuses, which is a worse experience than not offering it at all.
pub fn writable_folders(folders: &[String]) -> Vec<String> {
    folders
        .iter()
        .filter(|itm| !is_github_path(itm))
        .cloned()
        .collect()
}

pub fn render_tree_note(text: &str, failed: bool) -> Element {
    let class = if failed {
        "tree-note failed"
    } else {
        "tree-note"
    };

    rsx! {
        div { class: "{class}", "{text}" }
    }
}

pub fn render_viewer_note(text: &str, failed: bool) -> Element {
    let class = if failed {
        "viewer-note failed"
    } else {
        "viewer-note"
    };

    rsx! {
        div { class: "{class}", "{text}" }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_the_generic_icon_for_an_extension_nobody_has_drawn() {
        assert_eq!(file_icon("spec.pdf"), "/assets/file-types/pdf.svg");
        assert_eq!(file_icon("NOTES.MD"), "/assets/file-types/md.svg");
        assert_eq!(file_icon("logo.png"), "/assets/file-types/png.svg");

        // Only `.png` is drawn — this maps a name to a file, not a kind to a picture.
        assert_eq!(file_icon("logo.jpg"), "/assets/file-types/file.svg");
        assert_eq!(file_icon("Makefile"), "/assets/file-types/file.svg");
        assert_eq!(file_icon(".gitignore"), "/assets/file-types/file.svg");
    }
}
