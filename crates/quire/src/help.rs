//! Help and shortcuts: the reference guide's topics and a shortcut reference generated from the
//! keymap Quire actually bound, so the two never drift apart.

pub mod content;
pub mod view;

use gpui::{App, KeyBinding, actions};
use std::collections::BTreeSet;

actions!(
    quire_help,
    [
        /// Opens Help and shortcuts.
        OpenHelp,
    ]
);

/// One row of the shortcut reference.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Shortcut {
    pub scope: String,
    pub keys: String,
    pub action: String,
    pub name: &'static str,
}

impl Shortcut {
    pub fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || [self.keys.as_str(), &self.action, &self.scope, self.name]
                .into_iter()
                .any(|text| text.to_lowercase().contains(needle))
    }
}

pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenHelp, cx| view::open(cx));
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("f1", OpenHelp, None),
        KeyBinding::new("escape", menu::Cancel, Some(view::CONTEXT)),
        KeyBinding::new("escape", menu::Cancel, Some(view::FIELD_CONTEXT)),
    ]);
}

/// Every binding in the keymap, with its keys as this platform shows them, the action's own
/// documentation, and where it applies. Disabled bindings are left out.
pub fn shortcuts(cx: &App) -> Vec<Shortcut> {
    let documentation = cx.action_documentation();
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    keymap
        .bindings()
        .filter(|binding| {
            !gpui::is_no_action(binding.action()) && !gpui::is_unbind(binding.action())
        })
        .map(|binding| {
            let name = binding.action().name();
            Shortcut {
                scope: binding.predicate().map_or_else(
                    || "Everywhere".to_owned(),
                    |predicate| scope(&predicate.to_string()),
                ),
                keys: ui::text_for_keybinding_keystrokes(binding.keystrokes(), cx),
                action: documentation
                    .get(name)
                    .and_then(|text| text.lines().next())
                    .map(|line| line.trim_end_matches('.').to_owned())
                    .unwrap_or_else(|| humanize(name)),
                name,
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// A readable name for an undocumented action: `vim::MoveToNextWord` reads "Move to next word".
pub fn humanize(name: &str) -> String {
    let base = name.rsplit("::").next().unwrap_or(name);
    let mut words = String::new();
    for (index, char) in base.char_indices() {
        if char.is_uppercase() && index > 0 {
            words.push(' ');
            words.extend(char.to_lowercase());
        } else {
            words.push(char);
        }
    }
    words.replace('_', " ")
}

/// Where a binding applies, in the window's words rather than its key context.
pub fn scope(predicate: &str) -> String {
    let named = [
        ("QuirePreferences", "Settings"),
        ("QuireHelp", "Help"),
        ("QuireTaskList", "Task list"),
        ("QuireTaskField", "Task fields"),
        ("QuireTaskFilter", "Task filter"),
        ("QuireTags", "Tags"),
        ("QuireFiles", "Files"),
        ("QuireReferences", "Linked references"),
        ("QuireTagEdit", "Tag edits"),
        ("QuireList", "Lists"),
        ("QuireDialog", "Dialogs"),
        ("QuireEx", "Command line"),
        ("QuireNotebook", "Notebook"),
        ("BufferSearchBar", "Find in page"),
        ("Picker", "Find"),
        ("vim_mode == normal", "Vim Normal"),
        ("vim_mode == visual", "Vim Visual"),
        ("vim_mode == insert", "Vim Insert"),
        ("vim_mode == replace", "Vim Replace"),
        ("vim_mode == operator", "Vim operator pending"),
        ("vim_mode == waiting", "Vim waiting for a character"),
        ("vim_operator", "Vim operator pending"),
        ("VimControl", "Vim"),
        ("menu", "Menus"),
        ("Editor", "Editor"),
    ];
    named
        .iter()
        .find(|(context, _)| predicate.contains(context))
        .map_or_else(|| predicate.to_owned(), |(_, name)| (*name).to_owned())
}

#[cfg(test)]
mod tests;
