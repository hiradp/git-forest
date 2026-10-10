mod rex;

use std::path::{Path, PathBuf};

use crate::cli::AttachArgs;
use crate::config::{Config, Multiplexer};
use crate::domain::{
    AttachStatus, AttachedTabReport, CommandOutcome, CommandReport, WorkspaceAttachReport,
};
use crate::error::{AppError, Result};
use crate::git::Git;
use crate::herdr::{
    self, Herdr, HerdrPane, HerdrTab, HerdrWorkspace, TAB_TOKEN, WORKSPACE_ID_TOKEN,
    WORKSPACE_PATH_TOKEN,
};
use crate::rex::Rex;
use crate::workspace::{self, Ancestry, WorkspaceMetadata, WorkspaceState};

const MAIN_ROLE: &str = "main";
const WORKSPACE_ROLE_PREFIX: &str = "workspace:";

struct Attachment {
    /// The Herdr workspace or Rex session holding the tab.
    host_id: String,
    status: AttachStatus,
    tab: AttachedTab,
}

struct AttachedTab {
    label: String,
    path: PathBuf,
    id: String,
    status: AttachStatus,
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

    let multiplexer = multiplexer(config, herdr, arguments);
    let (attachment, attached_descendants) = match multiplexer {
        Multiplexer::Herdr => attach_tree(herdr, &root, parent, &descendants)?,
        Multiplexer::Rex => {
            let rex = Rex::new(&config.rex_command);
            rex::attach_tree(&rex, &root, parent, &descendants, &mut warnings)?
        }
    };

    let mut root_report = report(multiplexer, &root, attachment, warnings);
    root_report.descendants = descendants
        .iter()
        .zip(attached_descendants)
        .map(|(target, attached)| report(multiplexer, target, attached, Vec::new()))
        .collect();
    Ok(CommandOutcome::success(CommandReport::WorkspaceAttach(
        root_report,
    )))
}

/// The `--multiplexer` flag, then configuration, then the multiplexer `attach`
/// runs in, then Herdr.
fn multiplexer(config: &Config, herdr: &Herdr, arguments: &AttachArgs) -> Multiplexer {
    arguments
        .multiplexer
        .or(config.multiplexer)
        .unwrap_or_else(|| {
            if herdr.current_workspace_id().is_none() && Rex::current_session_id().is_some() {
                Multiplexer::Rex
            } else {
                Multiplexer::Herdr
            }
        })
}

fn attach_tree(
    herdr: &Herdr,
    root: &Target<'_>,
    parent: Option<&WorkspaceState>,
    descendants: &[Target<'_>],
) -> Result<(Attachment, Vec<Attachment>)> {
    for target in std::iter::once(root).chain(descendants) {
        Identity::new(&target.path)?;
    }
    let current = herdr.current_workspace_id();
    let attachment = attach_one(herdr, root, Placement::Current { current, parent })?;
    let mut attached = Vec::with_capacity(descendants.len());
    for target in descendants {
        let host = Placement::Host(attachment.host_id.clone());
        attached.push(attach_one(herdr, target, host)?);
    }

    herdr.focus_workspace(&attachment.host_id)?;
    herdr.focus_tab(&attachment.tab.id)?;
    Ok((attachment, attached))
}

/// A workspace checked before any Herdr or Rex call.
struct Target<'a> {
    state: &'a WorkspaceState,
    metadata: WorkspaceMetadata,
    /// The configured path, which validates the name.
    configured_path: PathBuf,
    path: PathBuf,
    display_name: String,
    /// The label of the workspace's own tab when it is open standalone.
    main_label: String,
}

