//! # Undo Manager
//!
//! ## Operations and Results
//!
//! Undo and Redo actions execute an operation against the filesystem, producing
//! a result that is recorded back into the history in place of the original
//! entry. Each result is the semantic inverse of its paired operation, so the
//! cycle can repeat for continued undo and redo.
//!
//!  Operations                            Results
//!  ─────────────────────────────────  ──────────────────────────────────────
//!  Create(ProjectPath)               →  Created(ProjectPath)
//!  Trash(ProjectPath)                →  Trashed(TrashedEntry)
//!  Rename(ProjectPath, ProjectPath)  →  Renamed(ProjectPath, ProjectPath)
//!  Restore(TrashedEntry)             →  Restored(ProjectPath)
//!  Batch(Vec<Operation>)             →  Batch(Vec<Result>)
//!
//!
//! ## History and Cursor
//!
//! The undo manager maintains an operation history with a cursor position (↑).
//! Recording an operation appends it to the history and advances the cursor to
//! the end. The cursor separates past entries (left of ↑) from future entries
//! (right of ↑).
//!
//! ─ **Undo**: Takes the history entry just *before* ↑, executes its inverse,
//!   records the result back in its place, and moves ↑ one step to the left.
//! ─ **Redo**: Takes the history entry just *at* ↑, executes its inverse,
//!   records the result back in its place, and advances ↑ one step to the right.
//!
//!
//! ## Example
//!
//! User Operation  Create(src/main.rs)
//! History
//! 	0 Created(src/main.rs)
//!     1 +++cursor+++
//!
//! User Operation  Rename(README.md, readme.md)
//! History
//! 	0 Created(src/main.rs)
//! 	1 Renamed(README.md, readme.md)
//!     2 +++cursor+++
//!
//! User Operation  Create(CONTRIBUTING.md)
//! History
//! 	0 Created(src/main.rs)
//!     1 Renamed(README.md, readme.md)
//! 	2 Created(CONTRIBUTING.md) ──┐
//!     3 +++cursor+++               │(before the cursor)
//!                                  │
//!   ┌──────────────────────────────┴─────────────────────────────────────────────┐
//!     Redoing will take the result at the cursor position, convert that into the
//!     operation that can revert that result, execute that operation and replace
//!     the result in the history with the new result, obtained from running the
//!     inverse operation, advancing the cursor position.
//!   └──────────────────────────────┬─────────────────────────────────────────────┘
//!                                  │
//!                                  │
//! User Operation  Undo             v
//! Execute         Created(CONTRIBUTING.md) ────────> Trash(CONTRIBUTING.md)
//! Record          Trashed(TrashedEntry(1))
//! History
//! 	0 Created(src/main.rs)
//! 	1 Renamed(README.md, readme.md) ─┐
//!     2 +++cursor+++                   │(before the cursor)
//! 	2 Trashed(TrashedEntry(1))       │
//!                                      │
//! User Operation  Undo                 v
//! Execute         Renamed(README.md, readme.md) ───> Rename(readme.md, README.md)
//! Record          Renamed(readme.md, README.md)
//! History
//! 	0 Created(src/main.rs)
//!     1 +++cursor+++
//! 	1 Renamed(readme.md, README.md) ─┐ (at the cursor)
//! 	2 Trashed(TrashedEntry(1))       │
//!                                      │
//!   ┌──────────────────────────────────┴─────────────────────────────────────────┐
//!     Redoing will take the result at the cursor position, convert that into the
//!     operation that can revert that result, execute that operation and replace
//!     the result in the history with the new result, obtained from running the
//!     inverse operation, advancing the cursor position.
//!   └──────────────────────────────────┬─────────────────────────────────────────┘
//!                                      │
//!                                      │
//! User Operation  Redo                 v
//! Execute         Renamed(readme.md, README.md) ───> Rename(README.md, readme.md)
//! Record          Renamed(README.md, readme.md)
//! History
//! 	0 Created(src/main.rs)
//! 	1 Renamed(README.md, readme.md)
//!     2 +++cursor+++
//! 	2 Trashed(TrashedEntry(1))────┐ (at the cursor)
//!                                   │
//! User Operation  Redo              v
//! Execute         Trashed(TrashedEntry(1)) ────────> Restore(TrashedEntry(1))
//! Record          Restored(ProjectPath)
//! History
//! 	0 Created(src/main.rs)
//! 	1 Renamed(README.md, readme.md)
//! 	2 Restored(ProjectPath)
//!     2 +++cursor+++

