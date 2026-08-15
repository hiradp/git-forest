use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::cli::AttachArgs;
use crate::config::Config;
use crate::domain::{
    AttachStatus, AttachedTabReport, CommandOutcome, CommandReport, WorkspaceAttachReport,
};
use crate::error::{AppError, Result};
use crate::git::Git;
use crate::herdr::{Herdr, HerdrPane, HerdrTab, HerdrWorkspace, TAB_TOKEN, WORKSPACE_PATH_TOKEN};
use crate::workspace;

const MAIN_ROLE: &str = "main";

#[derive(Debug)]
struct DesiredTab {
    role: String,
    name: String,
    path: PathBuf,
}

impl DesiredTab {
    fn label(&self, number: usize) -> String {
        format!("{number}-{}", self.name)
    }
}

#[derive(Debug)]
enum ExistingTabPlan {
    Reuse {
        tab: HerdrTab,
        rename: bool,
    },
    Recover {
        tab: HerdrTab,
        pane: HerdrPane,
        rename: bool,
    },
    Create,
}

pub fn run(
    config: &Config,
    git: &Git,
    herdr: &Herdr,
    arguments: &AttachArgs,
) -> Result<CommandOutcome> {
    let configured_path = config.workspace_path(&arguments.workspace)?;
    let mut states = workspace::scan(config, git)?;
    let state = states
        .drain(..)
        .find(|state| state.name == arguments.workspace)
        .ok_or_else(|| {
            AppError::Operational(format!(
                "workspace {:?} does not exist",
                arguments.workspace
            ))
        })?;
    if !state.exists {
        return Err(AppError::Operational(format!(
            "workspace {:?} does not exist",
            arguments.workspace
        )));
    }

    for member in &state.members {
        if !member.inconsistencies.is_empty() {
            return Err(AppError::Operational(format!(
                "workspace {:?} has an inconsistent repository {:?}: {}",
                arguments.workspace,
                member.id,
                member.inconsistencies.join("; ")
            )));
        }
    }

    let identity_path = canonicalize(&state.path, "Forest workspace")?;
    let identity = identity_path.to_str().ok_or_else(|| {
        AppError::Operational(format!(
            "Herdr cannot identify non-UTF-8 workspace path {}",
            identity_path.display()
        ))
    })?;
    let mut desired_tabs = vec![DesiredTab {
        role: MAIN_ROLE.to_owned(),
        name: MAIN_ROLE.to_owned(),
        path: identity_path.clone(),
    }];
    for member in &state.members {
        if !member.exists || !member.registered {
            continue;
        }
        let checkout = member.id.to_string();
        desired_tabs.push(DesiredTab {
            role: format!("repository:{checkout}"),
            name: checkout,
            path: canonicalize(&member.path, "repository worktree")?,
        });
    }

    let workspaces = herdr.workspaces()?;
    let matching_workspaces = workspaces
        .iter()
        .filter(|workspace| {
            workspace
                .tokens
                .get(WORKSPACE_PATH_TOKEN)
                .is_some_and(|path| path == identity)
        })
        .collect::<Vec<_>>();
    if matching_workspaces.len() > 1 {
        let ids = matching_workspaces
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(AppError::Operational(format!(
            "multiple Herdr workspaces match {}: {ids}",
            identity_path.display()
        )));
    }

    let existing = if let Some(existing) = matching_workspaces.first() {
        Some(((*existing).clone(), false))
    } else {
        let recoverable =
            recoverable_workspace(herdr, &workspaces, &arguments.workspace, &identity_path)?;
        if let Some(existing) = &recoverable {
            herdr.report_workspace_path(&existing.id, identity)?;
        }
        recoverable.map(|workspace| (workspace, true))
    };

    let (herdr_workspace_id, status, tabs, main_tab_id) =
        if let Some((existing, recovered)) = existing {
            let (mut status, tabs, main_tab_id) =
                reconcile_existing(herdr, &existing.id, &desired_tabs)?;
            if recovered {
                status = AttachStatus::Reconciled;
            }
            (existing.id, status, tabs, main_tab_id)
        } else {
            create_workspace(herdr, &arguments.workspace, identity, &desired_tabs)?
        };

    herdr.focus_workspace(&herdr_workspace_id)?;
    herdr.focus_tab(&main_tab_id)?;

    Ok(CommandOutcome::success(CommandReport::WorkspaceAttach(
        WorkspaceAttachReport {
            workspace: arguments.workspace.clone(),
            path: configured_path,
            herdr_workspace_id,
            status,
            tabs,
        },
    )))
}

