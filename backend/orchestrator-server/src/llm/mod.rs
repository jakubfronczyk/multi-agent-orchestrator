//! Model providers. Both providers produce raw items in the xAI Responses shape, and
//! `parse_output` is the one place that reads that shape.

mod parse;
mod scripted;
mod scripts;

pub use parse::parse_output;
pub use scripted::ScriptedProvider;
