//! Types partages entre tous les composants d'Auris.

pub mod error;
pub mod model;
pub mod settings;
pub mod zip;

pub use error::{AurisError, Result};
pub use model::*;
pub use settings::*;
pub use zip::ZipWriter;
