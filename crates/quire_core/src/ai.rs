//! The pure half of AI transforms, ported from the reference `ai-commands.ts` and
//! `ai-runner.ts`: argument parsing, prompts, block marker repair, the CLI argument lists, and
//! the parsers for each CLI's JSON event stream. The app's process host and proposal view share
//! it.

pub mod backend;
pub mod commands;

pub use backend::{Backend, Event, ParseState, Parsed, parse_line};
pub use commands::{AiRange, Command, Placement, Verb};
