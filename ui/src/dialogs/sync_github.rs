use std::collections::HashSet;

use dioxus::prelude::*;
use task_manager_shared::documents::normalise_document_path;

/// What a sync asks for, once the reader has chosen it.
#[derive(Clone, PartialEq)]
pub struct SyncGithubSubmit {
    pub project: String,
    pub connection: String,
    /// Relative to the connection's root. A FOLDER path means everything under it, expanded by the
    /// server rather than here — see [`SyncGithubDialog`].
    pub paths: Vec<String>,
    pub folder: String,
    pub override_existing: bool,
}

/// One connected repository as this dialog needs it: its name, and the paths its mirror holds.
#[derive(Clone, PartialEq)]
pub struct MirrorChoice {
    pub connection: String,
    /// Paths relative to the connection's root, sorted.
    pub paths: Vec<String>,
}

#[derive(Default)]
struct ComponentState {
    /// Which connection is being synced, by name. Empty before the first render fills it in.
    connection: String,
    /// What is ticked: file paths and FOLDER paths, mixed. A folder in here means everything under it,
    /// which is why a subtree does not have to be enumerated — see the note on [`SyncGithubDialog`].
    picked: HashSet<String>,
    /// Which folders of the mirror are drawn open.
    expanded: HashSet<String>,
    folder: String,
    override_existing: bool,
}

