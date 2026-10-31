use quire_core::workspace::{
    BrowserView, Destination, History, MAX_RECENT, MAX_TABS, TreeKind, Workspace, folder_rows,
    in_namespace, namespace_of, namespaces,
};
use std::collections::BTreeSet;

fn page(id: &str) -> Destination {
    Destination::page(id)
}

#[test]
fn keeps_physical_folders_distinct_from_logical_namespaces() {
    let rows = folder_rows(
        &["Projects/Note.md".into(), "Imported-safe-name.md".into()],
        &["attachments".into(), "Empty".into()],
        &BTreeSet::from(["Projects".into(), "attachments".into()]),
        |a, b| a.to_lowercase().cmp(&b.to_lowercase()),
    );
    assert_eq!(
        rows.iter()
            .map(|row| (row.path.as_str(), row.kind, row.depth))
            .collect::<Vec<_>>(),
        [
            ("attachments", TreeKind::Folder, 0),
            ("Empty", TreeKind::Folder, 0),
            ("Projects", TreeKind::Folder, 0),
            ("Projects/Note.md", TreeKind::File, 1),
            ("Imported-safe-name.md", TreeKind::File, 0),
        ]
    );
    assert_eq!(namespace_of("books/Distributed systems"), "books");
    assert!(!rows.iter().any(|row| row.path == "books"));
}

#[test]
fn finds_namespaces_only_where_a_slash_separates_names() {
    for (title, namespace) in [
        ("C/Survivorship Bias", "C"),
        ("roam/js/clock", "roam/js"),
        ("Projects/Active", "Projects"),
        ("Podcasts/Cooking (shows)", "Podcasts"),
        ("Reading List/Archive", "Reading List"),
        ("Plain title", ""),
        ("20/20", ""),
        ("TCP / IP", ""),
        ("queries/", ""),
        ("/", ""),
        ("/Haskell/etc.", ""),
        ("[[roam/js/clock]]", ""),
        ("tweets: 1/ A thread about bread", ""),
        ("articles: Sourdough // A Baking Blog", ""),
        ("Read https://example.com/guide first", ""),
        ("Based on @Notes/Plan.md write a summary", ""),
        (
            "Garden Plan - Beds A and B/Watering",
            "Garden Plan - Beds A and B",
        ),
    ] {
        assert_eq!(namespace_of(title), namespace, "{title}");
    }
}

#[test]
fn namespace_counts_include_parents_and_filters_include_nested_titles() {
    let titles = ["roam/js/clock", "roam/css", "Plain", "C/Bias"];
    assert_eq!(
        namespaces(titles),
        [("C".into(), 1), ("roam".into(), 2), ("roam/js".into(), 1)]
    );
    assert!(in_namespace("roam/js/clock", "roam"));
    assert!(in_namespace("roam/css", "roam"));
    assert!(!in_namespace("roamer/x", "roam"));
    assert!(!in_namespace("Plain", "roam"));
}

#[test]
fn restores_saved_state_and_survives_invalid_preferences() {
    let mut work = Workspace {
        expanded: vec!["Projects".into()],
        tabs: vec![page("p1")],
        favorites: vec![Destination::Browser {
            view: BrowserView::Daily,
        }],
        ..Default::default()
    };
    work.side = Some("p2".into());
    work.set_oldest_first("p1", true);
    work.tabs.push(Destination::Browser {
        view: BrowserView::Tags,
    });
    assert_eq!(Workspace::load(&work.save()), work);
    assert_eq!(Workspace::load("{"), Workspace::default());
    assert_eq!(Workspace::load("{\"favorites\":[]}"), Workspace::default());
    let mixed = Workspace::load(
        r##"{"tabs":[{"kind":"page","page":"a"},{"kind":"tag","tag":"#x"},{"kind":"browser","view":"help"},4],
            "recent":[{"kind":"page"}],"expanded":["A",3],"active":{"kind":"browser","view":"daily"}}"##,
    );
    assert_eq!(
        mixed.tabs,
        [page("a"), Destination::Tag { tag: "#x".into() }],
        "tag pages are destinations; unknown browsers drop"
    );
    assert!(mixed.recent.is_empty());
    assert_eq!(mixed.expanded, ["A"]);
    assert_eq!(
        mixed.active,
        Some(Destination::Browser {
            view: BrowserView::Daily
        })
    );
}

