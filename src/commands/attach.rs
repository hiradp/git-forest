use std::path::{Path, PathBuf};

use crate::cli::AttachArgs;
use crate::config::Config;
use crate::domain::{
    AttachStatus, AttachedTabReport, CommandOutcome, CommandReport, WorkspaceAttachReport,
};
use crate::error::{AppError, Result};
use crate::git::Git;
use crate::herdr::{
    self, Herdr, HerdrPane, HerdrTab, HerdrWorkspace, TAB_TOKEN, WORKSPACE_ID_TOKEN,
    WORKSPACE_PATH_TOKEN,
};
use crate::workspace::{self, Ancestry, WorkspaceMetadata, WorkspaceState};

const MAIN_ROLE: &str = "main";
const WORKSPACE_ROLE_PREFIX: &str = "workspace:";

struct Attachment {
    herdr_workspace_id: String,
    status: AttachStatus,
    tab: AttachedTabReport,
}

pub fn run(
    config: &Config,
    git: &Git,
    herdr: &Herdr,
    arguments: &AttachArgs,
) -> Result<CommandOutcome> {
    let states = workspace::scan(config, git)?;
    let root = preflight(config, &states, &arguments.workspace)?;
    let mut warnings = Vec::new();
    let parent = match workspace::ancestry(&states, &arguments.workspace) {
        Ancestry::Cycle(names) => {
            return Err(AppError::Operational(format!(
                "workspace {:?} has cyclic parents: {}",
                arguments.workspace,
                names.join(" -> ")
            )));
        }
        Ancestry::Linked(ancestors) => ancestors.first().copied(),
        Ancestry::MissingParent { ancestors, parent } => {
            if ancestors.is_empty() {
                warnings.push(format!(
                    "parent workspace {parent:?} of {:?} does not exist; ignoring it",
                    arguments.workspace
                ));
            }
            ancestors.first().copied()
        }
    };
    let mut descendants = Vec::new();
    collect_descendants(config, &states, &arguments.workspace, &mut descendants)?;

    let current = herdr.current_workspace_id();
    let attachment = attach_one(herdr, &root, Placement::Current { current, parent })?;
    let mut descendant_reports = Vec::with_capacity(descendants.len());
    for target in &descendants {
        let host = Placement::Host(attachment.herdr_workspace_id.clone());
        let attached = attach_one(herdr, target, host)?;
        descendant_reports.push(report(target, attached, Vec::new()));
    }

    herdr.focus_workspace(&attachment.herdr_workspace_id)?;
    herdr.focus_tab(&attachment.tab.herdr_tab_id)?;

    let mut root_report = report(&root, attachment, warnings);
    root_report.descendants = descendant_reports;
    Ok(CommandOutcome::success(CommandReport::WorkspaceAttach(
        root_report,
    )))
}

/// A workspace checked before any Herdr call.
struct Target<'a> {
    state: &'a WorkspaceState,
    metadata: WorkspaceMetadata,
    /// The configured path, which validates the name.
    configured_path: PathBuf,
    path: PathBuf,
    display_name: String,
}

enum Placement<'a> {
    /// Beside the parent when it is open in the Herdr workspace `attach` runs in.
    Current {
        current: Option<String>,
        parent: Option<&'a WorkspaceState>,
    },
    /// In this Herdr workspace, which holds the attached root.
    Host(String),
}

fn preflight<'a>(config: &Config, states: &'a [WorkspaceState], name: &str) -> Result<Target<'a>> {
    let configured_path = config.workspace_path(name)?;
    let Some(state) = states
        .iter()
        .find(|state| state.name == name && state.exists)
    else {
        return Err(AppError::Operational(format!(
            "workspace {name:?} does not exist"
        )));
    };
    for member in &state.members {
        if !member.inconsistencies.is_empty() {
            return Err(AppError::Operational(format!(
                "workspace {name:?} has an inconsistent repository {:?}: {}",
                member.id,
                member.inconsistencies.join("; ")
            )));
        }
    }
    let metadata = workspace::read_metadata(&state.path)?;
    let path = canonicalize(&state.path, "Forest workspace")?;
    Identity::new(&path)?;
    let display_name = match &metadata.symbol {
        Some(symbol) => format!("{symbol} {name}"),
        None => name.to_owned(),
    };
    Ok(Target {
        state,
        metadata,
        configured_path,
        path,
        display_name,
    })
}

