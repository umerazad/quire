//! Help topics, ported from the reference guide. Shortcut tables are not written here: the
//! reference beneath the topics is generated from the live keymap.

/// One topic: what it is for, the steps that use it, and an optional Markdown example.
pub struct HelpSection {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub steps: &'static [&'static str],
    pub example: Option<&'static str>,
}
pub const SECTIONS: &[HelpSection] = &[
    HelpSection {
        id: "start",
        title: "Start here",
        description: "Your notes are Markdown files. Write edits the formatted page; Source shows the original Markdown. Both share the same content and undo history.",
        steps: &[
            "Vim is on by default. Click the editor and press i to type. Press Escape to return to Normal mode. Turn Vim off in Settings (Command/Ctrl-,) if you prefer ordinary editing.",
            "Open a folder to use a vault or Git working tree. The notebook picker switches Work and Personal. Files shows the physical folders; Pages browses logical page titles.",
            "Open destinations become tabs. Right-click a tab, or press Shift-F10 on a focused tab, for Close, Close others, Close tabs to the left or right, Close all, favorites, and, for pages, Open beside, Copy link and Reveal in Finder / Explorer. Middle-click closes a tab.",
            "Edits save automatically. Check the bottom status bar and resolve any save or recovery warning. Help opens in its own window with F1, is available offline and does not add files to your notebook.",
        ],
        example: None,
    },
    HelpSection {
        id: "tasks",
        title: "Tasks: capture → clarify → do → review",
        description: "A task is a list checkbox anywhere in your notes. Tasks collects these actions without moving them out of their source pages. Imported Roam TODO/DONE macros work too.",
        steps: &[
            "Capture: open Tasks, type into the capture field and press Enter. The action is appended to your Inbox note, or to today's daily page if Settings > Tasks > Quick capture goes to says so. Select a project in the capture field if it belongs to one. Unsubmitted capture text is remembered on this computer.",
            "Clarify: Inbox holds undecided actions. Next is a concrete step you can do. Waiting means someone else must act. Someday holds possibilities. Use the state selector on a task card, or add a state tag in its note. To triage many at once, click the slim handle at a card's left edge (Shift-click selects a range), or focus a card and press Space; the toolbar then moves every selected task to one state, as one undo step per page. Escape clears the selection.",
            "Do: work from Next and check actions off. Done collects completed tasks. Reopen a task to restore its previous state; changing its state also reopens it. Cycling a completed task starts at Inbox.",
            "Create a task in a note: place the cursor in a list block, paragraph or empty line and use the task shortcut. A new task starts in Inbox unless the block already has a state tag. On an existing task, the same shortcut completes or reopens it. It never removes the task text. Select several lines first to make each line its own task in one step; if they are all tasks already, each one is completed or reopened.",
            "Review projects: mark a page #project, give it a project or project/<name> tag in its front matter, or use a logical title such as projects/Website. Tasks in that page and tasks linking to [[Website]] belong to it. A project link on a parent block also applies to its child tasks. A task tagged #project/<name> belongs to the page whose front matter carries that tag. Projects shows open/done counts and missing next actions. Choose Active (projects with an open action), Inactive or All.",
            "Edit details: click the source page beneath a task. Shift-click opens it in the second pane. Filter task text or source page, and use the project filter to focus. Start a word with - to hide matches: -#cloudmizan hides that tag and the tags nested under it. Task edits share ordinary note autosave, recovery and undo.",
            "Dates: capturing a task or making one with Mod-Enter adds ➕ and today's link, so it shows in that day's Linked references. Click the date icons on a task row to set its created, due or completed date. A 📅 date wins; otherwise link a task to a daily page, such as [[2026-10-30]], to make it due that day. A link on a parent block applies to its child tasks, and a task on a daily page is due that day. Due lists open tasks due today or earlier, oldest first; Upcoming lists open tasks due in the next 7 days, soonest first; other open lists put dated tasks first. Completing a task appends ✅ and the date; reopening removes it. Done shows the newest completions first.",
            "On a tag page, All, Open, Due and Done filter the tagged tasks. The choice is remembered for each tag.",
            "Linked references on a page offer the same filters for tasks that link to it, directly or from a parent line. Write action items under a [[Weekly sync]] line, then open Weekly sync and choose Open to see every unfinished item from past meetings. References from daily pages list newest first; the Newest first button switches that page to oldest first.",
            "State tags are #inbox, #next, #waiting, #someday and #done; #pending reads as Waiting. Roam forms such as #[[waiting]] are recognized. A checked box or #done means Done. Code examples and linked tag labels do not change task state.",
        ],
        example: Some(
            "- [ ] Decide what this needs #inbox\n- [ ] Send proposal [[Website]] #next\n- [ ] Await feedback #waiting\n- [ ] Explore an idea #someday\n- [x] Publish draft #next",
        ),
    },
    HelpSection {
        id: "blocks",
        title: "Blocks, nesting and folding",
        description: "Build an outline with list blocks. Every block has the same dot, and vertical guides show nesting depth. A ring around a dot means its children are folded.",
        steps: &[
            "Plain text stays plain text when you press Enter. To start an outline, type - followed by a space. In a full-page Write editor, Enter then creates a sibling list block; at the start of a block it adds an empty block above, and in a block with open children it starts the first child. Tab nests the block and its children under the previous sibling. Shift-Tab moves the subtree out after the parent's remaining children. With Vim enabled, use Insert mode for these editing keys.",
            "Enter on an empty list item moves it out one level; at the top level it exits the outline. Ordinary paragraphs, headings, code and tables keep their Markdown editing behavior. Embedded roots retain their existing editing boundary.",
            "The caret skips hidden bullets and block IDs: Left at the start of a block reaches the end of the one above, and Home and End stop at the block's text. Backspace at the start of a block joins it to the block above, and Delete at the end pulls the next block in; children stay nested. A task's first Backspace removes its checkbox. Backspace or Delete on an empty bullet, even the first on the page, removes it and moves its children up. A block with an ID is not merged, since references point at it. Alt-Up and Alt-Down, or Mod-Shift-Up and Mod-Shift-Down, move a block and its children past its neighbours. Pasting several lines makes each line a block. Numbered lists renumber themselves as items move.",
            "While zoomed into a block, edits stay inside it and its children, and Mod-A selects only that. With Vim, o, O, J, cc and >> or << work on blocks: o opens a sibling, J joins the next block, and >> nests the block with its children.",
            "Hover a block with children to show the arrow to the left of its dot; click the arrow to collapse or expand them. Click a dot to zoom into that block. The Source folding gutter also folds blocks. Folding does not modify your note.",
            "For a numbered list, type 1. and a space; Enter continues with 2., 3. and so on, and Tab nests a block under the number. To number an existing bullet list, put the cursor in any item and press Mod-Shift-7; press it again to turn the numbers back into bullets. With a selection, only the selected items change. Every item can be typed as 1.: the list still counts 1, 2, 3.",
            "Use Page actions to copy references or embeds, move a subtree, attach a file or open saved history. Referenced block IDs stay attached to their blocks.",
        ],
        example: None,
    },
    HelpSection {
        id: "links",
        title: "Links, embeds and tags",
        description: "Use page links for topics, block references for precise thoughts and embeds to work on a source subtree in place.",
        steps: &[
            "Type [[ to complete a page name, (( to find a block, or # to complete an existing tag. Use the suggestion menu to choose the result.",
            "An inline ((block-id)) shows a compact reference. Put it alone on a line for an editable embed containing the entire subtree. {{embed: ((block-id))}} also works. Editing an embed edits its source.",
            "Tags in the sidebar lists every tag with its block count. Sort by name, most blocks first or fewest blocks first; the choice is remembered. Tick tags to rename or delete them together after a preview; renaming moves nested tags too.",
            "Click a tag to browse matching blocks with parent context and expandable children. Common tags such as #inbox, #pending, #book, #quote and #idea show an icon and their own color; nested tags such as #book/fiction keep their family's look. Source always shows raw text.",
            "Shift-click page links or source links to open beside the current page. The page’s details panel shows the outline and backlinks. Mouse Back/Forward buttons navigate where supported by the operating system.",
            "Navigate history with ⌘+[ / ⌘+] on Mac, or Alt+Left / Alt+Right on Windows. Browser Back/Forward keys also work. Back/Forward restores tag filters and reading positions.",
        ],
        example: Some(
            "[[Website]]\nA precise thought: ((block-id))\n{{embed: ((block-id))}}\n#meeting/team",
        ),
    },
    HelpSection {
        id: "vim",
        title: "Vim essentials",
        description: "Vim applies to note editors, including editable embeds. Capture, search and other ordinary input fields use standard text entry.",
        steps: &[
            "Normal mode moves and changes text; Insert mode types; Visual mode selects. Your current mode appears in the editor’s bottom status bar.",
            "In Normal mode, h/j/k/l move left/down/up/right, w and b move by words, 0 and $ move to line boundaries, and gg / G jump to the document’s start / end.",
            "Use dd to delete a line, yy to copy a line, p to paste, and v to begin a visual selection. / searches forward; n repeats the search. Escape cancels a partial command or returns to Normal.",
            "When focus is outside a text field, for example on a tab or a toolbar button, i, a, o, I, A, O, s, S and C still start Insert mode: they go to the page editor used last, or the open page.",
            "Task editor shortcuts work in Normal and Insert modes. Settings can turn Vim off across all editors, at once.",
        ],
        example: None,
    },
    HelpSection {
        id: "templates",
        title: "Templates, images, PDFs and code",
        description: "Reusable templates and standard Markdown media work inside ordinary note files.",
        steps: &[
            "Open Templates in the sidebar. Each top-level block on that page with indented blocks under it is a template: the block is its name, the indented blocks are what gets inserted. The page starts with Meeting, Project, Person and My Day, and the guide under it adds more with one click.",
            "At the beginning of an empty block, type jj, keep typing to narrow the list, and press Enter. In Vim, enter Insert mode first. Insertion undoes as one action.",
            "Placeholders: {cursor}, {today}, {date: tomorrow} (also yesterday, +3, -1, a weekday or 2026-11-02), {time}, {weekday}, {page}, {pick: #meeting} (a fuzzy picker of names used before with the tag, or a new one) and {ask: Question?}. Live lists such as {{tasks: next}} and {{query: #idea}} are copied as written; here in a task list means the page you insert into.",
            "Use Page actions to attach an image or PDF. Standard Markdown image embeds and wiki attachment embeds display inline. Keep attachment files with the notebook when copying or syncing it.",
            "Use backticks for inline code and fenced blocks with a language name for syntax highlighting. Source lets you edit all the Markdown directly.",
        ],
        example: Some(
            "- Meeting\n  - [[{ask: Meeting with?}]] {time} #meeting\n    - Agenda\n      - {cursor}\n![Diagram](assets/diagram.png)\n![[assets/report.pdf]]\n`inline code`\n```typescript\nconst next = 'one small step';\n```",
        ),
    },
    HelpSection {
        id: "data",
        title: "Files, notebooks and backups",
        description: "Keep Work and Personal in separate notebooks. Notes, attachments and the portable .quire data folder travel with each notebook.",
        steps: &[
            "Files shows your vault’s actual folders. Right-click a page or folder, or press Shift-F10, for its menu. Rename (or F2) edits the name in place and updates links; Move to… previews affected links before applying. Right-click the page for clipboard actions, inline marks, Bullet list or Numbered list, tasks and block references. Delete moves a page or folder to the system Trash, or in a Quire notebook removes it on every computer, after naming the links it breaks. Pages browses logical titles independently of physical folders.",
            "Use the notebook picker to choose Work or Personal. Tabs, favorites and reading positions are remembered separately on this computer. Settings controls fonts, line spacing, Vim and tag formatting.",
            "Appearance: Settings > Appearance chooses System, Light or Dark. The Light / Dark button in the sidebar switches between light and dark. System follows your computer as it changes. Exports are unaffected.",
            "Daily notes: by default today's page is YYYY-MM-DD.md at the notebook root. For an existing vault, set its folder and title patterns in the Daily notes browser, or press Detect from notebook. The layout is saved in the notebook for every device. Existing pages stay where they are, and [[2026-10-27]], [[2026-10-27-Tuesday]] and [[October 27th, 2026]] open the same day.",
            "A notebook that is a Git repository keeps Quire's live databases out of commits: opening it adds .quire/devices and .quire/ai to the repository's local exclude file. If they are already tracked, a notice gives the command to untrack them.",
            "For OneDrive or Dropbox, make the whole notebook available offline. Let sync finish before switching machines. Resolve conflict and recovery warnings before continuing. The app cannot guarantee protection against every hardware, cloud or filesystem failure.",
            "Roam JSON and EDN import and Roam JSON export are available from the notebook’s import/export controls. Keep the original export until you have checked the imported notebook.",
            "Quire notebooks (experimental) keep pages in a database on each computer and sync them through change files in an empty OneDrive, Dropbox or Git folder. Open it on another computer by choosing the same folder. A page edited in two places at once merges; only where both places changed the same words are both versions of the line kept, with a warning. The status bar says when every change is published.",
        ],
        example: None,
    },
];
