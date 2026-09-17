//! Skillshard — a desktop manager for AI coding agent skills.
//!
//! Installing, updating and removing skills is delegated to the `skills` CLI
//! (<https://skills.sh>). Skillshard owns the parts that CLI has no command
//! for: seeing every skill across every agent at once, enabling and disabling
//! them, keeping agents in sync, and moving a skill between global and project
//! scope.

pub mod agents;
pub mod frontmatter;
pub mod lock;
pub mod model;
pub mod ops;
pub mod paths;
pub mod registry;
pub mod scan;
pub mod skills_cli;
pub mod state;
pub mod ui;
pub mod updates;
