//! Model providers. The scripted provider plays fixed replies in the xAI Responses shape, so
//! it reads them with the xAI adapter's parser; everything else xAI-shaped lives in `xai`.

mod router;
mod scripted;
mod scripts;
mod xai;

pub use router::ScenarioRouter;
pub use scripted::ScriptedProvider;
pub use scripts::{DRILL, GRANT_NOTE, LAUNDER, OWNER_NOTE, PLANTED_ERROR, S1_DECISION};
pub use xai::XaiProvider;
