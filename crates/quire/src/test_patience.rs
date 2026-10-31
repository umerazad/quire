//! How long a test waits on background work before failing. Tests poll for state rather than
//! sleeping, so this only bounds a hang; it is generous because full suites share the machine
//! with other builds.

pub const PATIENCE: std::time::Duration = std::time::Duration::from_secs(60);
