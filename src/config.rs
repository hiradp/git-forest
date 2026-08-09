use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::error::{AppError, Result};

const CONFIG_FILE: &str = ".forest.toml";

#[derive(Debug, Clone)]
pub struct Config {
    pub workspaces_root: PathBuf,
    pub repositories: Vec<RepositoryConfig>,
    branch_template: String,
}

#[derive(Debug, Clone)]
pub struct RepositoryConfig {
    pub name: String,
    pub path: PathBuf,
    pub remote: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    version: u32,
    repositories: RawRepositories,
    workspaces: RawWorkspaces,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRepositories {
    root: PathBuf,
    remote: Option<String>,
    members: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkspaces {
    root: PathBuf,
    branch: String,
}

impl Config {
    pub fn load(explicit_path: Option<&Path>) -> Result<Self> {
        let current_dir = env::current_dir().map_err(AppError::CurrentDirectory)?;
        let environment_path = env::var_os("FOREST_CONFIG").filter(|value| !value.is_empty());
        let path = discover_path(explicit_path, environment_path.as_deref(), &current_dir)?;
        Self::from_path(&path)
    }

    fn from_path(path: &Path) -> Result<Self> {
        let path = path
            .canonicalize()
            .map_err(|source| AppError::ResolveConfig {
                path: path.to_path_buf(),
                source,
            })?;
        let contents = fs::read_to_string(&path).map_err(|source| AppError::ReadConfig {
            path: path.clone(),
            source,
        })?;
        let raw: RawConfig = toml::from_str(&contents).map_err(|source| AppError::ParseConfig {
            path: path.clone(),
            source,
        })?;

        Self::from_raw(path, raw)
    }

    fn from_raw(path: PathBuf, raw: RawConfig) -> Result<Self> {
        if raw.version != 1 {
            return Err(AppError::InvalidConfig(format!(
                "unsupported version {}; expected 1",
                raw.version
            )));
        }

        validate_template(&raw.workspaces.branch, "workspaces.branch", "workspace")?;
        if let Some(remote) = &raw.repositories.remote {
            validate_template(remote, "repositories.remote", "name")?;
        }

        if raw.repositories.members.is_empty() {
            return Err(AppError::InvalidConfig(
                "repositories.members must not be empty".to_owned(),
            ));
        }

        let mut seen = HashSet::new();
        for name in &raw.repositories.members {
            validate_component(name, "repository name")?;
            if !seen.insert(name.clone()) {
                return Err(AppError::InvalidConfig(format!(
                    "duplicate repository member {name:?}"
                )));
            }
        }

        let project_root = path
            .parent()
            .expect("a canonical configuration path has a parent")
            .to_path_buf();
        let repositories_root =
            resolve_relative(&project_root, &raw.repositories.root, "repositories.root")?;
        let workspaces_root =
            resolve_relative(&project_root, &raw.workspaces.root, "workspaces.root")?;

        let remote_template = raw.repositories.remote;
        let repositories = raw
            .repositories
            .members
            .into_iter()
            .map(|name| RepositoryConfig {
                path: repositories_root.join(&name),
                remote: remote_template
                    .as_ref()
                    .map(|template| template.replace("{name}", &name)),
                name,
            })
            .collect();

        Ok(Self {
            workspaces_root,
            repositories,
            branch_template: raw.workspaces.branch,
        })
    }

    pub fn repository(&self, name: &str) -> Option<&RepositoryConfig> {
        self.repositories
            .iter()
            .find(|repository| repository.name == name)
    }

    pub fn branch_for(&self, workspace: &str) -> Result<String> {
        validate_workspace_name(workspace)?;
        Ok(self.branch_template.replace("{workspace}", workspace))
    }

    pub fn workspace_path(&self, workspace: &str) -> Result<PathBuf> {
        validate_workspace_name(workspace)?;
        Ok(self.workspaces_root.join(workspace))
    }
}

pub fn validate_workspace_name(name: &str) -> Result<()> {
    match component_error(name, "workspace name") {
        Some(message) => Err(AppError::InvalidInput(message)),
        None => Ok(()),
    }
}

fn discover_path(
    explicit_path: Option<&Path>,
    environment_path: Option<&std::ffi::OsStr>,
    current_dir: &Path,
) -> Result<PathBuf> {
    if let Some(path) = explicit_path {
        return Ok(resolve_from_current_dir(path, current_dir));
    }
    if let Some(path) = environment_path {
        return Ok(resolve_from_current_dir(Path::new(path), current_dir));
    }

    for ancestor in current_dir.ancestors() {
        let candidate = ancestor.join(CONFIG_FILE);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(AppError::ConfigNotFound)
}

fn resolve_from_current_dir(path: &Path, current_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_dir.join(path)
    }
}

fn resolve_relative(base: &Path, path: &Path, field: &str) -> Result<PathBuf> {
    if path.is_absolute() {
        return Err(AppError::InvalidConfig(format!(
            "{field} must be relative to the configuration directory"
        )));
    }

    let mut resolved = base.to_path_buf();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return Err(AppError::InvalidConfig(format!(
                        "{field} resolves above the filesystem root"
                    )));
                }
            }
            Component::Normal(component) => resolved.push(component),
            Component::Prefix(_) | Component::RootDir => {
                return Err(AppError::InvalidConfig(format!(
                    "{field} must be relative to the configuration directory"
                )));
            }
        }
    }
    Ok(resolved)
}