/// Copy files out of a connected repository into the project's own documents.
///
/// **What is ticked is sent as it was ticked, folders included, and the server expands it.** A tree
/// ticked at the top would otherwise have to be flattened into hundreds of paths here — and every one of
/// them would be a claim about what the mirror held when the tree was DRAWN rather than when the button
/// was pressed. Sending `docs` and letting the server ask its own mirror what is in it is both smaller
/// and more honest.
///
/// **Override is off by default and says what it does.** Off writes only what is not already there,
/// which is safe to press repeatedly and never overwrites a document somebody has since edited. On
/// writes a new version over every chosen path — keeping ids and history, exactly as re-uploading a file
/// does. The destructive reading of a button pressed by mistake should be the one you have to ask for.
#[component]
pub fn SyncGithubDialog(
    project: String,
    mirrors: Vec<MirrorChoice>,
    folders: Vec<String>,
    initial_folder: String,
    on_submit: EventHandler<SyncGithubSubmit>,
) -> Element {
    let mut cs = use_signal(|| ComponentState {
        connection: mirrors
            .first()
            .map(|itm| itm.connection.clone())
            .unwrap_or_default(),
        folder: initial_folder.clone(),
        ..Default::default()
    });

    let cs_ra = cs.read();

    let feedback = super::feedback();

    let connection = cs_ra.connection.clone();
    let folder = cs_ra.folder.clone();
    let override_existing = cs_ra.override_existing;
    let picked = cs_ra.picked.clone();

    drop(cs_ra);

    let Some(chosen) = mirrors
        .iter()
        .find(|itm| itm.connection == connection)
        .or_else(|| mirrors.first())
    else {
        return super::dialog_template_read_only(
            "Sync from GitHub",
            rsx! {
                div { class: "empty-note",
                    "No connected repository has anything to copy yet. Connect one under Projects setup — and if one is already connected, it may still be reading, or waiting for a key."
                }
            },
            None,
        );
    };

    let nodes = build_pick_tree(&chosen.paths);

    // What the button will actually write, counted the same way the server will count it — so the reader
    // is shown the answer rather than the inputs to it.
    let chosen_amount = chosen
        .paths
        .iter()
        .filter(|path| is_covered(&picked, path))
        .count();

    let destination = folder.trim().trim_matches('/');

    let path_problem = if destination.is_empty() {
        None
    } else {
        normalise_document_path(destination).err()
    };

    let landing = if destination.is_empty() {
        "Will land at the top level, keeping the folders the repository has".to_string()
    } else {
        format!("Will land in {destination}/, keeping the folders the repository has")
    };

    let ready = chosen_amount > 0 && path_problem.is_none();

    let content = rsx! {
        div { class: "sync-form",
            if mirrors.len() > 1 {
                div { class: "form-row",
                    label { "Repository" }
                    select {
                        onchange: move |event| {
                            let mut write = cs.write();
                            write.connection = event.value();
                            // A path from one repository names nothing in another.
                            write.picked.clear();
                            write.expanded.clear();
                        },
                        for mirror in mirrors.iter() {
                            option {
                                value: "{mirror.connection}",
                                selected: mirror.connection == connection,
                                "{mirror.connection} · {mirror.paths.len()} files"
                            }
                        }
                    }
                }
            }

            div { class: "form-row",
                label { "Into folder" }
                div { class: "upload-folder",
                    input {
                        r#type: "text",
                        placeholder: "docs/reference — leave empty for the top level",
                        value: "{folder}",
                        oninput: move |event| cs.write().folder = event.value(),
                    }
                    if !folders.is_empty() {
                        select {
                            class: "upload-folder-pick",
                            onchange: move |event| cs.write().folder = event.value(),
                            option { value: "", selected: folder.is_empty(), "— existing folders —" }
                            for existing in folders.iter() {
                                option {
                                    value: "{existing}",
                                    selected: existing == &folder,
                                    "{existing}"
                                }
                            }
                        }
                    }
                }
            }

            div { class: "form-row",
                label { "What to copy" }
                div { class: "page-actions",
                    button {
                        class: "btn",
                        onclick: {
                            let paths = chosen.paths.clone();
                            move |_| {
                                let mut write = cs.write();
                                write.picked = paths.iter().cloned().collect();
                            }
                        },
                        "All"
                    }
                    button {
                        class: "btn",
                        onclick: move |_| cs.write().picked.clear(),
                        "None"
                    }
                }
                div { class: "sync-tree",
                    if nodes.is_empty() {
                        div { class: "tree-note", "This repository's mirror is empty." }
                    } else {
                        PickNodes { nodes, depth: 0, cs }
                    }
                }
            }

            div { class: "form-row",
                div { class: "checkbox-row",
                    input {
                        r#type: "checkbox",
                        id: "sync-override",
                        checked: override_existing,
                        onchange: move |event: Event<FormData>| {
                            cs.write().override_existing = event.checked();
                        },
                    }
                    label { r#for: "sync-override",
                        "Override — replace documents that are already at these paths"
                    }
                }
                div { class: "field-hint",
                    if override_existing {
                        "Every chosen file is written, as a new version of whatever is already at its path. Ids and history are kept; the text is the repository's."
                    } else {
                        "Only what is not already there is written. Anything that exists is left exactly as it is and reported as skipped."
                    }
                }
            }

            if let Some(problem) = path_problem {
                div { class: "error-note", "{problem}" }
            } else {
                div { class: "field-hint", "{chosen_amount} files chosen. {landing}" }
            }

            if !feedback.error.is_empty() {
                div { class: "error-note", "{feedback.error}" }
            }
        }
    };

    let connection_for_submit = chosen.connection.clone();

    let ok = rsx! {
        button {
            class: "btn btn-primary",
            disabled: !ready || feedback.saving,
            onclick: move |_| {
                let cs_ra = cs.read();

                let submit = SyncGithubSubmit {
                    project: project.clone(),
                    connection: connection_for_submit.clone(),
                    // Ticked folders travel as folders. See the note on this component.
                    paths: cs_ra.picked.iter().cloned().collect(),
                    folder: cs_ra.folder.clone(),
                    override_existing: cs_ra.override_existing,
                };

                drop(cs_ra);

                on_submit.call(submit);
            },
            if feedback.saving { "Copying…" } else { "Sync" }
        }
    };

    super::dialog_template_ex("Sync from GitHub", content, ok, Some("modal-lg"))
}