#[test]
fn visits_lead_recents_once_and_add_bounded_tabs() {
    let mut workspace = Workspace::default();
    for index in 0..MAX_TABS + 5 {
        workspace.visit(page(&index.to_string()));
    }
    workspace.visit(page("3"));
    assert_eq!(workspace.recent.len(), MAX_RECENT);
    assert_eq!(workspace.recent[0], page("3"));
    assert_eq!(workspace.recent[1], page(&(MAX_TABS + 4).to_string()));
    assert_eq!(workspace.tabs.len(), MAX_TABS);
    assert_eq!(workspace.tabs.last(), Some(&page("3")));
    assert!(!workspace.tabs.contains(&page("0")));
    assert_eq!(workspace.active, Some(page("3")));
    workspace.remove_recent(&page("3"));
    assert_ne!(workspace.recent[0], page("3"));
}

#[test]
fn closing_the_active_tab_falls_back_to_preferred_last_or_pages() {
    let mut workspace = Workspace::default();
    for id in ["a", "b", "c"] {
        workspace.visit(page(id));
    }
    assert_eq!(workspace.close(|tab| tab == &page("b"), None), None);
    assert_eq!(
        workspace.close(|tab| tab == &page("c"), Some(&page("a"))),
        Some(page("a"))
    );
    workspace.visit(page("d"));
    assert_eq!(
        workspace.close(|tab| tab == &page("d"), None),
        Some(page("a"))
    );
    assert_eq!(
        workspace.close(|_| true, None),
        Some(Destination::Browser {
            view: BrowserView::Pages
        })
    );
}

#[test]
fn favorites_toggle_in_insertion_order_and_missing_pages_drop() {
    let mut workspace = Workspace::default();
    workspace.toggle_favorite(page("b"));
    workspace.toggle_favorite(page("a"));
    workspace.toggle_favorite(page("c"));
    workspace.toggle_favorite(page("a"));
    assert_eq!(workspace.favorites, [page("b"), page("c")]);
    workspace.visit(page("c"));
    workspace.side = Some("c".into());
    workspace.set_oldest_first("c", true);
    workspace.set_oldest_first("b", true);
    workspace.set_oldest_first("b", false);
    workspace.set_oldest_first("a", true);
    assert!(workspace.is_oldest_first("c") && !workspace.is_oldest_first("b"));
    workspace.retain_pages(|id| id != "c");
    assert_eq!(workspace.favorites, [page("b")]);
    assert_eq!(workspace.oldest_first, ["a"]);
    assert!(workspace.tabs.is_empty() && workspace.recent.is_empty());
    assert_eq!((workspace.active, workspace.side), (None, None));
}

#[test]
fn history_skips_repeats_and_cuts_forward_entries() {
    let mut history = History::default();
    assert!(!history.can_go_back());
    history.push(page("a"));
    history.push(page("a"));
    history.push(page("b"));
    history.push(page("c"));
    assert_eq!(history.back(), Some(page("b")));
    assert_eq!(history.back(), Some(page("a")));
    assert_eq!(history.back(), None);
    assert_eq!(history.forward(), Some(page("b")));
    history.push(page("d"));
    assert_eq!(history.forward(), None);
    assert_eq!(history.back(), Some(page("b")));
    assert_eq!(history.back(), Some(page("a")));
    history.retain(|item| item != &page("a"));
    assert_eq!(history.current(), Some(&page("b")));
    assert_eq!(history.forward(), Some(page("d")));
}

