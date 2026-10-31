use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings_macros::{MergeFrom, with_fallible_options};

/// Settings that belong to the Quire notes application.
#[with_fallible_options]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct QuireSettingsContent {
    /// Whether #tags render as soft labels in Write and rendered references.
    ///
    /// Default: true
    pub format_tags: Option<bool>,
    /// Where quick capture adds tasks.
    ///
    /// Default: inbox
    pub capture_target: Option<CaptureTarget>,
    /// AI transforms through a command line tool.
    pub ai: Option<QuireAiSettingsContent>,
}

/// How Quire runs AI transforms: through the Claude Code or Codex command line tool already
/// signed in on this computer.
#[with_fallible_options]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct QuireAiSettingsContent {
    /// Whether AI commands run. They stay off until a backend test succeeds.
    ///
    /// Default: false
    pub enabled: Option<bool>,
    /// The command line tool that runs AI commands.
    ///
    /// Default: claude
    pub backend: Option<AiBackendName>,
    /// The Claude Code binary, or empty to find it on the login shell's PATH.
    ///
    /// Default: ""
    pub claude_path: Option<String>,
    /// The Codex binary, or empty to find it on the login shell's PATH.
    ///
    /// Default: ""
    pub codex_path: Option<String>,
    /// The model name passed to the tool, or empty for its default.
    ///
    /// Default: ""
    pub model: Option<String>,
    /// What the notebook write tools may do: off, propose changes for review, or apply them.
    ///
    /// Default: review
    pub writes: Option<AiWritesName>,
}

/// What AI write tools may do to the notebook.
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum AiWritesName {
    /// Write tools are not offered.
    Off,
    /// Changes wait for review in Quire.
    #[default]
    Review,
    /// Changes apply at once.
    Apply,
}

/// The command line tools Quire can run AI commands through.
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum AiBackendName {
    /// Claude Code.
    #[default]
    Claude,
    /// Codex.
    Codex,
}

/// The page that quick capture appends tasks to.
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum CaptureTarget {
    /// The Inbox page.
    #[default]
    Inbox,
    /// Today's daily page.
    Today,
}