fn create_workspace(
    herdr: &Herdr,
    workspace_name: &str,
    identity: &str,
    desired_tabs: &[DesiredTab],
) -> Result<(String, AttachStatus, Vec<AttachedTabReport>, String)> {
    let main = desired_tabs
        .first()
        .expect("every Herdr attachment has a main tab");
    let created = herdr.create_workspace(&main.path, workspace_name)?;
    let workspace_id = created.workspace.id;
    herdr.report_workspace_path(&workspace_id, identity)?;
    herdr.report_tab_role(&created.root_pane.id, &main.role)?;
    let main_label = main.label(created.tab.number);
    if created.tab.label != main_label {
        herdr.rename_tab(&created.tab.id, &main_label)?;
    }

    let main_tab_id = created.tab.id.clone();
    let mut next_tab_number = created.tab.number.saturating_add(1);
    let mut tabs = Vec::with_capacity(desired_tabs.len());
    tabs.push(tab_report(
        main,
        main_label,
        created.tab.id,
        AttachStatus::Created,
    ));
    for desired in &desired_tabs[1..] {
        let requested_label = desired.label(next_tab_number);
        let created = herdr.create_tab(&workspace_id, &desired.path, &requested_label)?;
        herdr.report_tab_role(&created.root_pane.id, &desired.role)?;
        let label = desired.label(created.tab.number);
        if created.tab.label != label {
            herdr.rename_tab(&created.tab.id, &label)?;
        }
        next_tab_number = created.tab.number.saturating_add(1);
        tabs.push(tab_report(
            desired,
            label,
            created.tab.id,
            AttachStatus::Created,
        ));
    }

    Ok((workspace_id, AttachStatus::Created, tabs, main_tab_id))
}

