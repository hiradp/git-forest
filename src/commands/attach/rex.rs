use std::path::Path;

use super::{Attachment, MAIN_ROLE, Placement, Target, join_ids, names_workspace, tab_report};
use crate::domain::AttachStatus;
use crate::error::{AppError, Result};
use crate::rex::{Rex, RexSession, RexWindow};
use crate::workspace::WorkspaceState;

/// Where a workspace is already open. Rex has no metadata to tag, so labels
/// identify it: a session of its own, or a tab in another session.
#[derive(Clone, Copy)]
struct Open<'a> {
    session: &'a RexSession,
    window: Option<&'a RexWindow>,
}

impl Open<'_> {
    fn id(&self) -> &String {
        self.window.map_or(&self.session.id, |window| &window.id)
    }
}

pub(super) fn attach_tree(
    rex: &Rex<'_>,
    root: &Target<'_>,
    parent: Option<&WorkspaceState>,
    descendants: &[Target<'_>],
    warnings: &mut Vec<String>,
) -> Result<(Attachment, Vec<Attachment>)> {
    let current = Rex::current_session_id();
    let attachment = attach_one(rex, root, Placement::Current { current, parent })?;
    let mut attached = Vec::with_capacity(descendants.len());
    for target in descendants {
        let host = Placement::Host(attachment.host_id.clone());
        attached.push(attach_one(rex, target, host)?);
    }

    rex.focus_window(&attachment.host_id, &attachment.tab.id)?;
    // The workspace is open either way; only an app can change what it shows.
    if let Err(error) = rex.show_session(&attachment.host_id, &attachment.tab.id) {
        warnings.push(format!(
            "workspace {:?} is open in Rex session {}, but Rex could not switch to it: {error}",
            root.state.name, attachment.host_id
        ));
    }
    Ok((attachment, attached))
}

fn attach_one(rex: &Rex<'_>, target: &Target<'_>, placement: Placement<'_>) -> Result<Attachment> {
    let name = &target.state.name;
    let sessions = rex.sessions()?;
    let mut open = Vec::new();
    for session in &sessions {
        if names_workspace(&session.label, name) {
            open.push(Open {
                session,
                window: None,
            });
            continue;
        }
        open.extend(
            session
                .windows
                .iter()
                .filter(|window| names_workspace(&window.label, name))
                .map(|window| Open {
                    session,
                    window: Some(window),
                }),
        );
    }
    if open.len() > 1 {
        let candidates = join_ids(open.iter().map(Open::id));
        open.retain(|open| match open.window {
            Some(window) => rooted(rex, open.session, window, &target.path),
            None => open
                .session
                .windows
                .iter()
                .any(|window| rooted(rex, open.session, window, &target.path)),
        });
        if open.len() != 1 {
            return Err(AppError::Operational(format!(
                "multiple Rex sessions or tabs are labeled for workspace {name:?}: {candidates}"
            )));
        }
    }

    match open.first() {
        Some(Open {
            session,
            window: None,
        }) => reattach_standalone(rex, session, target),
        Some(Open {
            session,
            window: Some(window),
        }) => reattach_tab(rex, session, window, target),
        None => {
            let host = match placement {
                Placement::Host(host) => Some(host),
                Placement::Current {
                    current: Some(current),
                    parent: Some(parent),
                } => sessions
                    .iter()
                    .find(|session| session.id == current)
                    .filter(|session| {
                        names_workspace(&session.label, &parent.name)
                            || session
                                .windows
                                .iter()
                                .any(|window| names_workspace(&window.label, &parent.name))
                    })
                    .map(|session| session.id.clone()),
                Placement::Current { .. } => None,
            };
            let created = AttachStatus::Created;
            let (host_id, tab) = match host {
                Some(host) => {
                    let label = &target.display_name;
                    let window_id = rex.create_window(&host, label, &target.path)?;
                    (host, tab_report(label, &target.path, window_id, created))
                }
                None => {
                    let session =
                        rex.create_session(&target.display_name, MAIN_ROLE, &target.path)?;
                    let tab = tab_report(MAIN_ROLE, &target.path, session.window_id, created);
                    (session.session_id, tab)
                }
            };
            Ok(Attachment {
                host_id,
                status: created,
                tab,
            })
        }
    }
}

fn reattach_standalone(
    rex: &Rex<'_>,
    session: &RexSession,
    target: &Target<'_>,
) -> Result<Attachment> {
    let path = &target.path;
    let mut mains = session
        .windows
        .iter()
        .filter(|window| window.label == MAIN_ROLE)
        .collect::<Vec<_>>();
    if mains.len() > 1 {
        let candidates = join_ids(mains.iter().map(|window| &window.id));
        mains.retain(|window| rooted(rex, session, window, path));
        if mains.len() != 1 {
            return Err(AppError::Operational(format!(
                "multiple Rex tabs are labeled {MAIN_ROLE:?} in session {}: {candidates}",
                session.id
            )));
        }
    }
    let (tab_id, tab_status) = match mains.first() {
        Some(window) => (window.id.clone(), AttachStatus::Reused),
        None => (
            rex.create_window(&session.id, MAIN_ROLE, path)?,
            AttachStatus::Created,
        ),
    };

    let mut changed = tab_status == AttachStatus::Created;
    if target.metadata.symbol.is_some() && session.label != target.display_name {
        rex.rename_session(&session.id, &target.display_name)?;
        changed = true;
    }

    Ok(Attachment {
        host_id: session.id.clone(),
        status: if changed {
            AttachStatus::Reconciled
        } else {
            AttachStatus::Reused
        },
        tab: tab_report(MAIN_ROLE, path, tab_id, tab_status),
    })
}

fn reattach_tab(
    rex: &Rex<'_>,
    session: &RexSession,
    window: &RexWindow,
    target: &Target<'_>,
) -> Result<Attachment> {
    let label = &target.display_name;
    let status = if &window.label == label {
        AttachStatus::Reused
    } else {
        rex.rename_window(&session.id, &window.id, label)?;
        AttachStatus::Reconciled
    };
    Ok(Attachment {
        host_id: session.id.clone(),
        status,
        tab: tab_report(label, &target.path, window.id.clone(), status),
    })
}

/// Whether a pane in the window is working inside the workspace.
fn rooted(rex: &Rex<'_>, session: &RexSession, window: &RexWindow, path: &Path) -> bool {
    window.block_ids.iter().any(|block_id| {
        rex.block_cwd(&session.id, block_id)
            .is_some_and(|cwd| cwd.canonicalize().unwrap_or(cwd).starts_with(path))
    })
}
