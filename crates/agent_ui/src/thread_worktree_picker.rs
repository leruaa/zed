use std::path::PathBuf;
use std::sync::Arc;

use util::ResultExt as _;

use fuzzy::StringMatchCandidate;
use git::repository::Worktree as GitWorktree;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Task, Window, rems,
};
use picker::{Picker, PickerDelegate, PickerEditorPosition};
use project::Project;
use ui::{
    HighlightedLabel, Icon, IconName, Label, LabelCommon, ListItem, ListItemSpacing, prelude::*,
};

use crate::StartThreadIn;

pub(crate) struct ThreadWorktreePicker {
    picker: Entity<Picker<ThreadWorktreePickerDelegate>>,
    focus_handle: FocusHandle,
    _subscription: gpui::Subscription,
}

impl ThreadWorktreePicker {
    pub fn new(
        project: Entity<Project>,
        current_target: &StartThreadIn,
        has_git_repo: bool,
        is_via_collab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project_worktree_paths: Vec<PathBuf> = project
            .read(cx)
            .visible_worktrees(cx)
            .map(|wt| wt.read(cx).abs_path().to_path_buf())
            .collect();

        let repository = project.read(cx).active_repository(cx);

        let worktrees_request = repository.map(|repo| repo.update(cx, |repo, _| repo.worktrees()));

        let (preserved_branch_name, preserved_start_point) = match current_target {
            StartThreadIn::NewWorktree {
                branch_name,
                start_point,
                ..
            } => (branch_name.clone(), start_point.clone()),
            _ => (None, None),
        };

        let delegate = ThreadWorktreePickerDelegate {
            matches: vec![
                ThreadWorktreeEntry::CurrentWorktree,
                ThreadWorktreeEntry::NewWorktree,
            ],
            all_worktrees: None,
            project_worktree_paths,
            selected_index: match current_target {
                StartThreadIn::LocalProject => 0,
                StartThreadIn::NewWorktree { .. } => 1,
                _ => 0,
            },
            preserved_branch_name,
            preserved_start_point,
            has_git_repo,
            is_via_collab,
        };

        let picker = cx.new(|cx| {
            Picker::list(delegate, window, cx)
                .list_measure_all()
                .modal(false)
                .max_height(Some(rems(20.).into()))
        });

        let focus_handle = picker.focus_handle(cx);

        // Fetch worktrees asynchronously
        if let Some(worktrees_request) = worktrees_request {
            let picker_handle = picker.downgrade();
            cx.spawn_in(window, async move |_this, cx| {
                let all_worktrees: Vec<_> = worktrees_request
                    .await??
                    .into_iter()
                    .filter(|wt| wt.ref_name.is_some())
                    .collect();

                picker_handle.update_in(cx, |picker, window, cx| {
                    picker.delegate.all_worktrees = Some(all_worktrees);
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

impl Focusable for ThreadWorktreePicker {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for ThreadWorktreePicker {}

impl Render for ThreadWorktreePicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(rems(20.))
            .elevation_3(cx)
            .child(self.picker.clone())
            .on_mouse_down_out(cx.listener(|_, _, _, cx| {
                cx.emit(DismissEvent);
            }))
    }
}

#[derive(Clone)]
enum ThreadWorktreeEntry {
    CurrentWorktree,
    NewWorktree,
    LinkedWorktree {
        worktree: GitWorktree,
        positions: Vec<usize>,
    },
    CreateNamed {
        name: String,
    },
}

pub(crate) struct ThreadWorktreePickerDelegate {
    matches: Vec<ThreadWorktreeEntry>,
    all_worktrees: Option<Vec<GitWorktree>>,
    project_worktree_paths: Vec<PathBuf>,
    selected_index: usize,
    preserved_branch_name: Option<String>,
    preserved_start_point: Option<String>,
    has_git_repo: bool,
    is_via_collab: bool,
}

impl PickerDelegate for ThreadWorktreePickerDelegate {
    type ListItem = ListItem;

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Search or create worktrees…".into()
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

    fn separators_after_indices(&self) -> Vec<usize> {
        if self.matches.len() > 2 {
            vec![1]
        } else {
            Vec::new()
        }
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let Some(all_worktrees) = self.all_worktrees.clone() else {
            // Worktrees not loaded yet, keep showing the fixed items
            self.matches = vec![
                ThreadWorktreeEntry::CurrentWorktree,
                ThreadWorktreeEntry::NewWorktree,
            ];
            return Task::ready(());
        };

        // Filter out worktrees that belong to the project (those are "Current Worktree")
        let linked_worktrees: Vec<_> = all_worktrees
            .into_iter()
            .filter(|wt| {
                !self
                    .project_worktree_paths
                    .iter()
                    .any(|project_path| project_path == &wt.path)
            })
            .collect();

        let mut matches = vec![
            ThreadWorktreeEntry::CurrentWorktree,
            ThreadWorktreeEntry::NewWorktree,
        ];

        if query.is_empty() {
            for worktree in &linked_worktrees {
                matches.push(ThreadWorktreeEntry::LinkedWorktree {
                    worktree: worktree.clone(),
                    positions: Vec::new(),
                });
            }
        } else {
            let candidates: Vec<_> = linked_worktrees
                .iter()
                .enumerate()
                .map(|(ix, wt)| StringMatchCandidate::new(ix, wt.display_name()))
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

            // Use a foreground spawn to await the background work
            let linked_worktrees_clone = linked_worktrees.clone();
            return cx.spawn_in(_window, async move |picker, cx| {
                let fuzzy_matches = task.await;

                picker
                    .update_in(cx, |picker, _window, cx| {
                        let mut new_matches = vec![
                            ThreadWorktreeEntry::CurrentWorktree,
                            ThreadWorktreeEntry::NewWorktree,
                        ];

                        for candidate in &fuzzy_matches {
                            new_matches.push(ThreadWorktreeEntry::LinkedWorktree {
                                worktree: linked_worktrees_clone[candidate.candidate_id].clone(),
                                positions: candidate.positions.clone(),
                            });
                        }

                        // If query doesn't exactly match the top result, offer to create a new worktree
                        let has_exact_match = fuzzy_matches.first().is_some_and(|m| {
                            linked_worktrees_clone[m.candidate_id].display_name() == query
                        });

                        if !query.is_empty() && !has_exact_match {
                            let name = query.replace(' ', "-");
                            new_matches.push(ThreadWorktreeEntry::CreateNamed { name });
                        }

                        picker.delegate.matches = new_matches;

                        // Select the first linked worktree match if available
                        if picker.delegate.matches.len() > 2 {
                            picker.delegate.selected_index = 2;
                        } else {
                            picker.delegate.selected_index = 0;
                        }

                        cx.notify();
                    })
                    .log_err();
            });
        }

        // For empty query, also add CreateNamed if there are no linked worktrees
        // (no need in this case since we show all worktrees)

        self.matches = matches;
        if self.selected_index >= self.matches.len() {
            self.selected_index = 0;
        }

        Task::ready(())
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(entry) = self.matches.get(self.selected_index) else {
            return;
        };

        match entry {
            ThreadWorktreeEntry::CurrentWorktree => {
                window.dispatch_action(Box::new(StartThreadIn::LocalProject), cx);
            }
            ThreadWorktreeEntry::NewWorktree => {
                window.dispatch_action(
                    Box::new(StartThreadIn::NewWorktree {
                        worktree_name: None,
                        branch_name: self.preserved_branch_name.clone(),
                        start_point: self.preserved_start_point.clone(),
                    }),
                    cx,
                );
            }
            ThreadWorktreeEntry::LinkedWorktree { worktree, .. } => {
                window.dispatch_action(
                    Box::new(StartThreadIn::LinkedWorktree {
                        path: worktree.path.clone(),
                        display_name: worktree.display_name().to_string(),
                    }),
                    cx,
                );
            }
            ThreadWorktreeEntry::CreateNamed { name } => {
                window.dispatch_action(
                    Box::new(StartThreadIn::NewWorktree {
                        worktree_name: Some(name.clone()),
                        branch_name: self.preserved_branch_name.clone(),
                        start_point: self.preserved_start_point.clone(),
                    }),
                    cx,
                );
            }
        }

        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _window: &mut Window, _cx: &mut Context<Picker<Self>>) {}

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let entry = self.matches.get(ix)?;
        let is_new_worktree_disabled = !self.has_git_repo || self.is_via_collab;

        match entry {
            ThreadWorktreeEntry::CurrentWorktree => Some(
                ListItem::new("current-worktree")
                    .inset(true)
                    .spacing(ListItemSpacing::Sparse)
                    .toggle_state(selected)
                    .start_slot(Icon::new(IconName::Folder).color(Color::Muted))
                    .child(Label::new("Current Worktree")),
            ),
            ThreadWorktreeEntry::NewWorktree => Some(
                ListItem::new("new-worktree")
                    .inset(true)
                    .spacing(ListItemSpacing::Sparse)
                    .toggle_state(selected)
                    .disabled(is_new_worktree_disabled)
                    .start_slot(
                        Icon::new(IconName::Plus).color(if is_new_worktree_disabled {
                            Color::Disabled
                        } else {
                            Color::Muted
                        }),
                    )
                    .child(
                        Label::new("New Git Worktree").color(if is_new_worktree_disabled {
                            Color::Disabled
                        } else {
                            Color::Default
                        }),
                    ),
            ),
            ThreadWorktreeEntry::LinkedWorktree {
                worktree,
                positions,
            } => {
                let display_name = worktree.display_name();
                let first_line = display_name.lines().next().unwrap_or(display_name);
                let positions: Vec<_> = positions
                    .iter()
                    .copied()
                    .filter(|&pos| pos < first_line.len())
                    .collect();

                Some(
                    ListItem::new(SharedString::from(format!("linked-worktree-{ix}")))
                        .inset(true)
                        .spacing(ListItemSpacing::Sparse)
                        .toggle_state(selected)
                        .start_slot(Icon::new(IconName::GitWorktree).color(Color::Muted))
                        .child(HighlightedLabel::new(first_line.to_owned(), positions).truncate()),
                )
            }
            ThreadWorktreeEntry::CreateNamed { name } => Some(
                ListItem::new("create-named-worktree")
                    .inset(true)
                    .spacing(ListItemSpacing::Sparse)
                    .toggle_state(selected)
                    .start_slot(Icon::new(IconName::Plus).color(Color::Accent))
                    .child(Label::new(format!("Create Worktree: \"{name}\"…"))),
            ),
        }
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        None
    }
}
