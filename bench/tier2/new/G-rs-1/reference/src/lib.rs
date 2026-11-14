//! verso: semantic versions, version constraints, a package registry and a solver.

pub mod constraint;
pub mod error;
pub mod registry;
pub mod solver;
pub mod version;

pub use constraint::Constraint;
pub use error::Error;
pub use registry::Registry;
pub use solver::resolve;
pub use version::Version;