enum Placement<'a> {
    /// Beside the parent when it is open in the Herdr workspace or Rex session
    /// `attach` runs in.
    Current {
        current: Option<String>,
        parent: Option<&'a WorkspaceState>,
    },
    /// In this Herdr workspace or Rex session, which holds the attached root.
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
    let (display_name, main_label) = match &metadata.symbol {
        Some(symbol) => (format!("{symbol} {name}"), format!("{symbol} {MAIN_ROLE}")),
        None => (name.to_owned(), MAIN_ROLE.to_owned()),
    };
    Ok(Target {
        state,
        metadata,
        configured_path,
        path,
        display_name,
        main_label,
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
    multiplexer: Multiplexer,
    target: &Target<'_>,
    attachment: Attachment,
    warnings: Vec<String>,
) -> WorkspaceAttachReport {
    let herdr = |id: String| (multiplexer == Multiplexer::Herdr).then_some(id);
    let rex = |id: String| (multiplexer == Multiplexer::Rex).then_some(id);
    let Attachment {
        host_id,
        status,
        tab,
    } = attachment;
    WorkspaceAttachReport {
        workspace: target.state.name.clone(),
        path: target.configured_path.clone(),
        parent: target.metadata.parent.clone(),
        multiplexer,
        herdr_workspace_id: herdr(host_id.clone()),
        rex_session_id: rex(host_id),
        status,
        tabs: vec![AttachedTabReport {
            label: tab.label,
            path: tab.path,
            herdr_tab_id: herdr(tab.id.clone()),
            rex_window_id: rex(tab.id),
            status: tab.status,
        }],
        warnings,
        descendants: Vec::new(),
    }
}

fn attach_one(herdr: &Herdr, target: &Target<'_>, placement: Placement<'_>) -> Result<Attachment> {
    let name = &target.state.name;
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
            reattach_standalone(herdr, existing, target, untagged)
        }
        (None, Some(pane)) => reattach_tab(herdr, pane, display_name, path),
        (None, None) => {
            // A partial standalone workspace's only tab can carry the
            // workspace's name, so it is checked before child tabs.
            let recoverable = recoverable_workspace(herdr, &workspaces, name, path)?;
            let untagged_tab = match recoverable {
                Some(_) => None,
                None => recoverable_tab(herdr, &panes, display_name, path)?,
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
                    let here = workspaces.iter().find(|workspace| workspace.id == current);
                    let untagged_parent = here.filter(|workspace| {
                        is_untagged_standalone(workspace, &parent.name)
                            && panes.iter().any(|pane| {
                                pane.workspace_id == workspace.id
                                    && pane
                                        .cwd
                                        .as_deref()
                                        .is_some_and(|cwd| paths_match(cwd, &parent_path))
                            })
                    });
                    if let Some(workspace) = untagged_parent {
                        herdr.report_workspace_id(&workspace.id, &parent_identity.id)?;
                    }
                    here.filter(|workspace| {
                        untagged_parent.is_some()
                            || parent_untagged_here
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
                reattach_standalone(herdr, &existing, target, true)
            } else if let Some(host) = host {
                create_tab(herdr, &host, &role, display_name, path)
            } else {
                create_standalone(herdr, target, &identity)
            }
        }
    }
}

fn create_standalone(
    herdr: &Herdr,
    target: &Target<'_>,
    identity: &Identity<'_>,
) -> Result<Attachment> {
    let path = &target.path;
    let main_label = &target.main_label;
    let created = herdr.create_workspace(path, &target.display_name)?;
    let workspace_id = created.workspace.id;
    herdr.report_workspace_id(&workspace_id, &identity.id)?;
    herdr.report_tab_role(&created.root_pane.id, MAIN_ROLE)?;
    if &created.tab.label != main_label {
        herdr.rename_tab(&created.tab.id, main_label)?;
    }
    Ok(Attachment {
        host_id: workspace_id,
        status: AttachStatus::Created,
        tab: tab_report(main_label, path, created.tab.id, AttachStatus::Created),
    })
}

fn create_tab(
    herdr: &Herdr,
    workspace_id: &str,
    role: &str,
    display_name: &str,
    path: &Path,
) -> Result<Attachment> {
    let created = herdr.create_tab(workspace_id, path, display_name)?;
    herdr.report_tab_role(&created.root_pane.id, role)?;
    Ok(Attachment {
        host_id: workspace_id.to_owned(),
        status: AttachStatus::Created,
        tab: tab_report(display_name, path, created.tab.id, AttachStatus::Created),
    })
}

fn reattach_tab(
    herdr: &Herdr,
    pane: &HerdrPane,
    display_name: &str,
    path: &Path,
) -> Result<Attachment> {
    let tab = find_tab(herdr, pane)?;
    let status = if tab.label == display_name {
        AttachStatus::Reused
    } else {
        herdr.rename_tab(&tab.id, display_name)?;
        AttachStatus::Reconciled
    };
    Ok(Attachment {
        host_id: pane.workspace_id.clone(),
        status,
        tab: tab_report(display_name, path, tab.id, status),
    })
}

fn reattach_standalone(
    herdr: &Herdr,
    existing: &HerdrWorkspace,
    target: &Target<'_>,
    recovered: bool,
) -> Result<Attachment> {
    let display_name = &target.display_name;
    let main_label = &target.main_label;
    let path = &target.path;
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

    let (tab_id, tab_status) = match main {
        Some(tab) => {
            if &tab.label == main_label {
                (tab.id, AttachStatus::Reused)
            } else {
                herdr.rename_tab(&tab.id, main_label)?;
                changed = true;
                (tab.id, AttachStatus::Reconciled)
            }
        }
        None => {
            let created = create_tab(herdr, &existing.id, MAIN_ROLE, main_label, path)?;
            changed = true;
            (created.tab.id, AttachStatus::Created)
        }
    };

    if target.metadata.symbol.is_some() && existing.label.as_deref() != Some(display_name) {
        herdr.rename_workspace(&existing.id, display_name)?;
        changed = true;
    }

    Ok(Attachment {
        host_id: existing.id.clone(),
        status: if changed {
            AttachStatus::Reconciled
        } else {
            AttachStatus::Reused
        },
        tab: tab_report(main_label, path, tab_id, tab_status),
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
        .filter(|tab| tab.label == display_name)
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
    for workspace in workspaces
        .iter()
        .filter(|workspace| is_untagged_standalone(workspace, workspace_name))
    {
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

/// A Herdr workspace without Forest tokens whose label names the workspace.
fn is_untagged_standalone(workspace: &HerdrWorkspace, workspace_name: &str) -> bool {
    !workspace.tokens.contains_key(WORKSPACE_ID_TOKEN)
        && !workspace.tokens.contains_key(WORKSPACE_PATH_TOKEN)
        && workspace
            .label
            .as_deref()
            .is_some_and(|label| names_workspace(label, workspace_name))
}

/// Whether a label is the workspace's name, with or without a symbol.
fn names_workspace(label: &str, workspace_name: &str) -> bool {
    label == workspace_name
        || label.split_once(' ').is_some_and(|(symbol, name)| {
            name == workspace_name && workspace::validate_symbol(symbol).is_ok()
        })
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

fn tab_report(label: &str, path: &Path, tab_id: String, status: AttachStatus) -> AttachedTab {
    AttachedTab {
        label: label.to_owned(),
        path: path.to_path_buf(),
        id: tab_id,
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
