pub const APPLICATION_ID: i32 = i32::from_be_bytes(*b"QURE");
pub const SCHEMA_VERSION: i32 = 2;

// Keep version 1 immutable. Later schemas need an explicit migration and its own checksum.
pub(crate) const SCHEMA_V1: &[(&str, &str)] = &[
    (
        "schema_history",
        "CREATE TABLE schema_history (version INTEGER PRIMARY KEY, checksum TEXT NOT NULL)",
    ),
    (
        "meta",
        "CREATE TABLE meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), notebook TEXT NOT NULL, device TEXT NOT NULL, uncertain INTEGER NOT NULL CHECK(uncertain IN (0,1)))",
    ),
    (
        "revisions",
        "CREATE TABLE revisions (id TEXT PRIMARY KEY, page TEXT NOT NULL, path TEXT NOT NULL, path_key TEXT, generation INTEGER NOT NULL CHECK(generation>=0), created INTEGER NOT NULL CHECK(created>=0), deleted INTEGER NOT NULL CHECK(deleted IN (0,1)), body_hash TEXT NOT NULL, body BLOB NOT NULL)",
    ),
    (
        "revisions_by_page",
        "CREATE INDEX revisions_by_page ON revisions(page, generation DESC, id DESC)",
    ),
    (
        "parents",
        "CREATE TABLE parents (child TEXT NOT NULL REFERENCES revisions(id) ON DELETE CASCADE, parent TEXT NOT NULL REFERENCES revisions(id) ON DELETE RESTRICT, PRIMARY KEY(child,parent))",
    ),
    (
        "heads",
        "CREATE TABLE heads (page TEXT NOT NULL, revision TEXT NOT NULL REFERENCES revisions(id) ON DELETE RESTRICT, PRIMARY KEY(page,revision))",
    ),
    (
        "publication",
        "CREATE TABLE publication (singleton INTEGER PRIMARY KEY CHECK(singleton=1), seq INTEGER NOT NULL, hash TEXT NOT NULL, previous TEXT, name TEXT NOT NULL, data BLOB NOT NULL, revisions TEXT NOT NULL)",
    ),
    (
        "outbox",
        "CREATE TABLE outbox (position INTEGER PRIMARY KEY AUTOINCREMENT, revision TEXT NOT NULL UNIQUE REFERENCES revisions(id) ON DELETE RESTRICT, batch INTEGER REFERENCES publication(singleton) ON DELETE RESTRICT CHECK(batch IS NULL OR batch=1))",
    ),
    (
        "cursors",
        "CREATE TABLE cursors (device TEXT PRIMARY KEY, seq INTEGER NOT NULL, hash TEXT NOT NULL)",
    ),
    (
        "drafts",
        "CREATE TABLE drafts (page TEXT PRIMARY KEY, session TEXT NOT NULL, version INTEGER NOT NULL CHECK(version>=0), body_hash TEXT NOT NULL, body BLOB NOT NULL)",
    ),
    (
        "editor_saves",
        "CREATE TABLE editor_saves (page TEXT NOT NULL, session TEXT NOT NULL, version INTEGER NOT NULL CHECK(version>=0), input_hash TEXT NOT NULL, revision TEXT REFERENCES revisions(id) ON DELETE SET NULL, base TEXT NOT NULL, PRIMARY KEY(page,session))",
    ),
    (
        "attachments",
        "CREATE TABLE attachments (name TEXT PRIMARY KEY, hash TEXT NOT NULL, size INTEGER NOT NULL CHECK(size>=0))",
    ),
];

pub(crate) const SCHEMA_V2: &[(&str, &str)] = &[
    (
        "batch_hashes",
        "CREATE TABLE batch_hashes (device TEXT NOT NULL, seq INTEGER NOT NULL CHECK(seq BETWEEN 1 AND 999999999), hash TEXT NOT NULL CHECK(length(hash)=64 AND hash NOT GLOB '*[^0-9a-f]*'), PRIMARY KEY(device,seq))",
    ),
    (
        "attachment_needs",
        "CREATE TABLE attachment_needs (name TEXT PRIMARY KEY NOT NULL)",
    ),
];

pub(crate) fn migration(version: i32) -> &'static [(&'static str, &'static str)] {
    match version {
        1 => SCHEMA_V1,
        2 => SCHEMA_V2,
        _ => unreachable!("preflight must validate schema versions"),
    }
}

pub(crate) fn checksum(version: i32) -> String {
    super::sync::format::sha256(
        migration(version)
            .iter()
            .map(|(_, sql)| *sql)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}
