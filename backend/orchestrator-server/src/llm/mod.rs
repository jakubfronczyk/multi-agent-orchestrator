//! Model providers. The scripted provider plays fixed replies in the xAI Responses shape, so
//! it reads them with the xAI adapter's parser; everything else xAI-shaped lives in `xai`.

mod scripted;
mod scripts;
mod xai;

pub use scripted::ScriptedProvider;
pub use xai::XaiProvider;
