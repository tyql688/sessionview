use thiserror::Error;

mod catalog;
mod plan;
mod state;
mod tokens;
mod traits;
mod trash;

pub use catalog::*;
pub use plan::*;
pub use state::*;
pub use tokens::*;
pub use traits::*;
pub use trash::*;

#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}
