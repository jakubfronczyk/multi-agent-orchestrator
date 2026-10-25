//! Domain logic of the orchestrator: types, the approval rules and the engine loop.
//! It reaches storage, models and tools only through the traits it declares, so it holds no
//! network, disk or database code; the engine does own its tasks, timers and locks.

pub mod agents;
pub mod engine;
pub mod ids;
pub mod llm;
pub mod money;
pub mod policy;
pub mod runs;
pub mod store;
pub mod tools;
