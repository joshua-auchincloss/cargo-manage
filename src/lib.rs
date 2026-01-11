use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use thiserror::Error;
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value};
use tracing::{debug, info, warn};
use walkdir::WalkDir;

#[derive(Error, Debug)]
pub enum HoistError {
    #[error("No Cargo.toml found at {0}")]
    NoCargoToml(PathBuf),

    #[error("Failed to read file {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Failed to write file {path}: {source}")]
    WriteFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("Failed to parse TOML in {path}: {source}")]
    ParseToml {
        path: PathBuf,
        source: toml_edit::TomlError,
    },

    #[error("Failed to copy file from {from} to {to}: {source}")]
    CopyFile {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },

    #[error("Invalid root directory: {0}")]
    InvalidRoot(PathBuf),
}

pub type Result<T> = std::result::Result<T, HoistError>;

/// Configuration for hoist operations
#[derive(Debug, Clone, Default)]
pub struct HoistConfig {
    /// Directories to ignore when scanning
    pub ignores: HashSet<String>,
    /// Dependency sections to process
    pub sections: Vec<String>,
    /// Optional prefix for priority sorting (e.g., "kintsu" sorts kintsu-* deps first)
    pub sort_prefix: Option<String>,
}

impl HoistConfig {
    pub fn new() -> Self {
        Self {
            ignores: [
                "target",
                ".git",
                "node_modules",
                "vendor",
                ".config",
                ".cargo",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            sections: vec![
                "dependencies".into(),
                "dev-dependencies".into(),
                "build-dependencies".into(),
            ],
            sort_prefix: None,
        }
    }

    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.sort_prefix = Some(prefix.into());
        self
    }

    pub fn with_ignores(mut self, ignores: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.ignores = ignores.into_iter().map(Into::into).collect();
        self
    }
}

/// Statistics from a hoist operation
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HoistStats {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
}

/// Statistics from a sort operation
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SortStats {
    pub files_sorted: usize,
}

/// Statistics from a restore operation
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestoreStats {
    pub files_restored: usize,
}

/// Find all Cargo.toml files under root, excluding ignored directories
pub fn find_cargo_tomls(root: &Path, ignores: &HashSet<String>) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !ignores.contains(name.as_ref())
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "Cargo.toml")
        .map(|e| e.into_path())
        .collect();

    files.sort();
    files
}

/// Create a backup of a file, returning the backup path
pub fn backup_file(path: &Path) -> Result<PathBuf> {
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();

    let bak = path.with_file_name(format!("{}.bak", file_name));

    let target = if bak.exists() {
        let ts = Utc::now().format("%Y%m%dT%H%M%SZ");
        path.with_file_name(format!("{}.{}.bak", file_name, ts))
    } else {
        bak
    };

    fs::copy(path, &target).map_err(|e| HoistError::CopyFile {
        from: path.to_path_buf(),
        to: target.clone(),
        source: e,
    })?;

    debug!("Created backup: {:?}", target);
    Ok(target)
}

/// Read and parse a TOML file
pub fn read_toml(path: &Path) -> Result<DocumentMut> {
    let content = fs::read_to_string(path).map_err(|e| HoistError::ReadFile {
        path: path.to_path_buf(),
        source: e,
    })?;

    parse_toml(&content, path)
}

/// Parse TOML content (useful for testing without filesystem)
pub fn parse_toml(content: &str, path: &Path) -> Result<DocumentMut> {
    content
        .parse::<DocumentMut>()
        .map_err(|e| HoistError::ParseToml {
            path: path.to_path_buf(),
            source: e,
        })
}