use crate::ProjectPanel;
use anyhow::{Result, anyhow};
use fs::TrashedEntry;
use gpui::{AppContext, SharedString, Task, WeakEntity};
use project::{ProjectPath, WorktreeId};
use std::collections::VecDeque;
use ui::App;
use workspace::{
    Workspace,
    notifications::{NotificationId, simple_message_notification::MessageNotification},
};
use worktree::CreatedEntry;

enum Operation {
    Create(ProjectPath),
    Trash(ProjectPath),
    Rename(ProjectPath, ProjectPath),
    Restore(WorktreeId, TrashedEntry),
    Batch(Vec<Operation>),
}

impl Operation {
    async fn execute(self, undo_manager: &UndoManager, cx: &mut App) -> Result<Change> {
        Ok(match self {
            Operation::Create(project_path) => {
                undo_manager.create(&project_path, cx).await?;
                Change::Created(project_path)
            }
            Operation::Trash(project_path) => {
                let trash_entry = undo_manager.trash(&project_path, cx).await?;
                Change::Trashed(project_path.worktree_id, trash_entry)
            }
            Operation::Rename(from, to) => {
                undo_manager.rename(&from, &to, cx).await?;
                Change::Renamed(from, to)
            }
            Operation::Restore(worktree_id, trashed_entry) => {
                let project_path = undo_manager.restore(worktree_id, trashed_entry, cx).await?;
                Change::Restored(project_path)
            }
            Operation::Batch(operations) => {
                let mut res = Vec::new();
                for op in operations {
                    res.push(Box::pin(op.execute(undo_manager, cx)).await?);
                }
                Change::Batched(res)
            }
        })
    }
}

#[derive(Clone)]
pub(crate) enum Change {
    Created(ProjectPath),
    Trashed(WorktreeId, TrashedEntry),
    Renamed(ProjectPath, ProjectPath),
    Restored(ProjectPath),
    Batched(Vec<Change>),
}

impl Change {
    fn to_inverse(self) -> Operation {
        match self {
            Change::Created(project_path) => Operation::Trash(project_path),
            Change::Trashed(worktree_id, trashed_entry) => {
                Operation::Restore(worktree_id, trashed_entry)
            }
            Change::Renamed(from, to) => Operation::Rename(to, from),
            Change::Restored(project_path) => Operation::Trash(project_path),
            // When inverting a batch of operations, we reverse the order of
            // operations to handle dependencies between them. For example, if a
            // batch contains the following order of operations:
            //
            // 1. Create `src/`
            // 2. Create `src/main.rs`
            //
            // If we first tried to revert the directory creation, it would fail
            // because there's still files inside the directory.
            Change::Batched(changes) => {
                Operation::Batch(changes.into_iter().rev().map(Change::to_inverse).collect())
            }
        }
    }
}

// Imagine pressing undo 10000+ times?!
const MAX_UNDO_OPERATIONS: usize = 10_000;

pub struct UndoManager {
    workspace: WeakEntity<Workspace>,
    panel: WeakEntity<ProjectPanel>,
    history: VecDeque<Change>,
    cursor: usize,
    /// Maximum number of operations to keep on the undo history.
    limit: usize,
}

impl UndoManager {
    pub fn new(workspace: WeakEntity<Workspace>, panel: WeakEntity<ProjectPanel>) -> Self {
        Self::new_with_limit(workspace, panel, MAX_UNDO_OPERATIONS)
    }

    pub fn new_with_limit(
        workspace: WeakEntity<Workspace>,
        panel: WeakEntity<ProjectPanel>,
        limit: usize,
    ) -> Self {
        Self {
            workspace,
            panel,
            history: VecDeque::new(),
            cursor: 0usize,
            limit,
        }
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.history.len()
    }

