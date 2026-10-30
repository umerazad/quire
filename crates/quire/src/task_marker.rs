pub const PREFIX_CHAR_LIMIT: usize = 13;

pub struct TaskMarker<'a> {
    pub source: &'a str,
    pub checked: bool,
}

impl TaskMarker<'_> {
    pub fn toggled(&self) -> String {
        if self.source.starts_with('[') {
            if self.checked { "[ ]" } else { "[x]" }.into()
        } else {
            self.source.replace(
                if self.checked { "DONE" } else { "TODO" },
                if self.checked { "TODO" } else { "DONE" },
            )
        }
    }
}

pub fn read(text: &str) -> Option<TaskMarker<'_>> {
    [
        "[ ]",
        "[x]",
        "[X]",
        "{{TODO}}",
        "{{DONE}}",
        "{{[[TODO}}",
        "{{[[DONE}}",
        "{{TODO]]}}",
        "{{DONE]]}}",
        "{{[[TODO]]}}",
        "{{[[DONE]]}}",
    ]
    .into_iter()
    .find(|source| {
        text.strip_prefix(source)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
    })
    .map(|source| TaskMarker {
        source,
        checked: source == "[x]" || source == "[X]" || source.contains("DONE"),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn task_markers_preserve_legacy_spelling_and_boundaries() {
        for (input, checked, toggled) in [
            ("[ ] task", false, "[x]"),
            ("[X]\tمرحبا", true, "[ ]"),
            ("{{[[TODO]]}} task", false, "{{[[DONE]]}}"),
            ("{{DONE}}", true, "{{TODO}}"),
            ("{{[[TODO}} task", false, "{{[[DONE}}"),
            ("{{DONE]]}} task", true, "{{TODO]]}}"),
        ] {
            let marker = super::read(input).unwrap();
            assert_eq!(marker.checked, checked);
            assert_eq!(marker.toggled(), toggled);
        }
        for input in [
            "[x]body",
            "[-] task",
            "[ ]\nnext",
            " {{TODO}} task",
            "{{todo}} task",
            "{{TODO}}🙂",
        ] {
            assert!(super::read(input).is_none(), "{input:?}");
        }
    }
}
