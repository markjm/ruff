use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use ruff_db::files::system_path_to_file;
use ruff_db::system::{SystemPath, SystemPathBuf};
use ruff_python_ast::PySourceType;
use ruff_python_parser::{ParseOptions, parse};
use ty_module_resolver::file_to_module;

use crate::collector::Collector;
pub use crate::db::ModuleDb;
use crate::resolver::Resolver;
pub use crate::settings::{AnalyzeSettings, Direction, StringImports};

mod collector;
mod db;
mod resolver;
mod settings;

#[derive(Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ModuleImports(BTreeSet<SystemPathBuf>);

impl ModuleImports {
    /// Detect the [`ModuleImports`] for a given Python file.
    pub fn detect(
        db: &ModuleDb,
        source: &str,
        source_type: PySourceType,
        path: &SystemPath,
        string_imports: StringImports,
        type_checking_imports: bool,
    ) -> Result<Self> {
        // Parse the source code.
        let parsed = parse(source, ParseOptions::from(source_type))?;

        // Use the module resolver to determine the module path for this file.
        // This leverages the database's search paths (and desperate resolution fallback)
        // rather than manually computing the path from the package root.
        let module_path = system_path_to_file(db, path).ok().and_then(|file| {
            let module = file_to_module(db, file)?;
            let name = module.name(db);
            let mut components: Vec<String> = name.components().map(String::from).collect();
            // For __init__.py files, the module name doesn't include "__init__" but the
            // Collector needs it to correctly resolve relative imports (popping one level
            // from __init__ returns you to the package level).
            if path.ends_with("__init__.py") || path.ends_with("__init__.pyi") {
                components.push("__init__".to_string());
            }
            Some(components)
        });

        let imports = Collector::new(
            module_path.as_deref(),
            string_imports,
            type_checking_imports,
        )
        .collect(parsed.syntax());

        // Resolve the imports.
        let mut resolved_imports = ModuleImports::default();
        for import in imports {
            for resolved in Resolver::new(db, path).resolve(import) {
                if let Some(path) = resolved.as_system_path() {
                    resolved_imports.insert(path.to_path_buf());
                }
            }
        }

        Ok(resolved_imports)
    }

    /// Insert a file path into the module imports.
    pub fn insert(&mut self, path: SystemPathBuf) {
        self.0.insert(path);
    }

    /// Extend the module imports with additional file paths.
    pub fn extend(&mut self, paths: impl IntoIterator<Item = SystemPathBuf>) {
        self.0.extend(paths);
    }

    /// Returns `true` if the module imports are empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the number of module imports.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Convert the file paths to be relative to a given path.
    #[must_use]
    pub fn relative_to(self, path: &SystemPath) -> Self {
        Self(
            self.0
                .into_iter()
                .map(|import| {
                    import
                        .strip_prefix(path)
                        .map(SystemPath::to_path_buf)
                        .unwrap_or(import)
                })
                .collect(),
        )
    }
}

#[derive(Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ImportMap(BTreeMap<SystemPathBuf, ModuleImports>);

impl ImportMap {
    /// Create an [`ImportMap`] of file to its dependencies.
    ///
    /// Assumes that the input is a collection of unique file paths and their imports.
    pub fn dependencies(imports: impl IntoIterator<Item = (SystemPathBuf, ModuleImports)>) -> Self {
        let mut map = ImportMap::default();
        for (path, imports) in imports {
            map.0.insert(path, imports);
        }
        map
    }

    /// Create an [`ImportMap`] of file to its dependents.
    ///
    /// Assumes that the input is a collection of unique file paths and their imports.
    pub fn dependents(imports: impl IntoIterator<Item = (SystemPathBuf, ModuleImports)>) -> Self {
        let mut reverse = ImportMap::default();
        for (path, imports) in imports {
            for import in imports.0 {
                reverse.0.entry(import).or_default().insert(path.clone());
            }
            reverse.0.entry(path).or_default();
        }
        reverse
    }
}
