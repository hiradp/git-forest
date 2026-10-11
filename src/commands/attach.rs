use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::{Deserialize, Serialize};

use crate::cli::AttachArgs;
use crate::config::Config;
use crate::domain::{
    AttachStatus, AttachedTabReport, CommandOutcome, CommandReport, WorkspaceAttachReport,
};
use crate::error::{AppError, Result};
use crate::git::{Git, REPOSITORY_ENVIRONMENT};
use crate::workspace::{self, Ancestry, WorkspaceMetadata, WorkspaceState};

const PLAN_VERSION: u32 = 1;

/// What the attach command reads on standard input.
#[derive(Serialize)]
struct Plan<'a> {
    version: u32,
    /// The nearest existing ancestor of the first workspace.
    parent: Option<PlanParent<'a>>,
    /// The workspace being attached, then its descendants.
    workspaces: Vec<PlanWorkspace<'a>>,
}

#[derive(Serialize)]
struct PlanParent<'a> {
    name: &'a str,
    symbol: Option<&'a str>,
    dir: PathBuf,
}

#[derive(Serialize)]
struct PlanWorkspace<'a> {
    name: &'a str,
    symbol: Option<&'a str>,
    dir: &'a Path,
    parent: Option<&'a str>,
}

/// What the attach command writes to standard output.
#[derive(Deserialize)]
struct Attached {
    backend: String,
    /// One entry per planned workspace, in plan order.
    workspaces: Vec<AttachedWorkspace>,
    warnings: Vec<String>,
}

#[derive(Deserialize)]
struct AttachedWorkspace {
    name: String,
    session_id: String,
    status: AttachStatus,
    tab: AttachedTab,
}

#[derive(Deserialize)]
struct AttachedTab {
    label: String,
    dir: PathBuf,
    id: String,
    status: AttachStatus,
}

pub fn run(config: &Config, git: &Git, arguments: &AttachArgs) -> Result<CommandOutcome> {
    if config.attach_command.is_empty() {
        return Err(AppError::Operational(
            "attach.command is not configured; set it in the configuration to the command that opens workspaces"
                .to_owned(),
        ));
    }

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
    let mut targets = vec![root];
    collect_descendants(config, &states, &arguments.workspace, &mut targets)?;

    let plan = Plan {
        version: PLAN_VERSION,
        parent: parent
            .map(|parent| {
                Ok(PlanParent {
                    name: &parent.name,
                    symbol: parent.metadata.symbol.as_deref(),
                    dir: canonicalize(&parent.path, "parent workspace")?,
                })
            })
            .transpose()?,
        workspaces: targets
            .iter()
            .map(|target| PlanWorkspace {
                name: &target.state.name,
                symbol: target.metadata.symbol.as_deref(),
                dir: &target.path,
                parent: target.metadata.parent.as_deref(),
            })
            .collect(),
    };
    let attached = execute(&config.attach_command, &plan)?;
    warnings.extend(attached.warnings);

    let mut reports = targets
        .iter()
        .zip(attached.workspaces)
        .map(|(target, workspace)| report(&attached.backend, target, workspace));
    let mut root_report = reports
        .next()
        .expect("the plan starts with the attached workspace");
    root_report.warnings = warnings;
    root_report.descendants = reports.collect();
    Ok(CommandOutcome::success(CommandReport::WorkspaceAttach(
        root_report,
    )))
}

/// A workspace checked before the attach command runs.
struct Target<'a> {
    state: &'a WorkspaceState,
    metadata: WorkspaceMetadata,
    /// The configured path, which validates the name.
    configured_path: PathBuf,
    path: PathBuf,
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
    Ok(Target {
        state,
        metadata,
        configured_path,
        path,
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

/// Runs the attach command with the plan on its standard input and reads its
/// report from standard output. Its standard error reaches the user directly.
fn execute(command: &[String], plan: &Plan<'_>) -> Result<Attached> {
    let display = command.join(" ");
    let input = serde_json::to_vec(plan).map_err(|source| {
        AppError::Operational(format!("could not encode the attach plan: {source}"))
    })?;
    let output = pipe(command, &input).map_err(|source| {
        AppError::Operational(format!(
            "could not run attach command {display:?}: {source}"
        ))
    })?;
    if !output.status.success() {
        return Err(AppError::Operational(format!(
            "attach command {display:?} failed with {}",
            output.status
        )));
    }

    let attached: Attached = serde_json::from_slice(&output.stdout).map_err(|source| {
        AppError::Operational(format!(
            "attach command {display:?} returned an unexpected report: {source}"
        ))
    })?;
    if !attached
        .workspaces
        .iter()
        .map(|workspace| workspace.name.as_str())
        .eq(plan.workspaces.iter().map(|workspace| workspace.name))
    {
        return Err(AppError::Operational(format!(
            "attach command {display:?} returned an unexpected report: its workspaces do not match the plan"
        )));
    }
    Ok(attached)
}

fn pipe(command: &[String], input: &[u8]) -> std::io::Result<Output> {
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    for variable in REPOSITORY_ENVIRONMENT {
        process.env_remove(variable);
    }
    let mut child = process.spawn()?;
    let mut stdin = child.stdin.take().expect("standard input is piped");
    // Written beside the read so neither side blocks on a full pipe.
    std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input));
        let output = child.wait_with_output()?;
        match writer.join().expect("the plan writer does not panic") {
            // The command may exit without reading the whole plan.
            Err(source) if source.kind() != std::io::ErrorKind::BrokenPipe => Err(source),
            _ => Ok(output),
        }
    })
}

fn report(
    backend: &str,
    target: &Target<'_>,
    attached: AttachedWorkspace,
) -> WorkspaceAttachReport {
    WorkspaceAttachReport {
        workspace: target.state.name.clone(),
        path: target.configured_path.clone(),
        parent: target.metadata.parent.clone(),
        backend: backend.to_owned(),
        session_id: attached.session_id,
        status: attached.status,
        tabs: vec![AttachedTabReport {
            label: attached.tab.label,
            path: attached.tab.dir,
            tab_id: attached.tab.id,
            status: attached.tab.status,
        }],
        warnings: Vec::new(),
        descendants: Vec::new(),
    }
}

fn canonicalize(path: &Path, label: &str) -> Result<PathBuf> {
    path.canonicalize().map_err(|source| AppError::Filesystem {
        context: format!("could not resolve {label} {}", path.display()),
        source,
    })
}
