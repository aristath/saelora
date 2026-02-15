mod client;
mod error;
pub mod types;

pub use client::{Client, Config, DEFAULT_BASE_URL};
pub use error::HttpError;
pub use types::{ChatChoice, ChatCompletionRequest, Message, Model};