/// Depth-first in name order. The root's ancestry has no cycle and each
/// workspace has one parent, so this walks a tree.
fn collect_descendants<'a>(
    config: &Config,
    states: &'a [WorkspaceState],
    name: &str,
    descendants: &mut Vec<Target<'a>>,
) -> Result<()> {
    for child in workspace::children(states, name) {
        descendants.push(preflight(config, states, &child.name)?);
        collect_descendants(config, states, &child.name, descendants)?;
    }
    Ok(())
}

fn report(
    target: &Target<'_>,
    attachment: Attachment,
    warnings: Vec<String>,
) -> WorkspaceAttachReport {
    WorkspaceAttachReport {
        workspace: target.state.name.clone(),
        path: target.configured_path.clone(),
        parent: target.metadata.parent.clone(),
        herdr_workspace_id: attachment.herdr_workspace_id,
        status: attachment.status,
        tabs: vec![attachment.tab],
        warnings,
        descendants: Vec::new(),
    }
}

fn attach_one(herdr: &Herdr, target: &Target<'_>, placement: Placement<'_>) -> Result<Attachment> {
    let name = &target.state.name;
    let metadata = &target.metadata;
    let display_name = &target.display_name;
    let path = &target.path;
    let identity = Identity::new(path)?;

    let workspaces = herdr.workspaces()?;
    let tagged_workspaces = workspaces
        .iter()
        .filter(|workspace| hosts_standalone(workspace, &identity))
        .collect::<Vec<_>>();
    if tagged_workspaces.len() > 1 {
        return Err(AppError::Operational(format!(
            "multiple Herdr workspaces match {}: {}",
            path.display(),
            join_ids(tagged_workspaces.iter().map(|workspace| &workspace.id))
        )));
    }
    let panes = herdr.all_panes()?;
    let role = workspace_role(&identity);
    let tagged_panes = panes
        .iter()
        .filter(|pane| has_role(pane, &role))
        .collect::<Vec<_>>();
    if tagged_panes.len() > 1 {
        return Err(AppError::Operational(format!(
            "multiple Herdr panes identify workspace {name:?}: {}",
            join_ids(tagged_panes.iter().map(|pane| &pane.id))
        )));
    }

    match (tagged_workspaces.first(), tagged_panes.first()) {
        (Some(existing), Some(pane)) => Err(AppError::Operational(format!(
            "workspace {name:?} is open both as Herdr workspace {} and as tab {}",
            existing.id, pane.tab_id
        ))),
        (Some(existing), None) => {
            let untagged = existing.tokens.get(WORKSPACE_ID_TOKEN) != Some(&identity.id);
            if untagged {
                herdr.report_workspace_id(&existing.id, &identity.id)?;
            }
            reattach_standalone(herdr, existing, metadata, display_name, path, untagged)
        }
        (None, Some(pane)) => reattach_tab(herdr, pane, display_name, path),
        (None, None) => {
            let untagged_tab = recoverable_tab(herdr, &panes, display_name, path)?;
            let recoverable = match untagged_tab {
                Some(_) => None,
                None => recoverable_workspace(herdr, &workspaces, name, path)?,
            };
            let host = match placement {
                Placement::Host(host) => Some(host),
                Placement::Current {
                    current: Some(current),
                    parent: Some(parent),
                } => {
                    let parent_path = canonicalize(&parent.path, "parent workspace")?;
                    let parent_identity = Identity::new(&parent_path)?;
                    let parent_display_name = match &parent.metadata.symbol {
                        Some(symbol) => format!("{symbol} {}", parent.name),
                        None => parent.name.clone(),
                    };
                    let parent_untagged_here =
                        recoverable_tab(herdr, &panes, &parent_display_name, &parent_path)?
                            .is_some_and(|pane| pane.workspace_id == current);
                    workspaces
                        .iter()
                        .find(|workspace| workspace.id == current)
                        .filter(|workspace| {
                            parent_untagged_here
                                || hosts_standalone(workspace, &parent_identity)
                                || panes.iter().any(|pane| {
                                    pane.workspace_id == workspace.id
                                        && has_role(pane, &workspace_role(&parent_identity))
                                })
                        })
                        .map(|workspace| workspace.id.clone())
                }
                Placement::Current { .. } => None,
            };
            if let Some(pane) = untagged_tab {
                herdr.report_tab_role(&pane.id, &role)?;
                let mut attachment = reattach_tab(herdr, &pane, display_name, path)?;
                attachment.status = AttachStatus::Reconciled;
                Ok(attachment)
            } else if let Some(existing) = recoverable {
                herdr.report_workspace_id(&existing.id, &identity.id)?;
                reattach_standalone(herdr, &existing, metadata, display_name, path, true)
            } else if let Some(host) = host {
                create_tab(herdr, &host, &role, display_name, path)
            } else {
                create_standalone(herdr, display_name, &identity, path)
            }
        }
    }
}