/// One node of the mirror's tree. Folders come out of the paths, exactly as they do everywhere else in
/// this product — there is no folder in a mirror either, only files whose paths have slashes in them.
#[derive(Clone, PartialEq)]
pub enum PickNode {
    Folder {
        name: String,
        path: String,
        children: Vec<PickNode>,
    },
    File {
        name: String,
        path: String,
    },
}

fn build_pick_tree(paths: &[String]) -> Vec<PickNode> {
    let mut roots: Vec<PickNode> = Vec::new();

    for path in paths {
        let mut segments: Vec<&str> = path.split('/').filter(|itm| !itm.is_empty()).collect();

        let Some(name) = segments.pop() else {
            continue;
        };

        insert(&mut roots, &segments, "", name, path);
    }

    sort_level(&mut roots);
    roots
}

fn insert(
    level: &mut Vec<PickNode>,
    folders: &[&str],
    walked: &str,
    name: &str,
    full_path: &str,
) {
    let Some((head, rest)) = folders.split_first() else {
        level.push(PickNode::File {
            name: name.to_string(),
            path: full_path.to_string(),
        });

        return;
    };

    let path = if walked.is_empty() {
        head.to_string()
    } else {
        format!("{walked}/{head}")
    };

    // Found or created, in that order: two files under one folder must land in ONE node, or the folder is
    // drawn twice with half its files in each.
    let position = level.iter().position(|node| match node {
        PickNode::Folder { path: existing, .. } => existing == &path,
        PickNode::File { .. } => false,
    });

    let position = match position {
        Some(position) => position,
        None => {
            level.push(PickNode::Folder {
                name: head.to_string(),
                path: path.clone(),
                children: Vec::new(),
            });

            level.len() - 1
        }
    };

    if let PickNode::Folder { children, .. } = &mut level[position] {
        insert(children, rest, &path, name, full_path);
    }
}

fn sort_level(level: &mut Vec<PickNode>) {
    level.sort_by_key(|node| match node {
        PickNode::Folder { name, .. } => (0, name.to_lowercase()),
        PickNode::File { name, .. } => (1, name.to_lowercase()),
    });

    for node in level.iter_mut() {
        if let PickNode::Folder { children, .. } = node {
            sort_level(children);
        }
    }
}

/// Whether a path is taken by the selection — itself, or any folder above it.
///
/// The second half is what lets a ticked folder stand for its whole subtree without the subtree being
/// listed, and it is the same rule the server applies when it expands the selection.
fn is_covered(picked: &HashSet<String>, path: &str) -> bool {
    if picked.contains(path) {
        return true;
    }

    let mut walked = String::new();

    for segment in path.split('/') {
        if !walked.is_empty() {
            walked.push('/');
        }

        walked.push_str(segment);

        if picked.contains(&walked) {
            return true;
        }
    }

    false
}

#[component]
fn PickNodes(nodes: Vec<PickNode>, depth: usize, cs: Signal<ComponentState>) -> Element {
    rsx! {
        for node in nodes.iter() {
            PickRow {
                key: "{node_key(node)}",
                node: node.clone(),
                depth,
                cs,
            }
        }
    }
}

fn node_key(node: &PickNode) -> String {
    match node {
        PickNode::Folder { path, .. } => format!("folder:{path}"),
        PickNode::File { path, .. } => format!("file:{path}"),
    }
}

