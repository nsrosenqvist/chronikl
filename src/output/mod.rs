//! Output formatting.

pub mod markdown;
pub mod shape;

pub use markdown::{RenderOptions, render};
pub use shape::{Shaped, append_footer, shape};