fn create_standalone(
    herdr: &Herdr,
    display_name: &str,
    identity: &Identity<'_>,
    path: &Path,
) -> Result<Attachment> {
    let created = herdr.create_workspace(path, display_name)?;
    let workspace_id = created.workspace.id;
    herdr.report_workspace_id(&workspace_id, &identity.id)?;
    herdr.report_tab_role(&created.root_pane.id, MAIN_ROLE)?;
    let label = tab_label(created.tab.number, MAIN_ROLE);
    if created.tab.label != label {
        herdr.rename_tab(&created.tab.id, &label)?;
    }
    Ok(Attachment {
        herdr_workspace_id: workspace_id,
        status: AttachStatus::Created,
        tab: tab_report(label, path, created.tab.id, AttachStatus::Created),
    })
}

fn create_tab(
    herdr: &Herdr,
    workspace_id: &str,
    role: &str,
    display_name: &str,
    path: &Path,
) -> Result<Attachment> {
    let next_number = herdr
        .tabs(workspace_id)?
        .iter()
        .map(|tab| tab.number)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let created = herdr.create_tab(workspace_id, path, &tab_label(next_number, display_name))?;
    herdr.report_tab_role(&created.root_pane.id, role)?;
    let label = tab_label(created.tab.number, display_name);
    if created.tab.label != label {
        herdr.rename_tab(&created.tab.id, &label)?;
    }
    Ok(Attachment {
        herdr_workspace_id: workspace_id.to_owned(),
        status: AttachStatus::Created,
        tab: tab_report(label, path, created.tab.id, AttachStatus::Created),
    })
}

fn reattach_tab(
    herdr: &Herdr,
    pane: &HerdrPane,
    display_name: &str,
    path: &Path,
) -> Result<Attachment> {
    let tab = find_tab(herdr, pane)?;
    let label = tab_label(tab.number, display_name);
    let status = if tab.label == label {
        AttachStatus::Reused
    } else {
        herdr.rename_tab(&tab.id, &label)?;
        AttachStatus::Reconciled
    };
    Ok(Attachment {
        herdr_workspace_id: pane.workspace_id.clone(),
        status,
        tab: tab_report(label, path, tab.id, status),
    })
}

