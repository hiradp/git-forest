use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use time::{OffsetDateTime, UtcOffset, format_description};

use crate::config::{Config, validate_workspace_name};
use crate::error::{AppError, Result};

const GENERATIONS_DIRECTORY: &str = ".generations";
const ID_FORMAT: &str =
    "[year]-[month]-[day]-[hour]-[minute]-[second][offset_hour sign:mandatory][offset_minute]";

#[derive(Debug, Clone)]
pub struct ArchiveEntry {
    pub workspace: String,
    pub archive_id: String,
    pub path: PathBuf,
}

pub fn scan(config: &Config) -> Result<Vec<ArchiveEntry>> {
    let root = config.archive_root();
    if !directory_exists(&root)? {
        return Ok(Vec::new());
    }
    let generations = root.join(GENERATIONS_DIRECTORY);
    let generations_exist = directory_exists(&generations)?;
    let mut archives = Vec::new();
    for path in entries(&root)? {
        let Some(workspace) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if validate_workspace_name(workspace).is_ok()
            && path_metadata(&path)?.is_some_and(|metadata| metadata.is_dir())
        {
            archives.push(ArchiveEntry {
                workspace: workspace.to_owned(),
                archive_id: "legacy".to_owned(),
                path: config.archive_path(workspace)?,
            });
        }
    }
    if generations_exist {
        for path in entries(&generations)? {
            let Some((workspace, archive_id)) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(parse_generation_name)
            else {
                continue;
            };
            if path_metadata(&path)?.is_some_and(|metadata| metadata.is_dir()) {
                archives.push(ArchiveEntry {
                    workspace,
                    archive_id,
                    path,
                });
            }
        }
    }
    archives.sort_by_cached_key(|entry| {
        (
            entry.workspace.clone(),
            parse_id(&entry.archive_id)
                .map(|(instant, sequence)| (instant.unix_timestamp(), sequence)),
            entry.archive_id.clone(),
        )
    });
    Ok(archives)
}

pub fn for_workspace(config: &Config, workspace: &str) -> Result<Vec<ArchiveEntry>> {
    validate_workspace_name(workspace)?;
    Ok(scan(config)?
        .into_iter()
        .filter(|archive| archive.workspace == workspace)
        .collect())
}

pub fn destination(config: &Config, workspace: &str) -> Result<ArchiveEntry> {
    let now = OffsetDateTime::now_utc();
    let offset = UtcOffset::local_offset_at(now).map_err(|source| {
        AppError::Operational(format!(
            "could not determine local timezone offset: {source}"
        ))
    })?;
    destination_at(config, workspace, now.to_offset(offset))
}

fn destination_at(
    config: &Config,
    workspace: &str,
    timestamp: OffsetDateTime,
) -> Result<ArchiveEntry> {
    validate_workspace_name(workspace)?;
    let generations = config.archive_root().join(GENERATIONS_DIRECTORY);
    validate_storage_root(config, &generations)?;
    let format =
        format_description::parse_borrowed::<2>(ID_FORMAT).expect("valid archive ID format");
    let id = timestamp.format(&format).map_err(|source| {
        AppError::Operational(format!("could not format archive timestamp: {source}"))
    })?;
    let mut archive_id = id.clone();
    let mut suffix = 2_u64;
    loop {
        let path = generations.join(format!("{workspace}--{archive_id}"));
        if path_metadata(&path)?.is_none() {
            return Ok(ArchiveEntry {
                workspace: workspace.to_owned(),
                archive_id,
                path,
            });
        }
        archive_id = format!("{id}-{suffix}");
        suffix = suffix.checked_add(1).ok_or_else(|| {
            AppError::Operational("archive timestamp collision limit reached".to_owned())
        })?;
    }
}

pub fn display_date(id: &str) -> String {
    let Some((timestamp, _)) = parse_id(id) else {
        return id.to_owned();
    };
    let format = format_description::parse_borrowed::<2>(
        "[year]-[month]-[day] [hour]:[minute]:[second] [offset_hour sign:mandatory]:[offset_minute]",
    )
    .expect("valid archive display date format");
    timestamp.format(&format).unwrap_or_else(|_| id.to_owned())
}

pub(crate) fn path_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(AppError::Filesystem {
            context: format!("could not inspect path {}", path.display()),
            source,
        }),
    }
}

pub(crate) fn entries(path: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(path).map_err(|source| AppError::Filesystem {
        context: format!("could not read archived workspace {}", path.display()),
        source,
    })?;
    let mut entries = entries
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|source| AppError::Filesystem {
                    context: format!("could not read an entry in {}", path.display()),
                    source,
                })
        })
        .collect::<Result<Vec<_>>>()?;
    entries.sort();
    Ok(entries)
}

fn directory_exists(path: &Path) -> Result<bool> {
    match path_metadata(path)? {
        None => Ok(false),
        Some(metadata) if metadata.is_dir() => Ok(true),
        Some(_) => Err(AppError::Operational(format!(
            "archive root {} exists and is not a directory (symlinks are not allowed)",
            path.display()
        ))),
    }
}

pub fn validate_storage_root(config: &Config, root: &Path) -> Result<()> {
    directory_exists(&config.archive_root())?;
    directory_exists(root)?;
    Ok(())
}

pub fn valid_id(id: &str) -> bool {
    id == "legacy" || parse_id(id).is_some()
}

fn parse_generation_name(name: &str) -> Option<(String, String)> {
    let (workspace, id) = name.rsplit_once("--")?;
    validate_workspace_name(workspace).ok()?;
    parse_id(id)?;
    Some((workspace.to_owned(), id.to_owned()))
}

