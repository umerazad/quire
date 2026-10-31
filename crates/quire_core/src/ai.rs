//! The pure half of AI commands, ported from the reference `ai-commands.ts`, `ai-changes.ts`, and
//! `ai-runner.ts`: argument parsing, prompts, block marker repair, questions and their citations,
//! Tidy's limits, the CLI argument lists, and the parsers for each CLI's JSON event stream. The
//! app's process host, proposal view, and answer pane share it.

pub mod ask;
pub mod backend;
pub mod commands;
pub mod export;
pub mod mcp;
pub mod tidy;
pub mod tools;

pub use backend::{Backend, Event, ParseState, Parsed, parse_line};
pub use commands::{AiRange, Command, Placement, Verb};
