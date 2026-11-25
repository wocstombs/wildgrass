//! Chinese text normalization: Traditional-to-Simplified conversion and romanized alias mapping.

mod simplify;
mod table;

pub use self::simplify::{simplified_name, to_simplified};