    pub async fn undo(&mut self, cx: &mut App) -> Result<()> {
        if !self.can_undo() {
            return Ok(());
        }

        // Undo failure:
        //
        // History
        // 	0 Created(src/main.rs)
        // 	1 Renamed(README.md, readme.md) ─┐
        //     2 +++cursor+++                │(before the cursor)
        // 	2 Trashed(TrashedEntry(1))       │
        //                                   │
        // User Operation  Undo              v
        // Failed execute  Renamed(README.md, readme.md) ───> Rename(readme.md, README.md)
        // Record nothing
        // History
        // 	0 Created(src/main.rs)
        //     1 +++cursor+++
        // 	1 Trashed(TrashedEntry(1)) -----
        //                                  |(at the cursor)
        // User Operation  Redo             v
        // Execute         Trashed(TrashedEntry(1)) ────────> Restore(TrashedEntry(1))
        // Record          Restored(ProjectPath)
        // History
        // 	0 Created(src/main.rs)
        // 	1 Restored(ProjectPath)
        //  1 +++cursor+++

        // We always want to move the cursor back regardless of whether undoing
        // suceeds or fails, otherwise the cursor could end up pointing to a
        // position outside of the history, as we remove the change before the
        // cursor, in case undo fails.
        let before_cursor = self.cursor - 1; // see docs above
        self.cursor -= 1; // take a step back into the past

        let change_before_the_cursor = self
            .history
            .remove(before_cursor)
            .expect("we can undo")
            .clone();
        // If this fails we can not redo/undo this change so it needs to
        // be gone from history, thats why we just removed it above! :)
        let change_created_by_undoing = change_before_the_cursor
            .to_inverse()
            .execute(self, cx)
            .await?;
        self.history
            .insert(before_cursor, change_created_by_undoing);
        Ok(())
    }

    pub async fn redo(&mut self, cx: &mut App) -> Result<()> {
        if !self.can_redo() {
            return Ok(());
        }

        let change_at_the_cursor = self
            .history
            .remove(self.cursor)
            .expect("we can redo")
            .clone();
        let redo_change = change_at_the_cursor.to_inverse().execute(self, cx).await?;
        self.history.insert(self.cursor, redo_change);
        self.cursor += 1;
        Ok(())
    }

    /// Passed in changes will always be performed as a single step
    pub fn record(&mut self, changes: impl IntoIterator<Item = Change>) {
        let mut changes = changes.into_iter();
        let Some(first) = changes.by_ref().next() else {
            return;
        };

        let change = if let Some(second) = changes.by_ref().next() {
            Change::Batched([first].into_iter().chain([second]).chain(changes).collect())
        } else {
            first
        };

        // When recording a new change, discard any changes that could still be
        // redone.
        if self.cursor < self.history.len() {
            self.history.drain(self.cursor..);
        }

        // Ensure that the number of recorded changes does not exceed the
        // maximum amount of tracked changes.
        if self.history.len() >= self.limit {
            self.history.pop_front();
        }

        self.history.push_back(change);
    }

    async fn rename(
        &self,
        from: &ProjectPath,
        to: &ProjectPath,
        cx: &mut App,
    ) -> Result<CreatedEntry> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Err(anyhow!("Failed to obtain workspace."));
        };

        let res: Result<Task<Result<CreatedEntry>>> = workspace.update(cx, |workspace, cx| {
            workspace.project().update(cx, |project, cx| {
                let entry_id = project
                    .entry_for_path(from, cx)
                    .map(|entry| entry.id)
                    .ok_or_else(|| anyhow!("No entry for path."))?;

                Ok(project.rename_entry(entry_id, to.clone(), cx))
            })
        });

        res?.await
    }

    async fn create(&self, project_path: &ProjectPath, cx: &mut App) -> Result<CreatedEntry> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Err(anyhow!("Failed to obtain workspace."));
        };

        workspace
            .update(cx, |workspace, cx| {
                workspace.project().update(cx, |project, cx| {
                    // This should not be hardcoded to `false`, as it can genuinely
                    // be a directory and it misses all the nuances and details from
                    // `ProjectPanel::confirm_edit`. However, we expect this to be a
                    // short-lived solution as we add support for restoring trashed
                    // files, at which point we'll no longer need to `Create` new
                    // files, any redoing of a trash operation should be a restore.
                    let is_directory = false;
                    project.create_entry(project_path.clone(), is_directory, cx)
                })
            })
            .await
    }

    async fn trash(&self, project_path: &ProjectPath, cx: &mut App) -> Result<TrashedEntry> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Err(anyhow!("Failed to obtain workspace."));
        };

        workspace
            .update(cx, |workspace, cx| {
                workspace.project().update(cx, |project, cx| {
                    let entry_id = project
                        .entry_for_path(&project_path, cx)
                        .map(|entry| entry.id)
                        .ok_or_else(|| anyhow!("No entry for path."))?;

                    project
                        .delete_entry(entry_id, true, cx)
                        .ok_or_else(|| anyhow!("Worktree entry should exist"))
                })
            })?
            .await
            .and_then(|entry| {
                entry.ok_or_else(|| anyhow!("When trashing we should always get a trashentry"))
            })
    }

    async fn restore(
        &self,
        worktree_id: WorktreeId,
        trashed_entry: TrashedEntry,
        cx: &mut App,
    ) -> Result<ProjectPath> {
        let Some(workspace) = self.workspace.upgrade() else {
            return Err(anyhow!("Failed to obtain workspace."));
        };

        workspace
            .update(cx, |workspace, cx| {
                workspace.project().update(cx, |project, cx| {
                    project.restore_entry(worktree_id, trashed_entry, cx)
                })
            })
            .await
    }

    /// Displays a notification with the provided `title` and `error`.
    fn show_error(
        title: impl Into<SharedString>,
        workspace: WeakEntity<Workspace>,
        error: SharedString,
        cx: &mut App,
    ) {
        workspace
            .update(cx, move |workspace, cx| {
                let notification_id =
                    NotificationId::Named(SharedString::new_static("project_panel_undo"));

                workspace.show_notification(notification_id, cx, move |cx| {
                    cx.new(|cx| MessageNotification::new(error.to_string(), cx).with_title(title))
                })
            })
            .ok();
    }
}

