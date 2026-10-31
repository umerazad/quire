//! `jj` at the start of an empty block lists the Templates page's templates in the editor's
//! completion menu, and typing after it narrows the list. Choosing one expands it, asking each
//! question once, and replaces the trigger line in one undo step: inserted blocks get fresh IDs
//! with their internal links following, the first keeps the trigger block's ID, and the caret
//! lands at `{cursor}`. A changed template, a dismissed question, an unknown date, or a page that
//! changed meanwhile stops before editing, with a notice saying what to do.

use crate::{
    notebook_session::OpenPage,
    query_index::ask,
    references::{Notebook, ReferenceHost},
    tasks::TaskContext,
};
use editor::{Editor, SelectionEffects};
use gpui::{App, AsyncWindowContext, Context, Entity, SharedString, Task, WeakEntity, Window};
use language::{Buffer, BufferSnapshot, CodeLabel};
use multi_buffer::{ToOffset as _, ViewEditIntent};
use project::{
    Completion, CompletionDisplayOptions, CompletionResponse, CompletionSource,
    lsp_store::CompletionDocumentation,
};
use quire_core::{
    markdown::{ReferenceKind, copy_with_new_ids, page_name},
    templates::{
        self as core, CANCELLED, Clock, Context as Fill, Definition, Question, Step, TagUse,
        is_templates_page,
    },
};
use std::{collections::HashMap, ops::Range, rc::Rc, sync::Arc};

const NO_INDEX: &str =
    "Templates need the notebook index, which is still starting. Type jj again in a moment.";
const NO_PAGE: &str =
    "The Templates page could not be read. Open it from the sidebar, then type jj again.";
const CHILDREN: &str = "This block has children. Insert a template in an empty sibling block.";
const PAGE_CHANGED: &str = "The page changed during template expansion. Select the template again.";
const NO_CLOCK: &str = "Templates need the system time zone for dates and times. Set a valid time zone and reopen the notebook.";
const UNPARSED: &str = "The template could not be parsed. Check its Markdown on the Templates page, then type jj again.";

/// Where `jj` ends in a line typed up to the caret: after optional indentation and a list
/// marker, as the reference trigger `^[ \t]*(?:(?:[-+*]|\d+[.)])\s+)?jj` reads it.
pub(crate) fn trigger(prefix: &str) -> Option<usize> {
    let indent = prefix.len() - prefix.trim_start_matches([' ', '\t']).len();
    let rest = &prefix[indent..];
    let marker = match rest.chars().next()? {
        '-' | '+' | '*' => 1,
        '0'..='9' => {
            let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if rest[digits..].starts_with(['.', ')']) {
                digits + 1
            } else {
                0
            }
        }
        _ => 0,
    };
    let after = &rest[marker..];
    let spaced = after.len() - after.trim_start_matches(char::is_whitespace).len();
    let start = if marker > 0 && spaced > 0 {
        indent + marker + spaced
    } else {
        indent
    };
    prefix[start..].starts_with("jj").then_some(start + 2)
}

/// The `jj` trigger before `offset`, outside code blocks, with the offset after `jj`.
pub(crate) fn trigger_at(snapshot: &BufferSnapshot, offset: usize) -> Option<usize> {
    let row = snapshot.offset_to_point(offset).row;
    let line = snapshot.point_to_offset(text::Point::new(row, 0));
    let prefix: String = snapshot.text_for_range(line..offset).collect();
    let after = line + trigger(&prefix)?;
    let blocks = crate::markdown_blocks::current_blocks(snapshot)?;
    let in_code = blocks
        .iter()
        .any(|block| block.kind == "code" && block.from <= offset && block.subtree_to >= offset);
    (!in_code).then_some(after)
}

/// The Templates page as the completion menu listed it.
#[derive(Clone)]
struct Source {
    page: String,
    title: String,
}

/// The templates the Templates page holds now, read through its shared buffer.
async fn current(
    notebook: &Rc<dyn Notebook>,
    page: &str,
    cx: &mut AsyncWindowContext,
) -> Result<(String, Vec<Definition>), SharedString> {
    let open: Option<Entity<OpenPage>> = cx
        .update(|window, cx| notebook.open_page(page, window, cx))
        .ok()
        .flatten();
    let open = open.ok_or(SharedString::new_static(NO_PAGE))?;
    let buffer = super::loaded(open.clone(), cx)
        .await
        .ok_or(SharedString::new_static(NO_PAGE))?;
    cx.update(|_, cx| {
        let path = open.read(cx).path(cx)?;
        let definitions = super::definitions(&path, &buffer.read(cx).snapshot())?;
        Some((path, definitions))
    })
    .ok()
    .flatten()
    .ok_or(SharedString::new_static(NO_PAGE))
}

