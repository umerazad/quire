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