// #[cfg(test)]
// pub(crate) mod tests {
//     use crate::{ProjectPanel, project_panel_tests, undo::UndoManager};
//     use gpui::{Entity, TestAppContext, VisualTestContext, WindowHandle};
//     use project::{FakeFs, Project, ProjectPath, WorktreeId};
//     use serde_json::{Value, json};
//     use std::sync::Arc;
//     use util::rel_path::rel_path;
//     use workspace::MultiWorkspace;

//     struct TestContext {
//         project: Entity<Project>,
//         panel: Entity<ProjectPanel>,
//         window: WindowHandle<MultiWorkspace>,
//     }

//     async fn init_test(cx: &mut TestAppContext, tree: Option<Value>) -> TestContext {
//         project_panel_tests::init_test(cx);

//         let fs = FakeFs::new(cx.executor());
//         if let Some(tree) = tree {
//             fs.insert_tree("/root", tree).await;
//         }
//         let project = Project::test(fs.clone(), ["/root".as_ref()], cx).await;
//         let window =
//             cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
//         let workspace = window
//             .read_with(cx, |mw, _| mw.workspace().clone())
//             .unwrap();
//         let cx = &mut VisualTestContext::from_window(window.into(), cx);
//         let panel = workspace.update_in(cx, ProjectPanel::new);
//         cx.run_until_parked();

//         TestContext {
//             project,
//             panel,
//             window,
//         }
//     }

//     pub(crate) fn build_create_operation(
//         worktree_id: WorktreeId,
//         file_name: &str,
//     ) -> ProjectPanelOperation {
//         ProjectPanelOperation::Create(ProjectPath {
//             path: Arc::from(rel_path(file_name)),
//             worktree_id,
//         })
//     }

//     pub(crate) fn build_trash_operation(
//         worktree_id: WorktreeId,
//         file_name: &str,
//     ) -> ProjectPanelOperation {
//         ProjectPanelOperation::Trash(ProjectPath {
//             path: Arc::from(rel_path(file_name)),
//             worktree_id,
//         })
//     }

//     pub(crate) fn build_rename_operation(
//         worktree_id: WorktreeId,
//         from: &str,
//         to: &str,
//     ) -> ProjectPanelOperation {
//         let from_path = Arc::from(rel_path(from));
//         let to_path = Arc::from(rel_path(to));

//         ProjectPanelOperation::Rename(
//             ProjectPath {
//                 worktree_id,
//                 path: from_path,
//             },
//             ProjectPath {
//                 worktree_id,
//                 path: to_path,
//             },
//         )
//     }

//     async fn rename(
//         panel: &Entity<ProjectPanel>,
//         from: &str,
//         to: &str,
//         cx: &mut VisualTestContext,
//     ) {
//         project_panel_tests::select_path(panel, from, cx);
//         panel.update_in(cx, |panel, window, cx| {
//             panel.rename(&Default::default(), window, cx)
//         });
//         cx.run_until_parked();