#[test]
fn task_choices_survive_save_and_load_and_forget_deleted_projects() {
    use quire_core::workspace::{ProjectStatus, TaskChoices, TaskView};
    let mut workspace = Workspace::default();
    workspace.visit(Destination::Browser {
        view: BrowserView::Tasks,
    });
    workspace.tasks = TaskChoices {
        view: TaskView::Projects,
        project: Some("atlas".into()),
        project_status: ProjectStatus::Inactive,
        query: "call".into(),
        limit: 80,
        draft: "Email Ana".into(),
    };
    let loaded = Workspace::load(&workspace.save());
    assert_eq!(loaded, workspace);
    let older = Workspace::load(r#"{"tabs":[],"tasks":{"view":"later","project_status":"all"}}"#);
    assert_eq!(
        older.tasks,
        TaskChoices::default(),
        "unreadable choices fall back to Inbox and Active"
    );
    let mut pruned = loaded;
    pruned.retain_pages(|id| id != "atlas");
    assert_eq!(pruned.tasks.project, None);
    assert_eq!(pruned.tasks.project_status, ProjectStatus::Inactive);
}

#[test]
fn deleting_a_folder_forgets_it_and_the_folders_inside_it() {
    let mut workspace = Workspace::default();
    for folder in ["Projects", "Projects/Deep", "Projects2", "Other/Projects"] {
        workspace.toggle_expanded(folder);
    }
    workspace.forget_folder("Projects");
    assert_eq!(workspace.expanded, ["Projects2", "Other/Projects"]);
}

#[test]
fn a_page_whose_identity_changed_keeps_its_tabs_favorites_and_history() {
    let mut workspace = Workspace::default();
    workspace.visit(page("new"));
    workspace.visit(page("old"));
    workspace.visit(page("keep"));
    workspace.visit(page("old"));
    workspace.toggle_favorite(page("old"));
    workspace.side = Some("old".into());
    workspace.set_oldest_first("old", true);
    workspace.rename_page("old", "new");
    assert_eq!(workspace.tabs, [page("new"), page("keep")]);
    assert_eq!(workspace.recent, [page("new"), page("keep")]);
    assert_eq!(workspace.favorites, [page("new")]);
    assert_eq!(workspace.active, Some(page("new")));
    assert_eq!(workspace.side.as_deref(), Some("new"));
    assert!(workspace.is_oldest_first("new"));

    let mut history = History::default();
    for id in ["old", "keep", "old"] {
        history.push(page(id));
    }
    history.rename_page("old", "new");
    assert_eq!(history.current(), Some(&page("new")));
    assert_eq!(history.back(), Some(page("keep")));
    assert_eq!(history.back(), Some(page("new")));
}

#[test]
fn view_choices_and_reading_positions_survive_save_load_rename_and_delete() {
    use quire_core::workspace::{
        MAX_LIMIT, PageChoices, TagChoices, TagListChoices, TagSort, TaskStatus, position_key,
    };
    let mut workspace = Workspace::default();
    workspace.visit(page("a"));
    workspace.pages = PageChoices {
        query: "plan".into(),
        namespace: Some("Projects".into()),
    };
    workspace.tag_list = TagListChoices {
        query: "wo".into(),
        sort: TagSort::Most,
    };
    workspace.tag_pages.insert(
        "#work".into(),
        TagChoices {
            query: "deck".into(),
            status: TaskStatus::Due,
            scroll: 12,
        },
    );
    workspace.positions.insert(position_key(false, "a"), 40);
    workspace.positions.insert(position_key(true, "a"), 7);
    workspace.positions.insert(position_key(false, "b"), 3);
    assert_eq!(Workspace::load(&workspace.save()), workspace);

    let damaged = Workspace::load(
        r##"{"tabs":[],"positions":{"page:a":5,"page:b":-1,"page:c":"x"},
            "tag_pages":{"#a":{"query":"x","scroll":2},"#b":{"status":"later"}},
            "tag_list":{"sort":"loudest"},"tasks":{"limit":99999999}}"##,
    );
    assert_eq!(
        damaged.positions.into_iter().collect::<Vec<_>>(),
        [("page:a".to_owned(), 5)],
        "negative and unreadable positions drop"
    );
    assert_eq!(damaged.tag_pages.keys().collect::<Vec<_>>(), ["#a"]);
    assert_eq!(damaged.tag_list, TagListChoices::default());
    assert_eq!(damaged.tasks.limit, MAX_LIMIT);

    workspace.rename_page("a", "a2");
    assert_eq!(
        workspace.positions.get(&position_key(false, "a2")),
        Some(&40)
    );
    assert_eq!(workspace.positions.get(&position_key(true, "a2")), Some(&7));
    workspace.retain_pages(|id| id != "b");
    assert_eq!(
        workspace.positions.keys().collect::<Vec<_>>(),
        ["page:a2", "side:a2"]
    );
}

#[test]
fn new_folders_open_their_parents_survive_reload_and_go_with_their_parent() {
    let mut workspace = Workspace::default();
    workspace.add_folder("Projects/Plans/Q4");
    workspace.add_folder("Archive");
    workspace.add_folder("Archive");
    assert_eq!(workspace.folders, ["Archive", "Projects/Plans/Q4"]);
    assert_eq!(workspace.expanded, ["Projects", "Projects/Plans"]);
    assert_eq!(Workspace::load(&workspace.save()), workspace);
    workspace.forget_folder("Projects");
    assert_eq!(workspace.folders, ["Archive"]);
    assert!(workspace.expanded.is_empty());
}

#[test]
fn namespaces_match_the_reference_namespace_of_vectors() {
    let vectors: Vec<(String, String)> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/namespace-vectors.json"
    ))
    .unwrap();
    assert!(vectors.len() > 1000);
    let wrong: Vec<_> = vectors
        .iter()
        .filter(|(title, namespace)| namespace_of(title) != namespace)
        .map(|(title, namespace)| (title, namespace, namespace_of(title)))
        .collect();
    assert!(wrong.is_empty(), "title, reference, quire: {wrong:?}");
}
