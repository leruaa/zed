use std::collections::{HashMap, HashSet};

use collections::HashSet as CollectionsHashSet;
use std::path::PathBuf;
use std::sync::Arc;

use fuzzy::StringMatchCandidate;
use git::repository::Branch as GitBranch;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Task, Window, rems,
};
use picker::{Picker, PickerDelegate, PickerEditorPosition};
use project::Project;
use ui::{
    HighlightedLabel, Icon, IconName, Label, LabelCommon, ListItem, ListItemSpacing, Tooltip,
    prelude::*,
};
use util::ResultExt as _;

use crate::StartThreadIn;

pub(crate) struct ThreadBranchPicker {
    picker: Entity<Picker<ThreadBranchPickerDelegate>>,
    focus_handle: FocusHandle,
    _subscription: gpui::Subscription,
}

impl ThreadBranchPicker {
    pub fn new(
        project: Entity<Project>,
        current_target: &StartThreadIn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project_worktree_paths: HashSet<PathBuf> = project
            .read(cx)
            .visible_worktrees(cx)
            .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
            .collect();

        let repository = project.read(cx).active_repository(cx);
        let branches_request = repository
            .clone()
            .map(|repo| repo.update(cx, |repo, _| repo.branches()));
        let worktrees_request = repository.map(|repo| repo.update(cx, |repo, _| repo.worktrees()));

        let (selected_entry_name, prefer_create_entry, preserved_worktree_name) =
            match current_target {
                StartThreadIn::NewWorktree {
                    worktree_name,
                    branch_name,
                    start_point,
                } => match (branch_name, start_point) {
                    (None, None) => (None, false, worktree_name.clone()),
                    (None, Some(start_point)) => {
                        (Some(start_point.clone()), false, worktree_name.clone())
                    }
                    (Some(branch_name), None) => {
                        (Some(branch_name.clone()), true, worktree_name.clone())
                    }
                    (Some(_branch_name), Some(start_point)) => {
                        (Some(start_point.clone()), false, worktree_name.clone())
                    }
                },
                _ => (None, false, None),
            };

        let delegate = ThreadBranchPickerDelegate {
            matches: vec![ThreadBranchEntry::RandomBranch],
            all_branches: None,
            occupied_branches: None,
            selected_index: 0,
            selected_entry_name,
            prefer_create_entry,
            preserved_worktree_name,
            project_worktree_paths,
        };

        let picker = cx.new(|cx| {
            Picker::list(delegate, window, cx)
                .list_measure_all()
                .modal(false)
                .max_height(Some(rems(20.).into()))
        });

        let focus_handle = picker.focus_handle(cx);

        if let (Some(branches_request), Some(worktrees_request)) =
            (branches_request, worktrees_request)
        {
            let picker_handle = picker.downgrade();
            cx.spawn_in(window, async move |_this, cx| {
                let branches = branches_request.await??;
                let worktrees = worktrees_request.await??;

                let remote_upstreams: CollectionsHashSet<_> = branches
                    .iter()
                    .filter_map(|branch| {
                        branch
                            .upstream
                            .as_ref()
                            .filter(|upstream| upstream.is_remote())
                            .map(|upstream| upstream.ref_name.clone())
                    })
                    .collect();

                let mut occupied_branches = HashMap::new();
                for worktree in worktrees {
                    let Some(branch_name) = worktree.branch_name().map(ToOwned::to_owned) else {
                        continue;
                    };

                    let reason = if picker_handle
                        .read_with(cx, |picker, _| {
                            picker
                                .delegate
                                .project_worktree_paths
                                .contains(&worktree.path)
                        })
                        .unwrap_or(false)
                    {
                        format!(
                            "This branch is already checked out in the current project worktree at {}.",
                            worktree.path.display()
                        )
                    } else {
                        format!(
                            "This branch is already checked out in a linked worktree at {}.",
                            worktree.path.display()
                        )
                    };

                    occupied_branches.insert(branch_name, reason);
                }

                let mut all_branches: Vec<_> = branches
                    .into_iter()
                    .filter(|branch| !remote_upstreams.contains(&branch.ref_name))
                    .collect();
                all_branches.sort_by_key(|branch| {
                    (
                        branch.is_remote(),
                        !branch.is_head,
                        branch
                            .most_recent_commit
                            .as_ref()
                            .map(|commit| 0 - commit.commit_timestamp),
                    )
                });

                picker_handle.update_in(cx, |picker, window, cx| {
                    picker.delegate.all_branches = Some(all_branches);
                    picker.delegate.occupied_branches = Some(occupied_branches);
                    picker.refresh(window, cx);
                })?;

                anyhow::Ok(())
            })
            .detach_and_log_err(cx);
        }

        let subscription = cx.subscribe(&picker, |_, _, _, cx| {
            cx.emit(DismissEvent);
        });

        Self {
            picker,
            focus_handle,
            _subscription: subscription,
        }
    }
}