//         panel
//             .update_in(cx, |panel, window, cx| {
//                 panel
//                     .filename_editor
//                     .update(cx, |editor, cx| editor.set_text(to, window, cx));
//                 panel.confirm_edit(true, window, cx).unwrap()
//             })
//             .await
//             .unwrap();
//         cx.run_until_parked();
//     }

//     #[gpui::test]
//     async fn test_limit(cx: &mut TestAppContext) {
//         let test_context = init_test(cx, None).await;
//         let worktree_id = test_context.project.update(cx, |project, cx| {
//             project.visible_worktrees(cx).next().unwrap().read(cx).id()
//         });

//         // Since we're updating the `ProjectPanel`'s undo manager with one whose
//         // limit is 3 operations, we only need to create 4 operations which
//         // we'll record, in order to confirm that the oldest operation is
//         // evicted.
//         let operation_a = build_create_operation(worktree_id, "file_a.txt");
//         let operation_b = build_create_operation(worktree_id, "file_b.txt");
//         let operation_c = build_create_operation(worktree_id, "file_c.txt");
//         let operation_d = build_create_operation(worktree_id, "file_d.txt");

//         test_context.panel.update(cx, move |panel, cx| {
//             panel.undo_manager =
//                 UndoManager::new_with_limit(panel.workspace.clone(), cx.weak_entity(), 3);
//             panel.undo_manager.record(operation_a);
//             panel.undo_manager.record(operation_b);
//             panel.undo_manager.record(operation_c);
//             panel.undo_manager.record(operation_d);

//             assert_eq!(panel.undo_manager.undo_stack.len(), 3);
//         });
//     }
//     #[gpui::test]
//     async fn test_undo_redo_stacks(cx: &mut TestAppContext) {
//         let TestContext {
//             window,
//             panel,
//             project,
//             ..
//         } = init_test(
//             cx,
//             Some(json!({
//                 "a.txt": "",
//                 "b.txt": ""
//             })),
//         )
//         .await;
//         let worktree_id = project.update(cx, |project, cx| {
//             project.visible_worktrees(cx).next().unwrap().read(cx).id()
//         });
//         let cx = &mut VisualTestContext::from_window(window.into(), cx);

//         // Start by renaming `src/file_a.txt` to `src/file_1.txt` and asserting
//         // we get the correct inverse operation in the
//         // `UndoManager::undo_stackand asserting we get the correct inverse
//         // operation in the `UndoManager::undo_stack`.
//         rename(&panel, "root/a.txt", "1.txt", cx).await;
//         panel.update(cx, |panel, _cx| {
//             assert_eq!(
//                 panel.undo_manager.undo_stack,
//                 vec![build_rename_operation(worktree_id, "1.txt", "a.txt")]
//             );
//             assert!(panel.undo_manager.redo_stack.is_empty());
//         });

//         // After undoing, the operation to be executed should be popped from
//         // `UndoManager::undo_stack` and its inverse operation pushed to
//         // `UndoManager::redo_stack`.
//         panel.update_in(cx, |panel, window, cx| {
//             panel.undo(&Default::default(), window, cx);
//         });
//         cx.run_until_parked();

//         panel.update(cx, |panel, _cx| {
//             assert!(panel.undo_manager.undo_stack.is_empty());
//             assert_eq!(
//                 panel.undo_manager.redo_stack,
//                 vec![build_rename_operation(worktree_id, "a.txt", "1.txt")]
//             );
//         });

//         // Redoing should have the same effect as undoing, but in reverse.
//         panel.update_in(cx, |panel, window, cx| {
//             panel.redo(&Default::default(), window, cx);
//         });
//         cx.run_until_parked();

//         panel.update(cx, |panel, _cx| {
//             assert_eq!(
//                 panel.undo_manager.undo_stack,
//                 vec![build_rename_operation(worktree_id, "1.txt", "a.txt")]
//             );
//             assert!(panel.undo_manager.redo_stack.is_empty());
//         });
//     }

//     #[gpui::test]
//     async fn test_undo_redo_trash(cx: &mut TestAppContext) {
//         let TestContext {
//             window,
//             panel,
//             project,
//             ..
//         } = init_test(
//             cx,
//             Some(json!({
//                 "a.txt": "",
//                 "b.txt": ""
//             })),
//         )
//         .await;
//         let worktree_id = project.update(cx, |project, cx| {
//             project.visible_worktrees(cx).next().unwrap().read(cx).id()
//         });
//         let cx = &mut VisualTestContext::from_window(window.into(), cx);

