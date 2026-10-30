//! Linked-reference filters: `#tag`, `[[Page]]`, `project:Name`, words, and `"quoted words"`,
//! each negatable with `-`. A reference matches when every term holds for its context: the
//! referring block, its parents, its children, and its page. Ported from rt `42de3c8949e1`
//! `src/core/reference-filter.ts`.

use crate::markdown::{JS_SPACE, js_space};
use regex::Regex;
use std::{
    collections::{BTreeSet, HashMap},
    sync::LazyLock,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReferenceFilter {
    pub tags: Vec<String>,
    pub not_tags: Vec<String>,
    pub pages: Vec<String>,
    pub not_pages: Vec<String>,
    pub projects: Vec<String>,
    pub not_projects: Vec<String>,
    pub words: Vec<String>,
    pub not_words: Vec<String>,
}

/// What one reference is about. Tags carry `#` and are lowercase; links are lowercase page
/// names, the referring page's own names included; text is the title and context, lowercase.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReferenceContext {
    pub tags: BTreeSet<String>,
    pub links: BTreeSet<String>,
    pub text: String,
}

/// Lowercase names that count as linking to each `[[page]]` and `project:name` term.
#[derive(Clone, Debug, Default)]
pub struct FilterNames {
    pub pages: HashMap<String, Vec<String>>,
    pub projects: HashMap<String, Vec<String>>,
}

static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"-?\[\[[^\]]*\]\]|-?"[^"]*"?|[^{}]+"#,
        &JS_SPACE[1..JS_SPACE.len() - 1]
    ))
    .expect("the filter token pattern is valid")
});

/// The reference `queryTokens`: bracketed pages and quoted phrases stay whole.
pub fn query_tokens(source: &str) -> impl Iterator<Item = &str> {
    TOKEN.find_iter(source).map(|token| token.as_str())
}

pub const UNCLOSED_QUOTE: &str = "A quote is not closed. Add the closing \".";

impl ReferenceFilter {
    pub fn parse(source: &str) -> Result<Self, &'static str> {
        let len16 = |text: &str| text.encode_utf16().count();
        let mut filter = Self::default();
        for token in query_tokens(source) {
            let negated = token.starts_with('-') && len16(token) > 1;
            let body = if negated { &token[1..] } else { token };
            let lower = body.to_lowercase();
            let (terms, value) = if body.starts_with('"') {
                if len16(body) < 2 || !body.ends_with('"') {
                    return Err(UNCLOSED_QUOTE);
                }
                (Term::Word, lower[1..lower.len() - 1].to_owned())
            } else if body.starts_with("[[") && body.ends_with("]]") {
                (
                    Term::Page,
                    body[2..body.len() - 2].trim_matches(js_space).to_owned(),
                )
            } else if body.starts_with('#') && len16(body) > 1 {
                (Term::Tag, lower)
            } else if lower.starts_with("project:") && len16(body) > 8 {
                (Term::Project, lower["project:".len()..].to_owned())
            } else {
                (Term::Word, lower)
            };
            let (yes, no) = match terms {
                Term::Tag => (&mut filter.tags, &mut filter.not_tags),
                Term::Page => (&mut filter.pages, &mut filter.not_pages),
                Term::Project => (&mut filter.projects, &mut filter.not_projects),
                Term::Word => (&mut filter.words, &mut filter.not_words),
            };
            if negated { no } else { yes }.push(value);
        }
        Ok(filter)
    }

    pub fn is_empty(&self) -> bool {
        [
            &self.tags,
            &self.not_tags,
            &self.pages,
            &self.not_pages,
            &self.projects,
            &self.not_projects,
            &self.words,
            &self.not_words,
        ]
        .iter()
        .all(|terms| terms.is_empty())
    }

    /// The reference `referenceMatcher` applied to one context.
    pub fn matches(&self, names: &FilterNames, context: &ReferenceContext) -> bool {
        let has_tag = |tag: &String| {
            context.tags.iter().any(|own| {
                own == tag
                    || own
                        .strip_prefix(tag.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
        };
        let links = |table: &HashMap<String, Vec<String>>, name: &String| {
            table
                .get(name)
                .is_some_and(|names| names.iter().any(|name| context.links.contains(name)))
        };
        let page = |name: &String| links(&names.pages, name);
        let project = |name: &String| links(&names.projects, name);
        let word = |word: &String| context.text.contains(word.as_str());
        self.tags.iter().all(has_tag)
            && !self.not_tags.iter().any(has_tag)
            && self.pages.iter().all(page)
            && !self.not_pages.iter().any(page)
            && self.projects.iter().all(project)
            && !self.not_projects.iter().any(project)
            && self.words.iter().all(word)
            && !self.not_words.iter().any(word)
    }
}

/// How a filter uses a suggested term: shown only, hidden, or not at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermState {
    Include,
    Exclude,
}

/// Whether the filter text already includes or excludes `term`, ignoring case.
pub fn term_state(text: &str, term: &str) -> Option<TermState> {
    let lower = term.to_lowercase();
    query_tokens(text).find_map(|token| {
        let token = token.to_lowercase();
        if token == lower {
            Some(TermState::Include)
        } else if token.strip_prefix('-') == Some(lower.as_str()) {
            Some(TermState::Exclude)
        } else {
            None
        }
    })
}

/// The reference `toggleTerm`: adds the term, or `-term` when excluding, or takes it out when
/// already there. Excluding a term replaces including it, and the other way round.
pub fn toggle_term(text: &str, term: &str, exclude: bool) -> String {
    let lower = term.to_lowercase();
    let wanted = if exclude {
        format!("-{lower}")
    } else {
        lower.clone()
    };
    let tokens: Vec<&str> = query_tokens(text).collect();
    let present = tokens.iter().any(|token| token.to_lowercase() == wanted);
    let mut kept: Vec<String> = tokens
        .into_iter()
        .filter(|token| {
            let token = token.to_lowercase();
            token.strip_prefix('-').unwrap_or(&token) != lower
        })
        .map(str::to_owned)
        .collect();
    if !present {
        kept.push(if exclude {
            format!("-{term}")
        } else {
            term.to_owned()
        });
    }
    kept.join(" ")
}

enum Term {
    Tag,
    Page,
    Project,
    Word,
}