fn parse_id(id: &str) -> Option<(OffsetDateTime, u64)> {
    let (timestamp, suffix) = id.split_at_checked(24)?;
    let sequence = if suffix.is_empty() {
        1
    } else {
        let suffix = suffix.strip_prefix('-')?;
        let number = suffix.parse::<u64>().ok()?;
        if number < 2 || number.to_string() != suffix {
            return None;
        }
        number
    };
    let format =
        format_description::parse_borrowed::<2>(ID_FORMAT).expect("valid archive ID format");
    Some((OffsetDateTime::parse(timestamp, &format).ok()?, sequence))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn config(root: &std::path::Path) -> Config {
        let path = root.join(".forest.toml");
        fs::write(
            &path,
            "version = 1\n[repositories]\nroot = \"repos\"\nmembers = [\"alpha\"]\n[workspaces]\nroot = \"workspaces\"\nbranch = \"{workspace}\"\n",
        )
        .unwrap();
        Config::load(Some(&path)).unwrap()
    }

    fn timestamp(offset: UtcOffset) -> OffsetDateTime {
        time::Date::from_calendar_date(2026, time::Month::September, 26)
            .unwrap()
            .with_hms(9, 46, 11)
            .unwrap()
            .assume_offset(offset)
    }

    #[test]
    fn generation_names_keep_workspace_delimiters_and_local_offsets_unambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        for workspace in ["topic-a", "topic--2026-09-26-09-46-11-0700", "topic---"] {
            for (offset, expected) in [
                (
                    UtcOffset::from_hms(-7, 0, 0).unwrap(),
                    "2026-09-26-09-46-11-0700",
                ),
                (
                    UtcOffset::from_hms(5, 30, 0).unwrap(),
                    "2026-09-26-09-46-11+0530",
                ),
            ] {
                let archive = destination_at(&config, workspace, timestamp(offset)).unwrap();
                assert_eq!(archive.archive_id, expected);
                assert_eq!(
                    archive.path.file_name().unwrap(),
                    format!("{workspace}--{expected}").as_str()
                );
                assert_eq!(
                    parse_generation_name(archive.path.file_name().unwrap().to_str().unwrap()),
                    Some((workspace.to_owned(), expected.to_owned()))
                );
            }
        }
        assert!(!config.archive_root().exists());
        assert_eq!(
            display_date("2026-09-26-09-46-11-0700-2"),
            "2026-09-26 09:46:11 -07:00"
        );
    }

    #[test]
    fn discovery_keeps_timestamp_looking_legacy_names_separate_from_generations() {
        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let legacy_name = "topic--2026-09-26-09-46-11-0700";
        let legacy = config.archive_path(legacy_name).unwrap();
        fs::create_dir_all(&legacy).unwrap();
        let generated = destination_at(&config, "topic", timestamp(UtcOffset::UTC)).unwrap();
        fs::create_dir_all(&generated.path).unwrap();
        fs::create_dir_all(
            config
                .archive_root()
                .join(".generations/invalid--timestamp"),
        )
        .unwrap();

        let archives = scan(&config).unwrap();
        assert_eq!(archives.len(), 2);
        let legacy_entries = for_workspace(&config, legacy_name).unwrap();
        assert_eq!(legacy_entries.len(), 1);
        assert_eq!(legacy_entries[0].archive_id, "legacy");
        assert_eq!(legacy_entries[0].path, legacy);
        let generated_entries = for_workspace(&config, "topic").unwrap();
        assert_eq!(generated_entries.len(), 1);
        assert_eq!(generated_entries[0].path, generated.path);
    }

    #[cfg(unix)]
    #[test]
    fn collisions_skip_directories_files_and_dangling_links_without_following_links() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let config = config(temp.path());
        let timestamp = timestamp(UtcOffset::UTC);
        let first = destination_at(&config, "topic-a", timestamp).unwrap();
        fs::create_dir_all(&first.path).unwrap();
        let second = destination_at(&config, "topic-a", timestamp).unwrap();
        assert!(second.archive_id.ends_with("+0000-2"));
        fs::write(&second.path, "occupied").unwrap();
        let third = destination_at(&config, "topic-a", timestamp).unwrap();
        assert!(third.archive_id.ends_with("+0000-3"));
        symlink("missing", &third.path).unwrap();
        let fourth = destination_at(&config, "topic-a", timestamp).unwrap();
        assert!(fourth.archive_id.ends_with("+0000-4"));
        assert!(!fourth.path.exists());
        symlink(&first.path, config.archive_root().join("legacy-link")).unwrap();
        assert_eq!(scan(&config).unwrap().len(), 1);
        assert_eq!(fs::read_to_string(second.path).unwrap(), "occupied");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_storage_roots_are_rejected_before_discovery_or_allocation() {
        use std::os::unix::fs::symlink;

        for root_name in [".archive", ".archive/.generations"] {
            let temp = tempfile::tempdir().unwrap();
            let config = config(temp.path());
            let root = config.workspaces_root.join(root_name);
            fs::create_dir_all(root.parent().unwrap()).unwrap();
            let external = temp.path().join("external");
            fs::create_dir(&external).unwrap();
            symlink(&external, &root).unwrap();

            assert!(scan(&config).is_err());
            assert!(destination_at(&config, "topic", timestamp(UtcOffset::UTC)).is_err());
            assert_eq!(fs::read_dir(&external).unwrap().count(), 0);
        }
    }
}