fn recoverable_workspace(
    herdr: &Herdr,
    workspaces: &[HerdrWorkspace],
    workspace_name: &str,
    workspace_path: &Path,
) -> Result<Option<HerdrWorkspace>> {
    let mut recoverable = Vec::new();
    for workspace in workspaces.iter().filter(|workspace| {
        !workspace.tokens.contains_key(WORKSPACE_PATH_TOKEN)
            && workspace.label.as_deref() == Some(workspace_name)
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
        let ids = recoverable
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(AppError::Operational(format!(
            "multiple untagged Herdr workspaces match {}: {ids}",
            workspace_path.display()
        )));
    }
    Ok(recoverable.into_iter().next())
}

fn reconcile_existing(
    herdr: &Herdr,
    workspace_id: &str,
    desired_tabs: &[DesiredTab],
) -> Result<(AttachStatus, Vec<AttachedTabReport>, String)> {
    let tabs = herdr.tabs(workspace_id)?;
    let panes = herdr.panes(workspace_id)?;
    let tabs_by_id = tabs
        .iter()
        .map(|tab| (tab.id.as_str(), tab))
        .collect::<HashMap<_, _>>();

    let mut plans = Vec::with_capacity(desired_tabs.len());
    for desired in desired_tabs {
        let tagged = panes
            .iter()
            .filter(|pane| {
                pane.tokens
                    .get(TAB_TOKEN)
                    .is_some_and(|role| role == &desired.role)
            })
            .collect::<Vec<_>>();
        if tagged.len() > 1 {
            let ids = tagged
                .iter()
                .map(|pane| pane.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(AppError::Operational(format!(
                "multiple Herdr panes identify tab {:?}: {ids}",
                desired.name
            )));
        }
        if let Some(pane) = tagged.first() {
            let tab = tabs_by_id.get(pane.tab_id.as_str()).ok_or_else(|| {
                AppError::Operational(format!(
                    "Herdr pane {} references unknown tab {}",
                    pane.id, pane.tab_id
                ))
            })?;
            plans.push(ExistingTabPlan::Reuse {
                tab: (*tab).clone(),
                rename: tab.label != desired.label(tab.number),
            });
            continue;
        }

        let recoverable = tabs
            .iter()
            .filter_map(|tab| {
                if desired.role != MAIN_ROLE && tab.label != desired.label(tab.number) {
                    return None;
                }
                let pane = panes.iter().find(|pane| {
                    pane.tab_id == tab.id
                        && pane
                            .cwd
                            .as_deref()
                            .is_some_and(|cwd| paths_match(cwd, &desired.path))
                })?;
                Some((tab.clone(), pane.clone()))
            })
            .collect::<Vec<_>>();
        if recoverable.len() > 1 {
            let ids = recoverable
                .iter()
                .map(|(tab, _)| tab.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(AppError::Operational(format!(
                "multiple untagged Herdr tabs match {:?}: {ids}",
                desired.name
            )));
        }
        if let Some((tab, pane)) = recoverable.into_iter().next() {
            let rename = tab.label != desired.label(tab.number);
            plans.push(ExistingTabPlan::Recover { tab, pane, rename });
        } else {
            plans.push(ExistingTabPlan::Create);
        }
    }

    let mut changed = false;
    let mut next_tab_number = tabs
        .iter()
        .map(|tab| tab.number)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut reports = Vec::with_capacity(desired_tabs.len());
    for (desired, plan) in desired_tabs.iter().zip(plans) {
        match plan {
            ExistingTabPlan::Reuse { tab, rename } => {
                let label = desired.label(tab.number);
                let status = if rename {
                    herdr.rename_tab(&tab.id, &label)?;
                    changed = true;
                    AttachStatus::Reconciled
                } else {
                    AttachStatus::Reused
                };
                reports.push(tab_report(desired, label, tab.id, status));
            }
            ExistingTabPlan::Recover { tab, pane, rename } => {
                let label = desired.label(tab.number);
                herdr.report_tab_role(&pane.id, &desired.role)?;
                if rename {
                    herdr.rename_tab(&tab.id, &label)?;
                }
                changed = true;
                reports.push(tab_report(desired, label, tab.id, AttachStatus::Reconciled));
            }
            ExistingTabPlan::Create => {
                let requested_label = desired.label(next_tab_number);
                let created = herdr.create_tab(workspace_id, &desired.path, &requested_label)?;
                herdr.report_tab_role(&created.root_pane.id, &desired.role)?;
                let label = desired.label(created.tab.number);
                if created.tab.label != label {
                    herdr.rename_tab(&created.tab.id, &label)?;
                }
                next_tab_number = created.tab.number.saturating_add(1);
                changed = true;
                reports.push(tab_report(
                    desired,
                    label,
                    created.tab.id,
                    AttachStatus::Created,
                ));
            }
        }
    }

    let main_tab_id = reports
        .first()
        .expect("every Herdr attachment has a main tab")
        .herdr_tab_id
        .clone();
    Ok((
        if changed {
            AttachStatus::Reconciled
        } else {
            AttachStatus::Reused
        },
        reports,
        main_tab_id,
    ))
}

fn tab_report(
    desired: &DesiredTab,
    label: String,
    tab_id: String,
    status: AttachStatus,
) -> AttachedTabReport {
    AttachedTabReport {
        label,
        path: desired.path.clone(),
        herdr_tab_id: tab_id,
        status,
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
