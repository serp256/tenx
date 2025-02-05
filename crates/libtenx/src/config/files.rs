use std::path::PathBuf;

use crate::{config::Project, state::files::walk_files};

/// Walk project directory using ignore rules, returning all included files relative to project
/// root.
///
/// Applies project glob patterns and uses the ignore crate's functionality for respecting
/// .gitignore and other ignore files. Glob patterns can be positive (include) or negative
/// (exclude, prefixed with !).
pub fn walk_project(project: &Project) -> crate::Result<Vec<PathBuf>> {
    walk_files(project.root.clone(), project.include.clone())
}
