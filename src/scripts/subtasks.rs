use crate::board::SubtaskModel;

/// A checklist item to add. `text` is optional — a one-line item is a normal item.
pub struct NewSubtask {
    pub title: String,
    pub text: Option<String>,
}

/// A rewrite of one existing item, named by its id. `None` leaves that half alone, so a caller fixing a
/// typo in a title does not have to resend the text.
pub struct SubtaskEdit {
    pub id: String,
    pub title: Option<String>,
    pub text: Option<String>,
}

/// What a caller wants to change about a checklist.
///
/// Field-per-operation rather than "here is the new list", and for the same reason `add_labels` /
/// `remove_labels` are shaped that way: a caller ticking one item does not have to know — or resend — the
/// other nine. A whole-list write loses whatever it did not know about, which on a list two people are
/// adding to is a lost item rather than a conflict anybody notices.
///
/// **Every op names an item by id, and an id that names no item is refused.** Unlike a label, where
/// removing a tag that is not there is harmless, an unknown id here means the caller is working from a read
/// that has moved on — and quietly doing nothing would report success for a tick that never happened.
#[derive(Default)]
pub struct SubtasksPatch {
    pub add: Vec<NewSubtask>,
    pub edit: Vec<SubtaskEdit>,
    pub check: Vec<String>,
    pub uncheck: Vec<String>,
    pub remove: Vec<String>,
}

impl SubtasksPatch {
    /// Whether this patch would do nothing to the checklist.
    pub fn is_empty(&self) -> bool {
        self.add.is_empty()
            && self.edit.is_empty()
            && self.check.is_empty()
            && self.uncheck.is_empty()
            && self.remove.is_empty()
    }

    /// Apply the whole patch to one checklist, or refuse it entirely.
    ///
    /// **The order is fixed: add, edit, check, uncheck, remove.** It has to be one order or the other, and
    /// this is the one a reader assumes — an item passed to both `check` and `uncheck` ends up unchecked,
    /// and one passed to both `add` and `remove`... cannot happen, because a fresh item's id is minted here
    /// and no caller can name it in the same call. Removal last for the same reason it is last for labels.
    ///
    /// Validation happens as the ops are applied rather than up front, so a caller gets the first real
    /// problem named. That is safe because the caller edits a CLONE of the task — see `update_task` — and a
    /// refusal drops it without ever reaching Postgres or memory.
    ///
    /// `owner` is the handle the checklist hangs off, purely so an error says which task or goal it is
    /// talking about.
    pub fn apply(&self, items: &mut Vec<SubtaskModel>, owner: &str) -> Result<(), String> {
        for new_item in &self.add {
            items.push(build_subtask(new_item)?);
        }

        for edit in &self.edit {
            let found = find_mut(items, &edit.id, owner)?;

            if let Some(title) = &edit.title {
                let title = title.trim();

                if title.is_empty() {
                    return Err("a checklist item needs a title".to_string());
                }

                found.title = title.to_string();
            }

            if let Some(text) = &edit.text {
                found.text = text.trim().to_string();
            }
        }

        for id in &self.check {
            find_mut(items, id, owner)?.done = true;
        }

        for id in &self.uncheck {
            find_mut(items, id, owner)?.done = false;
        }

        for id in &self.remove {
            // Located before it is dropped, so an unknown id is refused rather than removing nothing and
            // reporting success.
            let position = position_of(items, id, owner)?;
            items.remove(position);
        }

        Ok(())
    }
}

/// One item as it should be stored. A title is required — an item nobody can read is not a checklist entry.
fn build_subtask(src: &NewSubtask) -> Result<SubtaskModel, String> {
    let title = src.title.trim();

    if title.is_empty() {
        return Err("a checklist item needs a title".to_string());
    }

    Ok(SubtaskModel {
        // Generated here and nowhere else, which is what keeps ids unique across a list two callers are
        // adding to: nothing outside chooses one.
        id: rust_extensions::SortableId::generate().to_string(),
        title: title.to_string(),
        text: src.text.as_deref().unwrap_or_default().trim().to_string(),
        done: false,
    })
}

/// Every item of a brand-new checklist, for a task or goal being created with one.
pub fn build_subtasks(src: &[NewSubtask]) -> Result<Vec<SubtaskModel>, String> {
    src.iter().map(build_subtask).collect()
}

fn position_of(items: &[SubtaskModel], id: &str, owner: &str) -> Result<usize, String> {
    let id = id.trim();

    items
        .iter()
        .position(|itm| itm.id == id)
        .ok_or_else(|| match items.len() {
            0 => format!("{owner} has no checklist at all, so there is no item '{id}' to change"),
            _ => format!(
                "{owner} has no checklist item '{id}' — read the task or goal again and use the `id` it returns for each item"
            ),
        })
}