/// The completion menu for a `jj` typed before `caret`, `after` being the offset after `jj`.
pub(crate) fn completions(
    host: &Entity<ReferenceHost>,
    buffer: &Entity<Buffer>,
    after: usize,
    caret: usize,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Task<anyhow::Result<Vec<CompletionResponse>>> {
    let Some(index) = host.read(cx).index().cloned() else {
        crate::outline::set_notice(Some(NO_INDEX), cx);
        return Task::ready(Ok(Vec::new()));
    };
    let notebook = host.read(cx).notebook().clone();
    let snapshot = buffer.read(cx).text_snapshot();
    let range = snapshot.anchor_after(caret)..snapshot.anchor_after(caret);
    let match_start = snapshot.anchor_before(after);
    let host = host.downgrade();
    cx.spawn_in(window, async move |editor, cx| {
        let pages = ask(index.pages()).await.map_err(anyhow::Error::msg)?;
        let Some(listed) = pages
            .into_iter()
            .find(|listed| is_templates_page(&listed.page.path))
        else {
            return Ok(Vec::new());
        };
        let source = Source {
            page: listed.page.page.to_lossy_string(),
            title: listed.page.title,
        };
        let (_, definitions) = current(&notebook, &source.page, cx)
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let completions = core::catalogue(definitions)
            .into_iter()
            .map(|definition| Completion {
                replace_range: range.clone(),
                new_text: String::new(),
                label: CodeLabel::plain(definition.name.clone(), None),
                documentation: Some(CompletionDocumentation::SingleLine(
                    source.title.clone().into(),
                )),
                source: CompletionSource::Custom,
                icon_path: None,
                icon_color: None,
                match_start: Some(match_start),
                snippet_deduplication_key: None,
                insert_text_mode: None,
                confirm: Some(on_confirm(
                    editor.clone(),
                    host.clone(),
                    source.clone(),
                    definition,
                )),
                group: None,
            })
            .collect();
        Ok(vec![CompletionResponse {
            completions,
            display_options: CompletionDisplayOptions::default(),
            is_incomplete: false,
        }])
    })
}

type Confirm = Arc<dyn Send + Sync + Fn(project::CompletionIntent, &mut Window, &mut App) -> bool>;

/// Starts inserting once Zed has closed the menu; the editor is still being updated here.
fn on_confirm(
    editor: WeakEntity<Editor>,
    host: WeakEntity<ReferenceHost>,
    source: Source,
    listed: Definition,
) -> Confirm {
    Arc::new(move |_, window, cx| {
        let (editor, host, source, listed) =
            (editor.clone(), host.clone(), source.clone(), listed.clone());
        window.defer(cx, move |window, cx| {
            if let (Some(editor), Some(host)) = (editor.upgrade(), host.upgrade()) {
                start(editor, host, source, listed, window, cx);
            }
        });
        false
    })
}

/// The trigger line as it was when the template was chosen.
struct Trigger {
    buffer: Entity<Buffer>,
    before: BufferSnapshot,
    line: Range<usize>,
    indent: String,
    id: Option<String>,
    path: String,
    title: String,
}

fn notice(message: impl Into<SharedString>, cx: &mut App) {
    crate::outline::show_notice(message.into(), cx);
}

fn start(
    editor: Entity<Editor>,
    host: Entity<ReferenceHost>,
    source: Source,
    listed: Definition,
    window: &mut Window,
    cx: &mut App,
) {
    match prepare(&editor, &host, cx) {
        Ok((trigger, clock)) => {
            let notebook = host.read(cx).notebook().clone();
            let index = host.read(cx).index().cloned();
            let editor = editor.downgrade();
            window
                .spawn(cx, async move |cx| {
                    let result =
                        insert(&notebook, index, &source, &listed, &trigger, clock, cx).await;
                    let applied = match result {
                        Ok((text, cursor)) => editor
                            .update_in(cx, |editor, window, cx| {
                                apply(editor, &trigger, text, cursor, window, cx)
                            })
                            .unwrap_or(Ok(())),
                        Err(error) => Err(error),
                    };
                    if let Err(error) = applied {
                        cx.update(|_, cx| notice(error, cx)).ok();
                    }
                })
                .detach();
        }
        Err(error) => notice(error, cx),
    }
}

fn prepare(
    editor: &Entity<Editor>,
    host: &Entity<ReferenceHost>,
    cx: &mut App,
) -> Result<(Trigger, Clock), SharedString> {
    let (buffer, caret) = {
        let editor = editor.read(cx);
        let multi = editor.buffer().read(cx);
        let head = editor
            .selections
            .newest_anchor()
            .head()
            .to_offset(&multi.snapshot(cx));
        let (buffer, offset) = multi
            .point_to_buffer_offset(head, cx)
            .ok_or(SharedString::new_static(PAGE_CHANGED))?;
        (buffer, offset.0)
    };
    let before = buffer.read(cx).snapshot();
    let row = before.offset_to_point(caret).row;
    let start = before.point_to_offset(text::Point::new(row, 0));
    let end = start + before.line_len(row) as usize;
    let indent: String = before
        .chars_at(start)
        .take_while(|c| matches!(c, ' ' | '\t'))
        .collect();
    let blocks = crate::markdown_blocks::current_blocks(&before)
        .ok_or(SharedString::new_static(PAGE_CHANGED))?;
    let node = blocks
        .iter()
        .find(|block| block.from >= start && block.from <= end);
    if let Some(node) = node
        && blocks
            .iter()
            .any(|block| block.parent.as_deref() == Some(&node.key))
    {
        return Err(CHILDREN.into());
    }
    let context = cx.global::<TaskContext>();
    let calendar = context
        .calendar
        .as_ref()
        .ok_or(SharedString::new_static(NO_CLOCK))?;
    let (date, hour, minute) = calendar.clock_at((context.now)())?;
    let notebook = host.read(cx).notebook().clone();
    let path = notebook.page_path(&buffer, cx).unwrap_or_default();
    let title = (context.title)(&buffer, cx).unwrap_or_else(|| {
        page_name(&path)
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned()
    });
    Ok((
        Trigger {
            id: node.and_then(|node| node.id.clone()),
            buffer,
            before,
            line: start..end,
            indent,
            path,
            title,
        },
        Clock { date, hour, minute },
    ))
}

fn new_id() -> String {
    format!("b-{}", uuid::Uuid::new_v4().simple())
}

/// Expands the chosen template for the trigger line: the replacement text and the caret in it.
async fn insert(
    notebook: &Rc<dyn Notebook>,
    index: Option<quire_store::index::IndexClient>,
    source: &Source,
    listed: &Definition,
    trigger: &Trigger,
    clock: Clock,
    cx: &mut AsyncWindowContext,
) -> Result<(String, usize), SharedString> {
    let (path, definitions) = current(notebook, &source.page, cx).await?;
    let template = core::current(listed, &definitions)?.clone();
    let body = super::parse(&template.body, cx)
        .await
        .ok_or(SharedString::new_static(UNPARSED))?;
    let names = [page_name(&path), source.title.as_str()];
    let copied = copy_with_new_ids(
        &template.body,
        &body.addresses(&template.body),
        &trigger.path,
        &names,
        new_id,
    );
    let parsed = super::parse(&copied, cx)
        .await
        .ok_or(SharedString::new_static(UNPARSED))?;
    let days = cx
        .update(|_, cx| cx.global::<TaskContext>().days())
        .ok()
        .flatten()
        .ok_or(SharedString::new_static(NO_CLOCK))?;
    let mut answers = HashMap::new();
    let expanded = loop {
        let fill = Fill {
            clock: &clock,
            page_title: &trigger.title,
            days: &days,
            answers: &answers,
        };
        match core::expand(&copied, &parsed, &fill)? {
            Step::Done(expanded) => break expanded,
            Step::Ask(question) => {
                let prompt = prompt(&question, index.as_ref(), &days, cx).await?;
                let answer = cx
                    .update(|window, cx| notebook.prompt(prompt, window, cx))
                    .map_err(|_| SharedString::new_static(CANCELLED))?
                    .await
                    .ok()
                    .flatten()
                    .ok_or(SharedString::new_static(CANCELLED))?;
                answers.insert(question.key(), answer);
            }
        }
    };
    let preserved = match &trigger.id {
        Some(id) => {
            let parsed = super::parse(&expanded.text, cx)
                .await
                .ok_or(SharedString::new_static(UNPARSED))?;
            Some(core::preserve_root(&expanded.text, id, &parsed))
        }
        None => None,
    };
    Ok(core::placement(
        &expanded,
        preserved.as_deref(),
        &trigger.indent,
    ))
}

/// The picker or question for one placeholder. A pick lists the names used with its tag, read
/// from the index, so tagged pages need not be open.
async fn prompt(
    question: &Question,
    index: Option<&quire_store::index::IndexClient>,
    days: &quire_core::tasks::DailyNames,
    cx: &mut AsyncWindowContext,
) -> Result<super::prompt::Prompt, SharedString> {
    let tag = match question {
        Question::Ask(question) => {
            return Ok(super::prompt::Prompt::Ask {
                question: question.clone(),
            });
        }
        Question::Pick(tag) => tag.clone(),
    };
    let index = index.ok_or(SharedString::new_static(NO_INDEX))?;
    let tagged = ask(index.tagged_blocks(tag.clone()))
        .await
        .map_err(|error| format!("Cannot list names used with {tag}: {error}. Wait for indexing to finish, then type jj again."))?;
    let mut uses = Vec::with_capacity(tagged.len());
    for block in tagged {
        let links = super::parse(&block.tag.snippet, cx)
            .await
            .map(|parsed| {
                parsed
                    .references
                    .into_iter()
                    .filter(|reference| reference.kind == ReferenceKind::Page)
                    .map(|reference| reference.target)
                    .collect()
            })
            .unwrap_or_default();
        uses.push(TagUse {
            path: block.source.path,
            title: block.source.title,
            links,
        });
    }
    Ok(super::prompt::Prompt::Pick {
        names: core::past_names(&uses, days),
        tag,
    })
}

/// Replaces the trigger line in one undo step, unless the page changed since the template was
/// chosen.
fn apply(
    editor: &mut Editor,
    trigger: &Trigger,
    text: String,
    cursor: usize,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Result<(), SharedString> {
    let snapshot = trigger.buffer.read(cx).snapshot();
    if snapshot.version() != trigger.before.version() {
        return Err(PAGE_CHANGED.into());
    }
    let multi = editor.buffer().clone();
    let range = multi
        .read(cx)
        .snapshot(cx)
        .buffer_anchor_range_to_anchor_range(
            snapshot.anchor_before(trigger.line.start)..snapshot.anchor_after(trigger.line.end),
        )
        .ok_or(SharedString::new_static(PAGE_CHANGED))?;
    crate::outline::set_notice(None, cx);
    editor.finalize_last_transaction(cx);
    editor.transact(window, cx, |editor, window, cx| {
        {
            let _intent = multi
                .read(cx)
                .scoped_edit_intent(ViewEditIntent::Structural);
            multi.update(cx, |multi, cx| multi.edit([(range, text)], None, cx));
        }
        let caret = trigger
            .buffer
            .read(cx)
            .snapshot()
            .anchor_after(trigger.line.start + cursor);
        if let Some(caret) = multi.read(cx).snapshot(cx).anchor_in_buffer(caret) {
            editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
                selections.select_anchor_ranges([caret..caret])
            });
        }
    });
    editor.finalize_last_transaction(cx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::trigger;

    #[test]
    fn triggers_follow_the_reference_line_pattern() {
        assert_eq!(trigger("jj"), Some(2));
        assert_eq!(trigger("- jj"), Some(4));
        assert_eq!(trigger("  - jjmee"), Some(6));
        assert_eq!(trigger("\t* \tjj"), Some(6));
        assert_eq!(trigger("12. jj"), Some(6));
        assert_eq!(trigger("3) jj"), Some(5));
        assert_eq!(trigger("+ jj and more"), Some(4));
        assert_eq!(trigger("-jj"), None);
        assert_eq!(trigger("- [ ] jj"), None);
        assert_eq!(trigger("- text jj"), None);
        assert_eq!(trigger("12 jj"), None);
        assert_eq!(trigger("- j"), None);
        assert_eq!(trigger(""), None);
    }
}