fn reattach_standalone(
    herdr: &Herdr,
    existing: &HerdrWorkspace,
    metadata: &workspace::WorkspaceMetadata,
    display_name: &str,
    path: &Path,
    recovered: bool,
) -> Result<Attachment> {
    let tabs = herdr.tabs(&existing.id)?;
    let panes = herdr.panes(&existing.id)?;
    let tagged = panes
        .iter()
        .filter(|pane| has_role(pane, MAIN_ROLE))
        .collect::<Vec<_>>();
    if tagged.len() > 1 {
        return Err(AppError::Operational(format!(
            "multiple Herdr panes identify tab {MAIN_ROLE:?}: {}",
            join_ids(tagged.iter().map(|pane| &pane.id))
        )));
    }

    let mut changed = recovered;
    let main = if let Some(pane) = tagged.first() {
        Some(
            tabs.iter()
                .find(|tab| tab.id == pane.tab_id)
                .cloned()
                .ok_or_else(|| unknown_tab(pane))?,
        )
    } else {
        // Tabs holding any tagged pane belong to another managed role.
        let recoverable = tabs
            .iter()
            .filter(|tab| {
                !panes
                    .iter()
                    .any(|pane| pane.tab_id == tab.id && pane.tokens.contains_key(TAB_TOKEN))
            })
            .filter_map(|tab| {
                let pane = panes.iter().find(|pane| {
                    pane.tab_id == tab.id
                        && pane
                            .cwd
                            .as_deref()
                            .is_some_and(|cwd| paths_match(cwd, path))
                })?;
                Some((tab, pane))
            })
            .collect::<Vec<_>>();
        if recoverable.len() > 1 {
            return Err(AppError::Operational(format!(
                "multiple untagged Herdr tabs match {MAIN_ROLE:?}: {}",
                join_ids(recoverable.iter().map(|(tab, _)| &tab.id))
            )));
        }
        if let Some((tab, pane)) = recoverable.first() {
            herdr.report_tab_role(&pane.id, MAIN_ROLE)?;
            changed = true;
            Some((*tab).clone())
        } else {
            None
        }
    };

    let (tab_id, label, tab_status) = match main {
        Some(tab) => {
            let label = tab_label(tab.number, MAIN_ROLE);
            if tab.label == label {
                (tab.id, label, AttachStatus::Reused)
            } else {
                herdr.rename_tab(&tab.id, &label)?;
                changed = true;
                (tab.id, label, AttachStatus::Reconciled)
            }
        }
        None => {
            let created = create_tab(herdr, &existing.id, MAIN_ROLE, MAIN_ROLE, path)?;
            changed = true;
            (
                created.tab.herdr_tab_id,
                created.tab.label,
                AttachStatus::Created,
            )
        }
    };

    if metadata.symbol.is_some() && existing.label.as_deref() != Some(display_name) {
        herdr.rename_workspace(&existing.id, display_name)?;
        changed = true;
    }

    Ok(Attachment {
        herdr_workspace_id: existing.id.clone(),
        status: if changed {
            AttachStatus::Reconciled
        } else {
            AttachStatus::Reused
        },
        tab: tab_report(label, path, tab_id, tab_status),
    })
}

/// A pane in a tab whose tagged pane was closed, identified by the tab's
/// managed label and an untagged pane rooted in the workspace.
fn recoverable_tab(
    herdr: &Herdr,
    panes: &[HerdrPane],
    display_name: &str,
    path: &Path,
) -> Result<Option<HerdrPane>> {
    let mut recoverable = herdr
        .all_tabs()?
        .into_iter()
        .filter(|tab| tab.label == tab_label(tab.number, display_name))
        .filter(|tab| {
            !panes
                .iter()
                .any(|pane| pane.tab_id == tab.id && pane.tokens.contains_key(TAB_TOKEN))
        })
        .filter_map(|tab| {
            panes
                .iter()
                .find(|pane| {
                    pane.tab_id == tab.id
                        && pane
                            .cwd
                            .as_deref()
                            .is_some_and(|cwd| paths_match(cwd, path))
                })
                .cloned()
        })
        .collect::<Vec<_>>();
    if recoverable.len() > 1 {
        return Err(AppError::Operational(format!(
            "multiple untagged Herdr tabs match {display_name:?}: {}",
            join_ids(recoverable.iter().map(|pane| &pane.tab_id))
        )));
    }
    Ok(recoverable.pop())
}

