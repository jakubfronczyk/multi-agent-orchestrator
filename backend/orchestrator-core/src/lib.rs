//! Domain logic of the orchestrator: types, the approval rules and the engine loop.
//! It declares the storage, model and tool traits and never does I/O itself.

pub mod agents;
pub mod ids;
pub mod llm;
pub mod money;
pub mod policy;
pub mod runs;
pub mod store;
pub mod tools;