#[component]
fn PickRow(node: PickNode, depth: usize, cs: Signal<ComponentState>) -> Element {
    let mut cs = cs;

    let indent = format!("padding-left: {}px", depth * 14 + 6);
    let picked = cs.read().picked.clone();

    match node {
        PickNode::Folder {
            name,
            path,
            children,
        } => {
            let expanded = cs.read().expanded.contains(&path);
            let checked = is_covered(&picked, &path);

            let for_toggle = path.clone();
            let for_check = path.clone();

            rsx! {
                div { class: "tree-row", style: "{indent}",
                    input {
                        r#type: "checkbox",
                        checked,
                        onclick: move |event: Event<MouseData>| {
                            // Without this the row's own click handler runs too and folds the folder,
                            // which reads as the tick having done nothing.
                            event.stop_propagation();
                        },
                        onchange: move |event: Event<FormData>| {
                            let mut write = cs.write();

                            if event.checked() {
                                // The folder stands for its subtree, so anything ticked inside it is now
                                // redundant — and leaving it would send the same file twice.
                                write.picked.retain(|itm| !itm.starts_with(&format!("{for_check}/")));
                                write.picked.insert(for_check.clone());
                            } else {
                                write.picked.remove(&for_check);
                                write.picked.retain(|itm| !itm.starts_with(&format!("{for_check}/")));
                            }
                        },
                    }
                    span {
                        class: "tree-name truncate",
                        onclick: move |_| {
                            let mut write = cs.write();

                            if !write.expanded.remove(&for_toggle) {
                                write.expanded.insert(for_toggle.clone());
                            }
                        },
                        if expanded { "▾ " } else { "▸ " }
                        "{name}"
                    }
                    span { class: "tree-size dim", "{children.len()}" }
                }

                if expanded {
                    PickNodes { nodes: children.clone(), depth: depth + 1, cs }
                }
            }
        }
        PickNode::File { name, path } => {
            let checked = is_covered(&picked, &path);
            // A file inside a ticked folder is ticked BY that folder, and unticking it on its own would
            // have to punch a hole in a selection this shape cannot express. Shown as ticked and locked,
            // which is the honest drawing of what will happen.
            let by_folder = checked && !picked.contains(&path);
            let for_check = path.clone();

            rsx! {
                div { class: "tree-row", style: "{indent}",
                    input {
                        r#type: "checkbox",
                        checked,
                        disabled: by_folder,
                        onchange: move |event: Event<FormData>| {
                            let mut write = cs.write();

                            if event.checked() {
                                write.picked.insert(for_check.clone());
                            } else {
                                write.picked.remove(&for_check);
                            }
                        },
                    }
                    span { class: "tree-name truncate", title: "{path}", "{name}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(nodes: &[PickNode]) -> Vec<String> {
        nodes
            .iter()
            .map(|node| match node {
                PickNode::Folder { name, children, .. } => {
                    format!("{name}/({})", names(children).join(","))
                }
                PickNode::File { name, .. } => name.clone(),
            })
            .collect()
    }

    #[test]
    fn folders_come_out_of_the_paths() {
        let tree = build_pick_tree(&[
            "readme.md".to_string(),
            "design/system.md".to_string(),
            "design/deep/x.md".to_string(),
        ]);

        assert_eq!(
            names(&tree),
            vec!["design/(deep/(x.md),system.md)", "readme.md"]
        );
    }

    /// The rule the whole selection turns on: a ticked folder stands for everything under it, and the
    /// server applies exactly this when it expands the paths.
    #[test]
    fn a_ticked_folder_covers_its_whole_subtree() {
        let mut picked = HashSet::new();
        picked.insert("design".to_string());

        assert!(is_covered(&picked, "design/system.md"));
        assert!(is_covered(&picked, "design/deep/x.md"));
        assert!(is_covered(&picked, "design"));

        // A sibling that merely starts the same way is not inside it.
        assert!(!is_covered(&picked, "design2/x.md"));
        assert!(!is_covered(&picked, "readme.md"));
    }

    #[test]
    fn a_ticked_file_covers_only_itself() {
        let mut picked = HashSet::new();
        picked.insert("design/system.md".to_string());

        assert!(is_covered(&picked, "design/system.md"));
        assert!(!is_covered(&picked, "design/other.md"));
        assert!(!is_covered(&picked, "design"));
    }
}
