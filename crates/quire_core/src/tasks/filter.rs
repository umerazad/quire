use super::Task;
use crate::markdown::js_space;

/// The Tasks filter: words must appear in the task or its page; `-word` and `-#tag` hide tasks
/// that contain the word or carry the tag or a tag nested under it.
#[derive(Clone, Debug, Default)]
pub struct TaskFilter {
    needle: String,
    exclude_words: Vec<String>,
    exclude_tags: Vec<String>,
}

impl TaskFilter {
    pub fn new(query: &str) -> Self {
        let mut include = Vec::new();
        let mut filter = Self::default();
        let query = query.to_lowercase();
        for word in query.split(js_space).filter(|w| !w.is_empty()) {
            if word.starts_with("-#") && word.len() > 2 {
                filter.exclude_tags.push(word[1..].to_owned());
            } else if let Some(rest) = word.strip_prefix('-').filter(|rest| !rest.is_empty()) {
                filter.exclude_words.push(rest.to_owned());
            } else {
                include.push(word);
            }
        }
        filter.needle = include.join(" ");
        filter
    }

    pub fn matches(&self, task: &Task, page: &str) -> bool {
        self.matches_parts(&task.text, &task.tags, page)
    }

    /// [`TaskFilter::matches`] over a task's shown text and lowercased tags.
    pub fn matches_parts(&self, text: &str, tags: &[String], page: &str) -> bool {
        let haystack = format!("{text} {page}").to_lowercase();
        if !self.needle.is_empty() && !haystack.contains(&self.needle) {
            return false;
        }
        if self
            .exclude_words
            .iter()
            .any(|word| haystack.contains(word))
        {
            return false;
        }
        !self.exclude_tags.iter().any(|excluded| {
            tags.iter().any(|tag| {
                let tag = tag.to_lowercase();
                tag == *excluded
                    || tag
                        .strip_prefix(excluded.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
        })
    }
}

/// The projects a task belongs to: the project page it sits on, project pages it links, and
/// the project page whose front matter carries a `project/<name>` tag the task has. `projects`
/// pairs each project page with its front matter tags; `find_page` resolves a link to a page.
pub fn project_membership<'p>(
    page: &str,
    task: &Task,
    projects: &'p [(String, Vec<String>)],
    find_page: impl Fn(&str) -> Option<String>,
) -> Vec<&'p str> {
    let mut members: Vec<&str> = Vec::new();
    let mut add = |path: &'p str| {
        if !members.contains(&path) {
            members.push(path);
        }
    };
    let project = |path: &str| projects.iter().find(|(project, _)| project == path);
    if let Some((path, _)) = project(page) {
        add(path);
    }
    for link in &task.links {
        if let Some((path, _)) = find_page(link).and_then(|path| project(&path)) {
            add(path);
        }
    }
    for tag in &task.tags {
        let name = tag.strip_prefix('#').unwrap_or(tag).to_lowercase();
        let owner = projects.iter().rev().find(|(_, tags)| {
            tags.iter().any(|front| {
                front.to_lowercase() == name
                    && front
                        .get(..8)
                        .is_some_and(|head| head.eq_ignore_ascii_case("project/"))
                    && front.len() > 8
            })
        });
        if let Some((path, _)) = owner {
            add(path);
        }
    }
    members
}