impl Focusable for ThreadBranchPicker {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for ThreadBranchPicker {}

impl Render for ThreadBranchPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(rems(22.))
            .elevation_3(cx)
            .child(self.picker.clone())
            .on_mouse_down_out(cx.listener(|_, _, _, cx| {
                cx.emit(DismissEvent);
            }))
    }
}

#[derive(Clone)]
enum ThreadBranchEntry {
    RandomBranch,
    ExistingBranch {
        branch: GitBranch,
        positions: Vec<usize>,
        disabled_reason: Option<String>,
    },
    CreateNamed {
        name: String,
    },
}

pub(crate) struct ThreadBranchPickerDelegate {
    matches: Vec<ThreadBranchEntry>,
    all_branches: Option<Vec<GitBranch>>,
    occupied_branches: Option<HashMap<String, String>>,
    selected_index: usize,
    selected_entry_name: Option<String>,
    prefer_create_entry: bool,
    preserved_worktree_name: Option<String>,
    project_worktree_paths: HashSet<PathBuf>,
}

impl ThreadBranchPickerDelegate {
    fn sync_selected_index(&mut self) {
        if !self.prefer_create_entry {
            if let Some(selected_entry_name) = &self.selected_entry_name {
                if let Some(index) = self.matches.iter().position(|entry| {
                    matches!(
                        entry,
                        ThreadBranchEntry::ExistingBranch { branch, .. }
                            if branch.name() == selected_entry_name
                    )
                }) {
                    self.selected_index = index;
                    return;
                }
            }
        }

        if self.prefer_create_entry {
            if let Some(selected_entry_name) = &self.selected_entry_name {
                if let Some(index) = self.matches.iter().position(|entry| {
                    matches!(
                        entry,
                        ThreadBranchEntry::CreateNamed { name } if name == selected_entry_name
                    )
                }) {
                    self.selected_index = index;
                    return;
                }
            }
        }

        self.selected_index = 0;
    }
}