fn recoverable_workspace(
    herdr: &Herdr,
    workspaces: &[HerdrWorkspace],
    workspace_name: &str,
    workspace_path: &Path,
) -> Result<Option<HerdrWorkspace>> {
    let mut recoverable = Vec::new();
    for workspace in workspaces.iter().filter(|workspace| {
        !workspace.tokens.contains_key(WORKSPACE_ID_TOKEN)
            && !workspace.tokens.contains_key(WORKSPACE_PATH_TOKEN)
            && workspace.label.as_deref().is_some_and(|label| {
                label == workspace_name
                    || label.split_once(' ').is_some_and(|(symbol, name)| {
                        name == workspace_name && workspace::validate_symbol(symbol).is_ok()
                    })
            })
    }) {
        let tabs = herdr.tabs(&workspace.id)?;
        let panes = herdr.panes(&workspace.id)?;
        if tabs.len() == 1
            && panes.len() == 1
            && panes[0]
                .cwd
                .as_deref()
                .is_some_and(|cwd| paths_match(cwd, workspace_path))
        {
            recoverable.push(workspace.clone());
        }
    }
    if recoverable.len() > 1 {
        return Err(AppError::Operational(format!(
            "multiple untagged Herdr workspaces match {}: {}",
            workspace_path.display(),
            join_ids(recoverable.iter().map(|workspace| &workspace.id))
        )));
    }
    Ok(recoverable.into_iter().next())
}

fn find_tab(herdr: &Herdr, pane: &HerdrPane) -> Result<HerdrTab> {
    herdr
        .tabs(&pane.workspace_id)?
        .into_iter()
        .find(|tab| tab.id == pane.tab_id)
        .ok_or_else(|| unknown_tab(pane))
}

fn unknown_tab(pane: &HerdrPane) -> AppError {
    AppError::Operational(format!(
        "Herdr pane {} references unknown tab {}",
        pane.id, pane.tab_id
    ))
}

fn hosts_standalone(workspace: &HerdrWorkspace, identity: &Identity<'_>) -> bool {
    workspace.tokens.get(WORKSPACE_ID_TOKEN) == Some(&identity.id)
        || workspace
            .tokens
            .get(WORKSPACE_PATH_TOKEN)
            .map(String::as_str)
            == Some(identity.path)
}

fn has_role(pane: &HerdrPane, role: &str) -> bool {
    pane.tokens
        .get(TAB_TOKEN)
        .is_some_and(|value| value == role)
}

fn workspace_role(identity: &Identity<'_>) -> String {
    format!("{WORKSPACE_ROLE_PREFIX}{}", identity.id)
}

fn tab_label(number: usize, name: &str) -> String {
    format!("{number}-{name}")
}

fn tab_report(
    label: String,
    path: &Path,
    tab_id: String,
    status: AttachStatus,
) -> AttachedTabReport {
    AttachedTabReport {
        label,
        path: path.to_path_buf(),
        herdr_tab_id: tab_id,
        status,
    }
}

fn join_ids<'a>(ids: impl Iterator<Item = &'a String>) -> String {
    ids.map(String::as_str).collect::<Vec<_>>().join(", ")
}

struct Identity<'a> {
    path: &'a str,
    id: String,
}

impl<'a> Identity<'a> {
    fn new(path: &'a Path) -> Result<Self> {
        let path = path.to_str().ok_or_else(|| {
            AppError::Operational(format!(
                "Herdr cannot identify non-UTF-8 workspace path {}",
                path.display()
            ))
        })?;
        Ok(Self {
            path,
            id: herdr::workspace_id(path),
        })
    }
}

fn canonicalize(path: &Path, label: &str) -> Result<PathBuf> {
    path.canonicalize().map_err(|source| AppError::Filesystem {
        context: format!("could not resolve {label} {}", path.display()),
        source,
    })
}

fn paths_match(left: &Path, right: &Path) -> bool {
    left == right
        || match (left.canonicalize(), right.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
}