fn find_mut<'s>(
    items: &'s mut Vec<SubtaskModel>,
    id: &str,
    owner: &str,
) -> Result<&'s mut SubtaskModel, String> {
    let position = position_of(items, id, owner)?;
    Ok(&mut items[position])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(title: &str) -> NewSubtask {
        NewSubtask {
            title: title.to_string(),
            text: None,
        }
    }

    #[test]
    fn adding_gives_every_item_an_id_of_its_own() {
        let items = build_subtasks(&[item("one"), item("two")]).unwrap();

        assert_eq!(items.len(), 2);
        assert_ne!(items[0].id, items[1].id);
        assert!(items.iter().all(|itm| !itm.done), "nothing starts ticked");
    }

    /// The order the caller wrote them in is the order the reader sees, so an item added later is at the
    /// bottom rather than wherever a map happened to put it.
    #[test]
    fn added_items_keep_the_order_they_arrived_in() {
        let mut items = build_subtasks(&[item("one")]).unwrap();

        SubtasksPatch {
            add: vec![item("two"), item("three")],
            ..Default::default()
        }
        .apply(&mut items, "RMS-1")
        .unwrap();

        let titles: Vec<&str> = items.iter().map(|itm| itm.title.as_str()).collect();
        assert_eq!(titles, vec!["one", "two", "three"]);
    }

    #[test]
    fn an_item_without_a_title_is_refused() {
        assert!(build_subtasks(&[item("  ")]).is_err());
    }

    #[test]
    fn ticking_and_unticking_both_work() {
        let mut items = build_subtasks(&[item("one")]).unwrap();
        let id = items[0].id.clone();

        SubtasksPatch {
            check: vec![id.clone()],
            ..Default::default()
        }
        .apply(&mut items, "RMS-1")
        .unwrap();

        assert!(items[0].done);

        SubtasksPatch {
            uncheck: vec![id],
            ..Default::default()
        }
        .apply(&mut items, "RMS-1")
        .unwrap();

        assert!(!items[0].done);
    }

    /// It has to be one or the other, and unchecking last is the documented order. The test exists so the
    /// order is a decision rather than whatever the loop happened to do.
    #[test]
    fn an_item_passed_to_both_ends_up_unchecked() {
        let mut items = build_subtasks(&[item("one")]).unwrap();
        let id = items[0].id.clone();

        SubtasksPatch {
            check: vec![id.clone()],
            uncheck: vec![id],
            ..Default::default()
        }
        .apply(&mut items, "RMS-1")
        .unwrap();

        assert!(!items[0].done);
    }

    /// An unknown id is the case that matters: it means the caller is working from a stale read, and
    /// silently succeeding would report a tick that never happened.
    #[test]
    fn an_unknown_id_is_refused_by_every_op() {
        let base = build_subtasks(&[item("one")]).unwrap();

        for patch in [
            SubtasksPatch {
                check: vec!["nope".to_string()],
                ..Default::default()
            },
            SubtasksPatch {
                uncheck: vec!["nope".to_string()],
                ..Default::default()
            },
            SubtasksPatch {
                remove: vec!["nope".to_string()],
                ..Default::default()
            },
            SubtasksPatch {
                edit: vec![SubtaskEdit {
                    id: "nope".to_string(),
                    title: Some("two".to_string()),
                    text: None,
                }],
                ..Default::default()
            },
        ] {
            let mut items = base.clone();
            let result = patch.apply(&mut items, "RMS-1");

            assert!(result.is_err(), "an unknown id must be refused");
            assert_eq!(items, base, "a refused patch must change nothing");
        }
    }

    /// Editing one half leaves the other alone — a caller fixing a typo in a title has no reason to know
    /// what the text says.
    #[test]
    fn an_edit_touches_only_what_it_names() {
        let mut items = build_subtasks(&[NewSubtask {
            title: "one".to_string(),
            text: Some("the long half".to_string()),
        }])
        .unwrap();

        let id = items[0].id.clone();

        SubtasksPatch {
            edit: vec![SubtaskEdit {
                id: id.clone(),
                title: Some("uno".to_string()),
                text: None,
            }],
            ..Default::default()
        }
        .apply(&mut items, "RMS-1")
        .unwrap();

        assert_eq!(items[0].title, "uno");
        assert_eq!(items[0].text, "the long half");
        assert_eq!(items[0].id, id, "an edit is not a new item");
    }

    #[test]
    fn removing_takes_out_exactly_one() {
        let mut items = build_subtasks(&[item("one"), item("two")]).unwrap();
        let id = items[0].id.clone();

        SubtasksPatch {
            remove: vec![id],
            ..Default::default()
        }
        .apply(&mut items, "RMS-1")
        .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "two");
    }

    #[test]
    fn an_empty_patch_is_recognised() {
        assert!(SubtasksPatch::default().is_empty());

        assert!(
            !SubtasksPatch {
                add: vec![item("one")],
                ..Default::default()
            }
            .is_empty()
        );
    }
}