//         // Start by setting up the `UndoManager::undo_stack` such that, undoing
//         // the last user operation will trash `a.txt`.
//         panel.update(cx, |panel, _cx| {
//             panel
//                 .undo_manager
//                 .undo_stack
//                 .push_back(build_trash_operation(worktree_id, "a.txt"));
//         });

//         // Undoing should now delete the file and update the
//         // `UndoManager::redo_stack` state with a new `Create` operation.
//         panel.update_in(cx, |panel, window, cx| {
//             panel.undo(&Default::default(), window, cx);
//         });
//         cx.run_until_parked();

//         panel.update(cx, |panel, _cx| {
//             assert!(panel.undo_manager.undo_stack.is_empty());
//             assert_eq!(
//                 panel.undo_manager.redo_stack,
//                 vec![build_create_operation(worktree_id, "a.txt")]
//             );
//         });

//         // Redoing should create the file again and pop the operation from
//         // `UndoManager::redo_stack`.
//         panel.update_in(cx, |panel, window, cx| {
//             panel.redo(&Default::default(), window, cx);
//         });
//         cx.run_until_parked();

//         panel.update(cx, |panel, _cx| {
//             assert_eq!(
//                 panel.undo_manager.undo_stack,
//                 vec![build_trash_operation(worktree_id, "a.txt")]
//             );
//             assert!(panel.undo_manager.redo_stack.is_empty());
//         });
//     }

//     #[gpui::test]
//     async fn test_undo_redo_batch(cx: &mut TestAppContext) {
//         let TestContext {
//             window,
//             panel,
//             project,
//             ..
//         } = init_test(
//             cx,
//             Some(json!({
//                 "a.txt": "",
//                 "b.txt": ""
//             })),
//         )
//         .await;
//         let worktree_id = project.update(cx, |project, cx| {
//             project.visible_worktrees(cx).next().unwrap().read(cx).id()
//         });
//         let cx = &mut VisualTestContext::from_window(window.into(), cx);

//         // There's currently no way to trigger two file renames in a single
//         // operation using the `ProjectPanel`. As such, we'll directly record
//         // the batch of operations in `UndoManager`, simulating that `1.txt` and
//         // `2.txt` had been renamed to `a.txt` and `b.txt`, respectively.
//         panel.update(cx, |panel, _cx| {
//             panel.undo_manager.record_batch(vec![
//                 build_rename_operation(worktree_id, "1.txt", "a.txt"),
//                 build_rename_operation(worktree_id, "2.txt", "b.txt"),
//             ]);

//             assert_eq!(
//                 panel.undo_manager.undo_stack,
//                 vec![ProjectPanelOperation::Batch(vec![
//                     build_rename_operation(worktree_id, "b.txt", "2.txt"),
//                     build_rename_operation(worktree_id, "a.txt", "1.txt"),
//                 ])]
//             );
//             assert!(panel.undo_manager.redo_stack.is_empty());
//         });

//         panel.update_in(cx, |panel, window, cx| {
//             panel.undo(&Default::default(), window, cx);
//         });
//         cx.run_until_parked();

//         // Since the operations in the `Batch` are meant to be done in order,
//         // the inverse should have the operations in the opposite order to avoid
//         // dependencies. For example, creating a `src/` folder come before
//         // creating the `src/file_a.txt` file, but when undoing, the file should
//         // be trashed first.
//         panel.update(cx, |panel, _cx| {
//             assert!(panel.undo_manager.undo_stack.is_empty());
//             assert_eq!(
//                 panel.undo_manager.redo_stack,
//                 vec![ProjectPanelOperation::Batch(vec![
//                     build_rename_operation(worktree_id, "1.txt", "a.txt"),
//                     build_rename_operation(worktree_id, "2.txt", "b.txt"),
//                 ])]
//             );
//         });

//         panel.update_in(cx, |panel, window, cx| {
//             panel.redo(&Default::default(), window, cx);
//         });
//         cx.run_until_parked();

//         panel.update(cx, |panel, _cx| {
//             assert_eq!(
//                 panel.undo_manager.undo_stack,
//                 vec![ProjectPanelOperation::Batch(vec![
//                     build_rename_operation(worktree_id, "b.txt", "2.txt"),
//                     build_rename_operation(worktree_id, "a.txt", "1.txt"),
//                 ])]
//             );
//             assert!(panel.undo_manager.redo_stack.is_empty());
//         });
//     }
// }