impl PickerDelegate for ThreadBranchPickerDelegate {
    type ListItem = ListItem;

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Search branches…".into()
    }

    fn editor_position(&self) -> PickerEditorPosition {
        PickerEditorPosition::Start
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = ix;
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let Some(all_branches) = self.all_branches.clone() else {
            self.matches = vec![ThreadBranchEntry::RandomBranch];
            self.selected_index = 0;
            return Task::ready(());
        };
        let occupied_branches = self.occupied_branches.clone().unwrap_or_default();

        if query.is_empty() {
            let mut matches = vec![ThreadBranchEntry::RandomBranch];
            for branch in all_branches {
                matches.push(ThreadBranchEntry::ExistingBranch {
                    disabled_reason: occupied_branches.get(branch.name()).cloned(),
                    branch,
                    positions: Vec::new(),
                });
            }

            if let Some(selected_entry_name) = &self.selected_entry_name {
                let has_existing = matches.iter().any(|entry| {
                    matches!(
                        entry,
                        ThreadBranchEntry::ExistingBranch { branch, .. }
                            if branch.name() == selected_entry_name
                    )
                });
                if self.prefer_create_entry && !has_existing {
                    matches.push(ThreadBranchEntry::CreateNamed {
                        name: selected_entry_name.clone(),
                    });
                }
            }

            self.matches = matches;
            self.sync_selected_index();
            return Task::ready(());
        }

        let candidates: Vec<_> = all_branches
            .iter()
            .enumerate()
            .map(|(ix, branch)| StringMatchCandidate::new(ix, branch.name()))
            .collect();
        let executor = cx.background_executor().clone();
        let query_clone = query.clone();

        let task = cx.background_executor().spawn(async move {
            fuzzy::match_strings(
                &candidates,
                &query_clone,
                true,
                true,
                10000,
                &Default::default(),
                executor,
            )
            .await
        });

        let all_branches_clone = all_branches.clone();
        return cx.spawn_in(window, async move |picker, cx| {
            let fuzzy_matches = task.await;

            picker
                .update_in(cx, |picker, _window, cx| {
                    let mut matches = vec![ThreadBranchEntry::RandomBranch];

                    for candidate in &fuzzy_matches {
                        let branch = all_branches_clone[candidate.candidate_id].clone();
                        let disabled_reason = occupied_branches.get(branch.name()).cloned();
                        matches.push(ThreadBranchEntry::ExistingBranch {
                            branch,
                            positions: candidate.positions.clone(),
                            disabled_reason,
                        });
                    }

                    if fuzzy_matches.is_empty() {
                        matches.push(ThreadBranchEntry::CreateNamed {
                            name: query.replace(' ', "-"),
                        });
                    }

                    picker.delegate.matches = matches;
                    picker.delegate.sync_selected_index();
                    cx.notify();
                })
                .log_err();
        });
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(entry) = self.matches.get(self.selected_index) else {
            return;
        };

        match entry {
            ThreadBranchEntry::RandomBranch => {
                window.dispatch_action(
                    Box::new(StartThreadIn::NewWorktree {
                        worktree_name: self.preserved_worktree_name.clone(),
                        branch_name: None,
                        start_point: None,
                    }),
                    cx,
                );
            }
            ThreadBranchEntry::ExistingBranch {
                branch,
                disabled_reason: None,
                ..
            } => {
                let action = if branch.is_remote() {
                    let branch_name = branch
                        .ref_name
                        .as_ref()
                        .strip_prefix("refs/remotes/")
                        .and_then(|stripped| stripped.split_once('/').map(|(_, name)| name))
                        .unwrap_or(branch.name())
                        .to_string();
                    StartThreadIn::NewWorktree {
                        worktree_name: self.preserved_worktree_name.clone(),
                        branch_name: Some(branch_name),
                        start_point: Some(branch.name().to_string()),
                    }
                } else {
                    StartThreadIn::NewWorktree {
                        worktree_name: self.preserved_worktree_name.clone(),
                        branch_name: None,
                        start_point: Some(branch.name().to_string()),
                    }
                };
                window.dispatch_action(Box::new(action), cx);
            }
            ThreadBranchEntry::ExistingBranch {
                disabled_reason: Some(_),
                ..
            } => {
                return;
            }
            ThreadBranchEntry::CreateNamed { name } => {
                window.dispatch_action(
                    Box::new(StartThreadIn::NewWorktree {
                        worktree_name: self.preserved_worktree_name.clone(),
                        branch_name: Some(name.clone()),
                        start_point: None,
                    }),
                    cx,
                );
            }
        }

        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _window: &mut Window, _cx: &mut Context<Picker<Self>>) {}

    fn separators_after_indices(&self) -> Vec<usize> {
        if self.matches.len() > 1 {
            vec![0]
        } else {
            Vec::new()
        }
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let entry = self.matches.get(ix)?;

        match entry {
            ThreadBranchEntry::RandomBranch => Some(
                ListItem::new("random-branch")
                    .inset(true)
                    .spacing(ListItemSpacing::Sparse)
                    .toggle_state(selected)
                    .start_slot(Icon::new(IconName::GitBranch).color(Color::Muted))
                    .child(Label::new("Random Branch")),
            ),
            ThreadBranchEntry::ExistingBranch {
                branch,
                positions,
                disabled_reason,
            } => {
                let is_disabled = disabled_reason.is_some();
                let icon_color = if is_disabled {
                    Color::Disabled
                } else {
                    Color::Muted
                };
                let label_color = if is_disabled {
                    Color::Disabled
                } else {
                    Color::Default
                };

                let item = ListItem::new(SharedString::from(format!("branch-{ix}")))
                    .inset(true)
                    .spacing(ListItemSpacing::Sparse)
                    .toggle_state(selected)
                    .disabled(is_disabled)
                    .start_slot(Icon::new(IconName::GitBranch).color(icon_color))
                    .child(
                        HighlightedLabel::new(branch.name().to_string(), positions.clone())
                            .color(label_color)
                            .truncate(),
                    );

                Some(if let Some(reason) = disabled_reason.clone() {
                    item.tooltip(Tooltip::text(reason))
                } else if branch.is_remote() {
                    item.tooltip(Tooltip::text(
                        "Create a new local branch from this remote branch",
                    ))
                } else {
                    item.tooltip(Tooltip::text(branch.name().to_string()))
                })
            }
            ThreadBranchEntry::CreateNamed { name } => Some(
                ListItem::new("create-named-branch")
                    .inset(true)
                    .spacing(ListItemSpacing::Sparse)
                    .toggle_state(selected)
                    .start_slot(Icon::new(IconName::Plus).color(Color::Accent))
                    .child(Label::new(format!("Create Branch: \"{name}\"…"))),
            ),
        }
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        None
    }
}