/// Write a TOML document to a file
pub fn write_toml(path: &Path, doc: &DocumentMut) -> Result<()> {
    fs::write(path, doc.to_string()).map_err(|e| HoistError::WriteFile {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Extract version from a dependency spec.
/// Returns None if the dependency uses a path (local dependency).
pub fn extract_version(item: &Item) -> Option<String> {
    match item {
        Item::Value(Value::String(s)) => Some(s.value().clone()),
        Item::Value(Value::InlineTable(t)) => {
            if t.get("path").is_some() {
                None
            } else {
                t.get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            }
        }
        Item::Table(t) => {
            if t.get("path").is_some() {
                None
            } else {
                t.get("version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            }
        }
        _ => None,
    }
}

/// Check if a dependency already has workspace = true
pub fn is_workspace_dep(item: &Item) -> bool {
    match item {
        Item::Value(Value::InlineTable(t)) => t
            .get("workspace")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        Item::Table(t) => t
            .get("workspace")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        _ => false,
    }
}

/// Check if dependency has a path (local dependency)
pub fn has_path(item: &Item) -> bool {
    match item {
        Item::Value(Value::InlineTable(t)) => t.get("path").is_some(),
        Item::Table(t) => t.get("path").is_some(),
        _ => false,
    }
}

/// Mutate a dependency to use workspace = true
pub fn mutate_to_workspace(item: &mut Item) {
    match item {
        Item::Value(Value::String(_)) => {
            let mut inline = InlineTable::new();
            inline.insert("workspace", Value::from(true));
            *item = Item::Value(Value::InlineTable(inline));
        }
        Item::Value(Value::InlineTable(t)) => {
            t.remove("version");
            t.insert("workspace", Value::from(true));
        }
        Item::Table(t) => {
            t.remove("version");
            t.insert("workspace", Item::Value(Value::from(true)));
        }
        _ => {}
    }
}

/// Ensure workspace.dependencies exists in the document
pub fn ensure_workspace_deps(doc: &mut DocumentMut) -> &mut Table {
    if !doc.contains_key("workspace") {
        doc["workspace"] = Item::Table(Table::new());
    }

    let ws = doc["workspace"].as_table_mut().unwrap();

    if !ws.contains_key("dependencies") {
        ws.insert("dependencies", Item::Table(Table::new()));
    }

    ws["dependencies"].as_table_mut().unwrap()
}

/// Sort a dependency table: prefixed deps first (sorted), then others (sorted)
pub fn sort_table(table: &mut Table, prefix: Option<&str>) -> bool {
    let items: Vec<(String, Item)> = table
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();

    if items.is_empty() {
        return false;
    }

    let (mut prefixed, mut others): (Vec<_>, Vec<_>) = match prefix {
        Some(p) => items.into_iter().partition(|(k, _)| k.starts_with(p)),
        None => (vec![], items),
    };

    prefixed.sort_by(|a, b| a.0.cmp(&b.0));
    others.sort_by(|a, b| a.0.cmp(&b.0));

    table.clear();

    for (key, value) in prefixed.into_iter().chain(others.into_iter()) {
        table.insert(&key, value);
    }

    true
}

/// Sort a single TOML document's dependencies
pub fn sort_document(doc: &mut DocumentMut, sections: &[String], prefix: Option<&str>) -> bool {
    let mut modified = false;

    // Sort dependency sections
    for section in sections {
        if let Some(tbl) = doc.get_mut(section).and_then(|i| i.as_table_mut()) {
            if sort_table(tbl, prefix) {
                modified = true;
            }
        }
    }

    // Sort workspace.dependencies
    if let Some(ws) = doc.get_mut("workspace").and_then(|i| i.as_table_mut()) {
        if let Some(deps) = ws.get_mut("dependencies").and_then(|i| i.as_table_mut()) {
            if sort_table(deps, prefix) {
                modified = true;
            }
        }

        // Sort workspace.members
        if let Some(members) = ws.get_mut("members") {
            if let Some(arr) = members.as_array_mut() {
                let mut items: Vec<String> = arr
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();
                items.sort();

                arr.clear();
                for item in items {
                    arr.push(item);
                }
                modified = true;
            }
        }
    }

    modified
}

/// Dependency occurrence tracking
#[derive(Debug, Clone)]
struct DepOccurrence {
    section: String,
    path: PathBuf,
    version: Option<String>,
    is_path_dep: bool,
}

/// Hoist dependencies to workspace level
///
/// This function:
/// 1. Scans all Cargo.toml files in the workspace
/// 2. Collects all dependencies from member crates
/// 3. Adds unique dependencies to workspace.dependencies
/// 4. Updates member crates to use `workspace = true`
pub fn hoist(root: &Path, config: &HoistConfig, dry_run: bool) -> Result<HoistStats> {
    let root = root
        .canonicalize()
        .map_err(|_| HoistError::InvalidRoot(root.to_path_buf()))?;
    let root_toml = root.join("Cargo.toml");

    if !root_toml.exists() {
        return Err(HoistError::NoCargoToml(root_toml));
    }

    let mut files = find_cargo_tomls(&root, &config.ignores);

    // Ensure root toml is first
    if let Some(pos) = files.iter().position(|p| *p == root_toml) {
        files.remove(pos);
    }
    files.insert(0, root_toml.clone());

    info!("Found {} Cargo.toml files", files.len());

    // Parse all files
    let mut docs: HashMap<PathBuf, DocumentMut> = HashMap::new();
    for file in &files {
        let doc = read_toml(file)?;
        docs.insert(file.clone(), doc);
    }

    // Collect dependencies from member packages
    let mut needed: HashMap<String, Vec<DepOccurrence>> = HashMap::new();

    for file in files.iter().skip(1) {
        let doc = docs.get(file).unwrap();

        for section in &config.sections {
            if let Some(tbl) = doc.get(section).and_then(|i| i.as_table()) {
                for (dep_name, item) in tbl.iter() {
                    if is_workspace_dep(item) {
                        continue;
                    }

                    let occ = DepOccurrence {
                        section: section.clone(),
                        path: file.clone(),
                        version: extract_version(item),
                        is_path_dep: has_path(item),
                    };

                    needed.entry(dep_name.to_string()).or_default().push(occ);
                }
            }
        }
    }

    let root_doc = docs.get_mut(&root_toml).unwrap();
    let ws_deps = ensure_workspace_deps(root_doc);

    let mut stats = HoistStats::default();
    let dep_names: Vec<String> = needed.keys().cloned().collect();

    for dep_name in &dep_names {
        let occurrences = needed.get(dep_name).unwrap();

        // Check if any occurrence is a path dependency (skip these)
        if occurrences.iter().any(|o| o.is_path_dep) {
            debug!("Skipping {} (path dependency)", dep_name);
            stats.skipped += 1;
            continue;
        }

        // Find first available version
        let version = occurrences.iter().find_map(|o| o.version.clone());

        let Some(version) = version else {
            debug!("Skipping {} (no version found)", dep_name);
            stats.skipped += 1;
            continue;
        };

        // Add to workspace.dependencies if not present
        if !ws_deps.contains_key(dep_name) {
            ws_deps.insert(dep_name, Item::Value(Value::from(version.as_str())));
            info!(
                "Added {} = \"{}\" to workspace.dependencies",
                dep_name, version
            );
            stats.added += 1;
        }
    }

    // Collect updates to apply
    let updates: Vec<(PathBuf, Vec<(String, String)>)> = needed
        .iter()
        .flat_map(|(dep_name, occs)| {
            occs.iter()
                .filter(|o| !o.is_path_dep && o.version.is_some())
                .map(|o| (o.path.clone(), dep_name.clone(), o.section.clone()))
        })
        .fold(
            HashMap::<PathBuf, Vec<(String, String)>>::new(),
            |mut acc, (path, dep, sec)| {
                acc.entry(path).or_default().push((dep, sec));
                acc
            },
        )
        .into_iter()
        .collect();

    // Mutate member dependencies
    for (path, deps_to_update) in &updates {
        if *path == root_toml {
            continue;
        }

        let doc = docs.get_mut(path).unwrap();

        for (dep_name, section) in deps_to_update {
            if let Some(sec_table) = doc.get_mut(section).and_then(|i| i.as_table_mut()) {
                if let Some(item) = sec_table.get_mut(dep_name) {
                    if !is_workspace_dep(item) && !has_path(item) {
                        mutate_to_workspace(item);
                        stats.updated += 1;
                        debug!("Updated {}.{} in {:?}", section, dep_name, path);
                    }
                }
            }
        }
    }

    if dry_run {
        info!(
            "[dry-run] added={} updated={} skipped={}",
            stats.added, stats.updated, stats.skipped
        );
        return Ok(stats);
    }

    // Write files with backups
    let mut modified = Vec::new();

    if stats.added > 0 {
        let bak = backup_file(&root_toml)?;
        modified.push((root_toml.clone(), bak));
        write_toml(&root_toml, docs.get(&root_toml).unwrap())?;
    }

    let mut seen = HashSet::new();
    for (path, _) in &updates {
        if seen.contains(path) || *path == root_toml {
            continue;
        }
        seen.insert(path.clone());

        let bak = backup_file(path)?;
        modified.push((path.clone(), bak));
        write_toml(path, docs.get(path).unwrap())?;
    }

    info!(
        "Done: added={} updated={} skipped={}",
        stats.added, stats.updated, stats.skipped
    );

    if !modified.is_empty() {
        info!("Backups created:");
        for (orig, bak) in &modified {
            let orig_rel = orig.strip_prefix(&root).unwrap_or(orig);
            let bak_name = bak.file_name().unwrap_or_default().to_string_lossy();
            info!("  {:?} -> {}", orig_rel, bak_name);
        }
    }

    Ok(stats)
}

/// Sort dependencies in all Cargo.toml files
pub fn sort(root: &Path, config: &HoistConfig, dry_run: bool) -> Result<SortStats> {
    let root = root
        .canonicalize()
        .map_err(|_| HoistError::InvalidRoot(root.to_path_buf()))?;
    let files = find_cargo_tomls(&root, &config.ignores);

    info!("Found {} Cargo.toml files to sort", files.len());

    let mut stats = SortStats::default();

    for file in &files {
        backup_file(file)?;

        let mut doc = read_toml(file)?;
        let modified = sort_document(&mut doc, &config.sections, config.sort_prefix.as_deref());

        if modified {
            stats.files_sorted += 1;

            if !dry_run {
                write_toml(file, &doc)?;
                info!("Sorted {:?}", file.strip_prefix(&root).unwrap_or(file));
            }
        }
    }

    if dry_run {
        info!("[dry-run] would sort {} files", stats.files_sorted);
    } else {
        info!("Sorted {} files", stats.files_sorted);
    }

    Ok(stats)
}

/// Restore Cargo.toml files from backups
pub fn restore(root: &Path, config: &HoistConfig, dry_run: bool) -> Result<RestoreStats> {
    let root = root
        .canonicalize()
        .map_err(|_| HoistError::InvalidRoot(root.to_path_buf()))?;

    let backups: Vec<PathBuf> = WalkDir::new(&root)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !config.ignores.contains(name.as_ref())
        })
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy();
            name.starts_with("Cargo.toml.") && name.ends_with(".bak")
        })
        .map(|e| e.into_path())
        .collect();

    if backups.is_empty() {
        warn!("No Cargo.toml.*.bak files found");
        return Ok(RestoreStats::default());
    }

    let mut stats = RestoreStats::default();

    for bak in &backups {
        let orig = bak.with_file_name("Cargo.toml");

        if !orig.exists() {
            warn!(
                "Skipping {:?} (no original file)",
                bak.strip_prefix(&root).unwrap_or(bak)
            );
            continue;
        }

        if dry_run {
            info!(
                "[dry-run] would restore {:?}",
                orig.strip_prefix(&root).unwrap_or(&orig)
            );
        } else {
            fs::copy(bak, &orig).map_err(|e| HoistError::CopyFile {
                from: bak.clone(),
                to: orig.clone(),
                source: e,
            })?;
            info!("Restored {:?}", orig.strip_prefix(&root).unwrap_or(&orig));
        }

        stats.files_restored += 1;
    }

    if dry_run {
        info!("[dry-run] would restore {} files", stats.files_restored);
    } else {
        info!("Restored {} files", stats.files_restored);
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Helper to create a test workspace
    fn create_test_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        dir
    }

    fn write_file(dir: &Path, relative_path: &str, content: &str) {
        let path = dir.join(relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    mod toml_helpers {
        use super::*;

        #[test]
        fn test_extract_version_string() {
            let doc: DocumentMut = r#"
                [dependencies]
                serde = "1.0"
            "#
            .parse()
            .unwrap();

            let item = doc["dependencies"]["serde"].clone();
            assert_eq!(extract_version(&item), Some("1.0".to_string()));
        }

        #[test]
        fn test_extract_version_inline_table() {
            let doc: DocumentMut = r#"
                [dependencies]
                serde = { version = "1.0", features = ["derive"] }
            "#
            .parse()
            .unwrap();

            let item = doc["dependencies"]["serde"].clone();
            assert_eq!(extract_version(&item), Some("1.0".to_string()));
        }

        #[test]
        fn test_extract_version_table() {
            let doc: DocumentMut = r#"
                [dependencies.serde]
                version = "1.0"
                features = ["derive"]
            "#
            .parse()
            .unwrap();

            let item = doc["dependencies"]["serde"].clone();
            assert_eq!(extract_version(&item), Some("1.0".to_string()));
        }

        #[test]
        fn test_extract_version_path_dep_returns_none() {
            let doc: DocumentMut = r#"
                [dependencies]
                my-crate = { path = "../my-crate" }
            "#
            .parse()
            .unwrap();

            let item = doc["dependencies"]["my-crate"].clone();
            assert_eq!(extract_version(&item), None);
        }

        #[test]
        fn test_is_workspace_dep_true() {
            let doc: DocumentMut = r#"
                [dependencies]
                serde = { workspace = true }
            "#
            .parse()
            .unwrap();

            let item = doc["dependencies"]["serde"].clone();
            assert!(is_workspace_dep(&item));
        }

        #[test]
        fn test_is_workspace_dep_false() {
            let doc: DocumentMut = r#"
                [dependencies]
                serde = "1.0"
            "#
            .parse()
            .unwrap();

            let item = doc["dependencies"]["serde"].clone();
            assert!(!is_workspace_dep(&item));
        }

        #[test]
        fn test_has_path_true() {
            let doc: DocumentMut = r#"
                [dependencies]
                my-crate = { path = "../my-crate", version = "0.1" }
            "#
            .parse()
            .unwrap();

            let item = doc["dependencies"]["my-crate"].clone();
            assert!(has_path(&item));
        }

        #[test]
        fn test_has_path_false() {
            let doc: DocumentMut = r#"
                [dependencies]
                serde = { version = "1.0" }
            "#
            .parse()
            .unwrap();

            let item = doc["dependencies"]["serde"].clone();
            assert!(!has_path(&item));
        }

        #[test]
        fn test_mutate_to_workspace_string() {
            let mut doc: DocumentMut = r#"
                [dependencies]
                serde = "1.0"
            "#
            .parse()
            .unwrap();

            let item = doc["dependencies"]
                .as_table_mut()
                .unwrap()
                .get_mut("serde")
                .unwrap();
            mutate_to_workspace(item);

            assert!(is_workspace_dep(&doc["dependencies"]["serde"]));
        }

        #[test]
        fn test_mutate_to_workspace_inline_table() {
            let mut doc: DocumentMut = r#"
                [dependencies]
                serde = { version = "1.0", features = ["derive"] }
            "#
            .parse()
            .unwrap();

            let item = doc["dependencies"]
                .as_table_mut()
                .unwrap()
                .get_mut("serde")
                .unwrap();
            mutate_to_workspace(item);

            let output = doc.to_string();
            assert!(output.contains("workspace = true"));
            assert!(!output.contains("version"));
            // Features should be preserved
            assert!(output.contains("features"));
        }

        #[test]
        fn test_ensure_workspace_deps_creates_structure() {
            let mut doc: DocumentMut = r#"
                [package]
                name = "test"
            "#
            .parse()
            .unwrap();

            let ws_deps = ensure_workspace_deps(&mut doc);
            ws_deps.insert("serde", Item::Value(Value::from("1.0")));

            let output = doc.to_string();
            assert!(output.contains("[workspace.dependencies]") || output.contains("[workspace]"));
            assert!(output.contains("serde"));
        }
    }

    mod sorting {
        use super::*;

        #[test]
        fn test_sort_table_alphabetical() {
            let mut doc: DocumentMut = r#"
                [dependencies]
                zebra = "1.0"
                apple = "2.0"
                mango = "3.0"
            "#
            .parse()
            .unwrap();

            let table = doc["dependencies"].as_table_mut().unwrap();
            sort_table(table, None);

            let keys: Vec<_> = table.iter().map(|(k, _)| k.to_string()).collect();
            assert_eq!(keys, vec!["apple", "mango", "zebra"]);
        }

        #[test]
        fn test_sort_table_with_prefix() {
            let mut doc: DocumentMut = r#"
                [dependencies]
                zebra = "1.0"
                kintsu-core = "2.0"
                apple = "3.0"
                kintsu-api = "4.0"
            "#
            .parse()
            .unwrap();

            let table = doc["dependencies"].as_table_mut().unwrap();
            sort_table(table, Some("kintsu"));

            let keys: Vec<_> = table.iter().map(|(k, _)| k.to_string()).collect();
            // kintsu-* deps should come first (sorted), then others (sorted)
            assert_eq!(keys, vec!["kintsu-api", "kintsu-core", "apple", "zebra"]);
        }

        #[test]
        fn test_sort_document_sorts_multiple_sections() {
            let mut doc: DocumentMut = r#"
                [dependencies]
                zebra = "1.0"
                apple = "2.0"

                [dev-dependencies]
                beta = "1.0"
                alpha = "2.0"
            "#
            .parse()
            .unwrap();

            let sections = vec!["dependencies".into(), "dev-dependencies".into()];
            sort_document(&mut doc, &sections, None);

            let deps: Vec<_> = doc["dependencies"]
                .as_table()
                .unwrap()
                .iter()
                .map(|(k, _)| k.to_string())
                .collect();
            assert_eq!(deps, vec!["apple", "zebra"]);

            let dev_deps: Vec<_> = doc["dev-dependencies"]
                .as_table()
                .unwrap()
                .iter()
                .map(|(k, _)| k.to_string())
                .collect();
            assert_eq!(dev_deps, vec!["alpha", "beta"]);
        }

        #[test]
        fn test_sort_document_sorts_workspace_members() {
            let mut doc: DocumentMut = r#"
                [workspace]
                members = ["crate-c", "crate-a", "crate-b"]
            "#
            .parse()
            .unwrap();

            sort_document(&mut doc, &[], None);

            let members: Vec<_> = doc["workspace"]["members"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str())
                .collect();
            assert_eq!(members, vec!["crate-a", "crate-b", "crate-c"]);
        }
    }

    mod integration {
        use super::*;

        #[test]
        fn test_find_cargo_tomls() {
            let dir = create_test_workspace();

            write_file(dir.path(), "Cargo.toml", "[workspace]");
            write_file(dir.path(), "crate-a/Cargo.toml", "[package]");
            write_file(dir.path(), "crate-b/Cargo.toml", "[package]");
            write_file(dir.path(), "target/debug/Cargo.toml", "[package]"); // Should be ignored

            let config = HoistConfig::new();
            let files = find_cargo_tomls(dir.path(), &config.ignores);

            assert_eq!(files.len(), 3);
            assert!(
                files
                    .iter()
                    .all(|p| !p.to_string_lossy().contains("target"))
            );
        }

        #[test]
        fn test_backup_file() {
            let dir = create_test_workspace();
            let file_path = dir.path().join("Cargo.toml");
            fs::write(&file_path, "original content").unwrap();

            let bak_path = backup_file(&file_path).unwrap();

            assert!(bak_path.exists());
            assert_eq!(fs::read_to_string(&bak_path).unwrap(), "original content");
        }

        #[test]
        fn test_hoist_basic() {
            let dir = create_test_workspace();

            // Root Cargo.toml (workspace)
            write_file(
                dir.path(),
                "Cargo.toml",
                r#"
[workspace]
members = ["crate-a", "crate-b"]
"#,
            );

            // Crate A with serde dependency
            write_file(
                dir.path(),
                "crate-a/Cargo.toml",
                r#"
[package]
name = "crate-a"
version = "0.1.0"

[dependencies]
serde = "1.0"
"#,
            );

            // Crate B with serde and tokio dependencies
            write_file(
                dir.path(),
                "crate-b/Cargo.toml",
                r#"
[package]
name = "crate-b"
version = "0.1.0"

[dependencies]
serde = "1.0"
tokio = "1.0"
"#,
            );

            let config = HoistConfig::new();
            let stats = hoist(dir.path(), &config, false).unwrap();

            // Should have added serde and tokio to workspace.dependencies
            assert_eq!(stats.added, 2);
            // Should have updated 3 occurrences (serde in a, serde in b, tokio in b)
            assert_eq!(stats.updated, 3);
            assert_eq!(stats.skipped, 0);

            // Verify root Cargo.toml was updated
            let root_content = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
            assert!(root_content.contains("[workspace.dependencies]"));
            assert!(root_content.contains("serde"));
            assert!(root_content.contains("tokio"));

            // Verify member crates were updated
            let crate_a = fs::read_to_string(dir.path().join("crate-a/Cargo.toml")).unwrap();
            assert!(crate_a.contains("workspace = true"));

            let crate_b = fs::read_to_string(dir.path().join("crate-b/Cargo.toml")).unwrap();
            assert!(crate_b.contains("workspace = true"));
        }

        #[test]
        fn test_hoist_skips_path_deps() {
            let dir = create_test_workspace();

            write_file(
                dir.path(),
                "Cargo.toml",
                r#"
[workspace]
members = ["crate-a"]
"#,
            );

            write_file(
                dir.path(),
                "crate-a/Cargo.toml",
                r#"
[package]
name = "crate-a"
version = "0.1.0"

[dependencies]
local-crate = { path = "../local-crate" }
"#,
            );

            let config = HoistConfig::new();
            let stats = hoist(dir.path(), &config, false).unwrap();

            assert_eq!(stats.added, 0);
            assert_eq!(stats.updated, 0);
            assert_eq!(stats.skipped, 1);
        }

        #[test]
        fn test_hoist_skips_existing_workspace_deps() {
            let dir = create_test_workspace();

            write_file(
                dir.path(),
                "Cargo.toml",
                r#"
[workspace]
members = ["crate-a"]
"#,
            );

            write_file(
                dir.path(),
                "crate-a/Cargo.toml",
                r#"
[package]
name = "crate-a"
version = "0.1.0"

[dependencies]
serde = { workspace = true }
"#,
            );

            let config = HoistConfig::new();
            let stats = hoist(dir.path(), &config, false).unwrap();

            // Already workspace dep, should be skipped entirely
            assert_eq!(stats.added, 0);
            assert_eq!(stats.updated, 0);
        }

        #[test]
        fn test_hoist_dry_run() {
            let dir = create_test_workspace();

            write_file(
                dir.path(),
                "Cargo.toml",
                r#"
[workspace]
members = ["crate-a"]
"#,
            );

            let original_content = r#"
[package]
name = "crate-a"
version = "0.1.0"

[dependencies]
serde = "1.0"
"#;

            write_file(dir.path(), "crate-a/Cargo.toml", original_content);

            let config = HoistConfig::new();
            let stats = hoist(dir.path(), &config, true).unwrap();

            assert_eq!(stats.added, 1);
            assert_eq!(stats.updated, 1);

            // Files should NOT be modified in dry run
            let crate_a = fs::read_to_string(dir.path().join("crate-a/Cargo.toml")).unwrap();
            assert!(!crate_a.contains("workspace = true"));
        }

        #[test]
        fn test_sort_integration() {
            let dir = create_test_workspace();

            write_file(
                dir.path(),
                "Cargo.toml",
                r#"
[workspace]
members = ["crate-b", "crate-a"]

[workspace.dependencies]
zebra = "1.0"
apple = "2.0"
"#,
            );

            let config = HoistConfig::new();
            let stats = sort(dir.path(), &config, false).unwrap();

            assert_eq!(stats.files_sorted, 1);

            let content = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
            let doc: DocumentMut = content.parse().unwrap();

            // Check workspace.dependencies is sorted
            let ws_deps: Vec<_> = doc["workspace"]["dependencies"]
                .as_table()
                .unwrap()
                .iter()
                .map(|(k, _)| k.to_string())
                .collect();
            assert_eq!(ws_deps, vec!["apple", "zebra"]);

            // Check members is sorted
            let members: Vec<_> = doc["workspace"]["members"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str())
                .collect();
            assert_eq!(members, vec!["crate-a", "crate-b"]);
        }

        #[test]
        fn test_restore_integration() {
            let dir = create_test_workspace();

            let original_content = "[package]\nname = \"original\"";
            let modified_content = "[package]\nname = \"modified\"";

            // Create original file
            write_file(dir.path(), "Cargo.toml", modified_content);
            // Create backup
            write_file(dir.path(), "Cargo.toml.bak", original_content);

            let config = HoistConfig::new();
            let stats = restore(dir.path(), &config, false).unwrap();

            assert_eq!(stats.files_restored, 1);

            // Verify file was restored
            let content = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
            assert_eq!(content, original_content);
        }

        #[test]
        fn test_restore_dry_run() {
            let dir = create_test_workspace();

            let original_content = "[package]\nname = \"original\"";
            let modified_content = "[package]\nname = \"modified\"";

            write_file(dir.path(), "Cargo.toml", modified_content);
            write_file(dir.path(), "Cargo.toml.bak", original_content);

            let config = HoistConfig::new();
            let stats = restore(dir.path(), &config, true).unwrap();

            assert_eq!(stats.files_restored, 1);

            // File should NOT be restored in dry run
            let content = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
            assert_eq!(content, modified_content);
        }

        #[test]
        fn test_hoist_preserves_extra_fields() {
            let dir = create_test_workspace();

            write_file(
                dir.path(),
                "Cargo.toml",
                r#"
[workspace]
members = ["crate-a"]
"#,
            );

            write_file(
                dir.path(),
                "crate-a/Cargo.toml",
                r#"
[package]
name = "crate-a"
version = "0.1.0"

[dependencies]
serde = { version = "1.0", features = ["derive"], optional = true }
"#,
            );

            let config = HoistConfig::new();
            hoist(dir.path(), &config, false).unwrap();

            let crate_a = fs::read_to_string(dir.path().join("crate-a/Cargo.toml")).unwrap();

            // Should have workspace = true
            assert!(crate_a.contains("workspace = true"));
            // Should preserve features
            assert!(crate_a.contains("features"));
            assert!(crate_a.contains("derive"));
            // Should preserve optional
            assert!(crate_a.contains("optional"));
            // Should NOT have version anymore
            assert!(!crate_a.contains("version = \"1.0\""));
        }

        #[test]
        fn test_hoist_handles_dev_and_build_deps() {
            let dir = create_test_workspace();

            write_file(
                dir.path(),
                "Cargo.toml",
                r#"
[workspace]
members = ["crate-a"]
"#,
            );

            write_file(
                dir.path(),
                "crate-a/Cargo.toml",
                r#"
[package]
name = "crate-a"
version = "0.1.0"

[dependencies]
serde = "1.0"

[dev-dependencies]
tokio-test = "0.4"

[build-dependencies]
cc = "1.0"
"#,
            );

            let config = HoistConfig::new();
            let stats = hoist(dir.path(), &config, false).unwrap();

            assert_eq!(stats.added, 3);
            assert_eq!(stats.updated, 3);

            let root = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
            assert!(root.contains("serde"));
            assert!(root.contains("tokio-test"));
            assert!(root.contains("cc"));
        }
    }

    mod errors {
        use super::*;

        #[test]
        fn test_hoist_no_cargo_toml() {
            let dir = create_test_workspace();
            let config = HoistConfig::new();

            let result = hoist(dir.path(), &config, false);

            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(matches!(err, HoistError::NoCargoToml(_)));
        }

        #[test]
        fn test_parse_invalid_toml() {
            let path = PathBuf::from("test.toml");
            let result = parse_toml("invalid [ toml", &path);

            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(matches!(err, HoistError::ParseToml { .. }));
        }

        #[test]
        fn test_invalid_root_directory() {
            let config = HoistConfig::new();
            let result = hoist(Path::new("/nonexistent/path/12345"), &config, false);

            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(matches!(err, HoistError::InvalidRoot(_)));
        }
    }

    mod config {
        use super::*;

        #[test]
        fn test_config_default() {
            let config = HoistConfig::new();

            assert!(config.ignores.contains("target"));
            assert!(config.ignores.contains(".git"));
            assert_eq!(config.sections.len(), 3);
            assert!(config.sort_prefix.is_none());
        }

        #[test]
        fn test_config_with_prefix() {
            let config = HoistConfig::new().with_prefix("mycompany");

            assert_eq!(config.sort_prefix, Some("mycompany".to_string()));
        }

        #[test]
        fn test_config_with_custom_ignores() {
            let config = HoistConfig::new().with_ignores(["custom", "dirs"]);

            assert!(config.ignores.contains("custom"));
            assert!(config.ignores.contains("dirs"));
            assert!(!config.ignores.contains("target")); // Replaced, not merged
        }
    }
}
