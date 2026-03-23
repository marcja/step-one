mod params;
mod plugin;
pub mod seq;

// Re-export so integration tests and the standalone binary can access the type.
pub use plugin::StepOne;

use nih_plug::prelude::*;

nih_export_clap!(StepOne);
