use thiserror::Error;

mod catalog;
mod state;
mod tokens;
mod traits;
pub mod util;

pub use catalog::*;
pub use state::*;
pub use tokens::*;
pub use traits::*;

#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}