fn validate_component(value: &str, label: &str) -> Result<()> {
    match component_error(value, label) {
        Some(message) => Err(AppError::InvalidConfig(message)),
        None => Ok(()),
    }
}

fn component_error(value: &str, label: &str) -> Option<String> {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return Some(format!("{label} must not be empty"));
    };

    if !first.is_ascii_alphanumeric()
        || !characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        return Some(format!(
            "invalid {label} {value:?}; expected [A-Za-z0-9][A-Za-z0-9._-]*"
        ));
    }

    (value == "." || value == "..").then(|| format!("invalid {label} {value:?}"))
}

fn validate_template(template: &str, field: &str, required: &str) -> Result<()> {
    if template.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "{field} must not be empty"
        )));
    }

    let placeholders = placeholders(template, field)?;
    for placeholder in &placeholders {
        if *placeholder != required {
            return Err(AppError::InvalidConfig(format!(
                "unknown placeholder {{{placeholder}}} in {field}"
            )));
        }
    }

    if !placeholders.contains(&required) {
        return Err(AppError::InvalidConfig(format!(
            "{field} must contain {{{required}}}"
        )));
    }

    Ok(())
}

fn placeholders<'a>(template: &'a str, field: &str) -> Result<Vec<&'a str>> {
    let mut result = Vec::new();
    let mut offset = 0;

    while offset < template.len() {
        let Some((relative_index, character)) = template[offset..].char_indices().next() else {
            break;
        };
        let index = offset + relative_index;
        match character {
            '{' => {
                let content_start = index + character.len_utf8();
                let Some(relative_end) = template[content_start..].find('}') else {
                    return Err(AppError::InvalidConfig(format!(
                        "unclosed placeholder in {field}"
                    )));
                };
                let end = content_start + relative_end;
                let placeholder = &template[content_start..end];
                if placeholder.is_empty() || placeholder.contains('{') {
                    return Err(AppError::InvalidConfig(format!(
                        "invalid placeholder in {field}"
                    )));
                }
                result.push(placeholder);
                offset = end + 1;
            }
            '}' => {
                return Err(AppError::InvalidConfig(format!(
                    "unmatched closing brace in {field}"
                )));
            }
            _ => offset = index + character.len_utf8(),
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_config(members: &[&str]) -> RawConfig {
        RawConfig {
            version: 1,
            repositories: RawRepositories {
                root: PathBuf::from("src"),
                remote: Some("git@example.com:{name}.git".to_owned()),
                members: members.iter().map(|member| (*member).to_owned()).collect(),
            },
            workspaces: RawWorkspaces {
                root: PathBuf::from("src/.workspaces"),
                branch: "user/{workspace}".to_owned(),
            },
        }
    }

    #[test]
    fn preserves_repository_order_and_resolves_paths() {
        let config = Config::from_raw(
            PathBuf::from("/project/.forest.toml"),
            raw_config(&["second", "first"]),
        )
        .unwrap();

        assert_eq!(config.repositories[0].name, "second");
        assert_eq!(
            config.repositories[0].path,
            Path::new("/project/src/second")
        );
        assert_eq!(
            config.repositories[0].remote.as_deref(),
            Some("git@example.com:second.git")
        );
        assert_eq!(config.repositories[1].name, "first");
        assert_eq!(
            config.workspaces_root,
            Path::new("/project/src/.workspaces")
        );
    }

    #[test]
    fn rejects_duplicate_members() {
        let error = Config::from_raw(
            PathBuf::from("/project/.forest.toml"),
            raw_config(&["same", "same"]),
        )
        .unwrap_err();

        assert!(error.to_string().contains("duplicate repository"));
    }

    #[test]
    fn rejects_unknown_placeholders() {
        let mut raw = raw_config(&["repo"]);
        raw.workspaces.branch = "user/{task}".to_owned();

        let error = Config::from_raw(PathBuf::from("/project/.forest.toml"), raw).unwrap_err();

        assert!(error.to_string().contains("unknown placeholder {task}"));
    }

    #[test]
    fn validates_workspace_names() {
        for valid in ["logical-slots", "ABC_123", "one.two"] {
            validate_workspace_name(valid).unwrap();
        }
        for invalid in ["", ".", "..", "a/b", "-leading", "with space"] {
            assert!(validate_workspace_name(invalid).is_err(), "{invalid:?}");
        }
    }
}
