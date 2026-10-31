//! A pointer menu described as data and built as a Zed `ContextMenu`. An item either dispatches
//! an action from the focus it was opened over, so the menu and the keys share one handler, or
//! runs a closure on the clicked thing, as Zed's tab menu does, naming the action whose keys it
//! shares.

use gpui::{Action, App, Entity, FocusHandle, SharedString, Window};
use std::rc::Rc;
use ui::{ContextMenu, ContextMenuEntry, IconPosition};

type Run = Rc<dyn Fn(&mut Window, &mut App)>;

enum Choice {
    Dispatch(Box<dyn Action>),
    Run {
        run: Run,
        keys: Option<Box<dyn Action>>,
    },
}

pub struct Item {
    label: SharedString,
    disabled: bool,
    checked: Option<bool>,
    choice: Choice,
}

pub enum Entry {
    Item(Item),
    Separator,
}

impl Item {
    /// Dispatches `action` from the focus the menu was opened over.
    pub fn action(label: impl Into<SharedString>, action: impl Action) -> Item {
        Self::boxed(label, Box::new(action))
    }

    pub fn boxed(label: impl Into<SharedString>, action: Box<dyn Action>) -> Item {
        Item {
            label: label.into(),
            disabled: false,
            checked: None,
            choice: Choice::Dispatch(action),
        }
    }

    pub fn run(
        label: impl Into<SharedString>,
        run: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Item {
        Item {
            label: label.into(),
            disabled: false,
            checked: None,
            choice: Choice::Run {
                run: Rc::new(run),
                keys: None,
            },
        }
    }

    /// Shows the keys of `action`, which does the same to the focused or active item.
    pub fn keys(mut self, action: impl Action) -> Self {
        if let Choice::Run { keys, .. } = &mut self.choice {
            *keys = Some(Box::new(action));
        }
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn done(self) -> Entry {
        Entry::Item(self)
    }
}

/// Drops separators at either end and doubled ones, as the reference menus do.
pub fn tidy(entries: Vec<Entry>) -> Vec<Entry> {
    let mut tidy: Vec<Entry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let separator = matches!(entry, Entry::Separator);
        let after_separator = matches!(tidy.last(), None | Some(Entry::Separator));
        if !(separator && after_separator) {
            tidy.push(entry);
        }
    }
    if matches!(tidy.last(), Some(Entry::Separator)) {
        tidy.pop();
    }
    tidy
}

/// The menu as text, one line per entry: `-` for a separator, `(disabled)` after a disabled
/// label, and `[x]` or `[ ]` before a choice.
#[cfg(test)]
pub fn summary(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Separator => "-".to_owned(),
            Entry::Item(item) => {
                let check = match item.checked {
                    Some(true) => "[x] ",
                    Some(false) => "[ ] ",
                    None => "",
                };
                let disabled = if item.disabled { " (disabled)" } else { "" };
                format!("{check}{}{disabled}", item.label)
            }
        })
        .collect()
}

/// Builds the Zed menu. `context` is the focus actions dispatch from.
pub fn build(
    entries: Vec<Entry>,
    context: Option<FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ContextMenu> {
    ContextMenu::build(window, cx, move |menu, _, _| {
        let menu = match &context {
            Some(context) => menu.context(context.clone()),
            None => menu,
        };
        entries.into_iter().fold(menu, |menu, entry| match entry {
            Entry::Separator => menu.separator(),
            Entry::Item(item) => menu.item(zed_entry(item, context.clone())),
        })
    })
}

fn zed_entry(item: Item, context: Option<FocusHandle>) -> ContextMenuEntry {
    let entry = ContextMenuEntry::new(item.label).disabled(item.disabled);
    let entry = match item.checked {
        Some(checked) => entry.toggleable(IconPosition::Start, checked),
        None => entry,
    };
    match item.choice {
        Choice::Dispatch(action) => {
            entry
                .action(action.boxed_clone())
                .handler(move |window, cx| {
                    if let Some(context) = &context {
                        window.focus(context, cx);
                    }
                    window.dispatch_action(action.boxed_clone(), cx);
                })
        }
        Choice::Run { run, keys } => {
            let entry = entry.handler(move |window, cx| run(window, cx));
            match keys {
                Some(keys) => entry.action(keys),
                None => entry,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidy_drops_leading_trailing_and_doubled_separators() {
        let entries = vec![
            Entry::Separator,
            Item::run("Open", |_, _| {}).done(),
            Entry::Separator,
            Entry::Separator,
            Item::run("Close", |_, _| {}).disabled(true).done(),
            Entry::Separator,
        ];
        assert_eq!(summary(&tidy(entries)), ["Open", "-", "Close (disabled)"]);
    }
}
