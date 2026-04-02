#![cfg(test)]

use std::path::PathBuf;

use fs::FakeFs;
use gpui::{Entity, VisualTestContext, WindowHandle};
use project::Project;
use serde_json::{Value, json};
use util::path;
use workspace::MultiWorkspace;
use std::sync::Arc;

use crate::project_panel_tests::{self, find_project_entry, select_path};
use crate::{NewDirectory, NewFile, PROJECT_PANEL_KEY, ProjectPanel, Redo, Rename, Undo};

struct TestContext {
    project: Entity<Project>,
    panel: Entity<ProjectPanel>,
    window: WindowHandle<MultiWorkspace>,
    fs: Arc<FakeFs>,
    cx: VisualTestContext,
}

impl TestContext {
    async fn undo(&mut self) {
        self.panel.update_in(&mut self.cx, |panel, window, cx| {
            panel.undo(&Undo, window, cx);
        });
        self.cx.run_until_parked();
    }
    async fn redo(&mut self) {
        self.panel.update_in(&mut self.cx, |panel, window, cx| {
            panel.redo(&Redo, window, cx);
        });
        self.cx.run_until_parked();
    }

    fn assert_fs_state_is(&mut self, state: &[&str]) {
        // Y WINDOWS?! :(
        path!("root/a.txt")
        assert_eq!(self.fs.paths(), state)
    }

    fn assert_exists(&mut self, path: &str) {
        assert!(
            find_project_entry(&self.panel, &format!("root/{path}"), &mut self.cx).is_some(),
            "{path} should exist"
        );
    }

    fn assert_not_exists(&mut self, path: &str) {
        assert_eq!(
            find_project_entry(&self.panel, &format!("root/{path}"), &mut self.cx),
            None,
            "{path} should not exist"
        );
    }

    async fn rename(&mut self, from: &str, to: &str) {
        let from = format!("root/{from}");
        let to = format!("root/{to}");
        let Self { panel, cx, .. } = self;
        select_path(&panel, &from, cx);
        panel.update_in(cx, |panel, window, cx| panel.rename(&Rename, window, cx));
        cx.run_until_parked();

        let confirm = panel.update_in(cx, |panel, window, cx| {
            panel
                .filename_editor
                .update(cx, |editor, cx| editor.set_text(to, window, cx));
            panel.confirm_edit(true, window, cx).unwrap()
        });
        confirm.await.unwrap();
        cx.run_until_parked();
    }

    async fn create_file(&mut self, path: &str) {
        let Self { panel, cx, .. } = self;
        select_path(&panel, "root", cx);
        panel.update_in(cx, |panel, window, cx| panel.new_file(&NewFile, window, cx));
        cx.run_until_parked();

        let confirm = panel.update_in(cx, |panel, window, cx| {
            panel
                .filename_editor
                .update(cx, |editor, cx| editor.set_text(path, window, cx));
            panel.confirm_edit(true, window, cx).unwrap()
        });
        confirm.await.unwrap();
        cx.run_until_parked();
    }

    async fn create_directory(&mut self, path: &str) {
        let Self { panel, cx, .. } = self;

        select_path(&panel, "root", cx);
        panel.update_in(cx, |panel, window, cx| {
            panel.new_directory(&NewDirectory, window, cx)
        });
        cx.run_until_parked();

        let confirm = panel.update_in(cx, |panel, window, cx| {
            panel
                .filename_editor
                .update(cx, |editor, cx| editor.set_text(path, window, cx));
            panel.confirm_edit(true, window, cx).unwrap()
        });
        confirm.await.unwrap();
        cx.run_until_parked();
    }

    /// Drags the `files` to the provided `directory`.
    fn drag(&mut self, files: impl IntoIterator<Item = AsRef<str>>, directory: &str) {
        self.panel.update(&mut self.cx, |panel, _| panel.marked_entries.clear());
        paths.map(|path| project_panel_tests::select_path_with_mark(&self.panel, &format!("root/{path}"), cx));
        project_panel_tests::drag_selection_to(&self.panel, &format!("root/{directory}"), false, cx);
    }

    // TODO(dino): do we want to move this here instead of calling the one in
    // `project_panel_tests`?
    fn toggle_expand_directory(&mut self, path: &str) {
        project_panel_tests::toggle_expand_dir(&self.panel, path, &mut self.cx);
    }

    /// Only supports files in root (otherwise would need toggle_expand_dir).
    /// For undo redo the paths themselves do not matter so this is fine
    async fn cut(&mut self, path: &str) {
        select_path_with_mark(&panel, &format!("root/{}"), &mut self.cx);
        self.panel.update_in(&mut self.cx, |panel, window, cx| {
            panel.cut(&Default::default(), window, cx);
        });
    }

    /// Only supports files in root (otherwise would need toggle_expand_dir).
    /// For undo redo the paths themselves do not matter so this is fine
    async fn paste(&mut self, path: &str {
        select_path(&panel, &format!("root/{}"), &mut self.cx);
        panel.update_in(&mut cx, |panel, window, cx| {
            panel.paste(&Default::default(), window, cx);
        });
        cx.run_until_parked();
    }

    const DEFAULT_TREE: serde_json::Value = json!({
        "a.txt": "",
        "b.txt": "",
    });

    /// The test tree is:
    /// ```txt
    /// a.txt
    /// b.txt
    /// x.txt
    /// ```
    /// a and b are empty, x has the text "content" inside
    async fn new(cx: &mut gpui::TestAppContext) -> TestContext {
        Self::new_with_tree(
            cx,
            DEFAULT_TREE,
        )
        .await
    }

    async fn new_with_tree(cx: &mut gpui::TestAppContext, tree: Value) -> TestContext {
        project_panel_tests::init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/root", tree).await;
        let project = Project::test(fs.clone(), ["/root".as_ref()], cx).await;
        let window =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = window
            .read_with(cx, |mw, _| mw.workspace().clone())
            .unwrap();
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let panel = workspace.update_in(&mut cx, ProjectPanel::new);
        cx.run_until_parked();

        TestContext {
            project,
            panel,
            window,
            fs,
            cx,
        }
    }
}

#[gpui::test]
async fn rename_undo_redo(cx: &mut gpui::TestAppContext) {
    let mut cx = TestContext::new(cx).await;

    cx.rename("a.txt", "renamed.txt").await;
    cx.assert_exists("renamed.txt");
    cx.assert_not_exists("a.txt.txt");

    cx.undo().await;
    cx.assert_exists("a.txt.txt");
    cx.assert_not_exists("renamed.txt");

    cx.redo().await;
    cx.assert_exists("renamed.txt");
    cx.assert_not_exists("a.txt.txt");
}

// TODO(dino): Would be nice if this test also actually confirmed that, if
// `new.txt` has some content before removal, that same content is preserved
// when restoring the file.
#[gpui::test]
async fn create_undo_redo(cx: &mut gpui::TestAppContext) {
    let mut cx = TestContext::new(cx).await;

    cx.create_file("new.txt").await;
    cx.assert_exists("new.txt");

    cx.undo().await;
    cx.assert_not_exists("new.txt");

    cx.redo().await;
    cx.assert_exists("new.txt");
}

#[gpui::test]
async fn create_dir_undo(cx: &mut gpui::TestAppContext) {
    let mut cx = TestContext::new(cx).await;

    cx.create_directory("new_dir").await;
    cx.assert_exists("new_dir");
    cx.undo().await;
    cx.assert_not_exists("new_dir");
}

#[gpui::test]
async fn cut_paste_undo(cx: &mut gpui::TestAppContext) {
    let mut cx = TestContext::new(cx).await;

    cx.cut("a.txt").await;
    cx.paste("a.txt").await;
    cx.assert_exist("a.txt");

    cx.undo().await;
    cx.assert_not_exists("a.txt");
}

#[gpui::test]
async fn drag_undo_redo(cx: &mut gpui::TestAppContext) {
    let mut cx = TestContext::new(cx).await;

    cx.create_directory("src").await;
    cx.create_file("src/a.rs").await;
    cx.toggle_expand_directory("root/src");

    cx.drag("src/a.rs", "");
    cx.assert_exists("a.rs");
    cx.assert_not_exists("src/a.rs");

    cx.undo().awit;
    cx.assert_exists("src/a.rs");
    cx.assert_not_exists("a.rs");

    cx.redo().awit;
    cx.assert_exists("a.rs");
    cx.assert_not_exists("src/a.rs");
}

#[gpui::test]
async fn drag_multiple_undo_redo(cx: &mut gpui::TestAppContext) {
    let mut cx = TestContext::new(cx).await;

    cx.create_directory("src").await;
    cx.create_file("src/a.rs").await;
    cx.create_file("src/b.rs").await;
    cx.toggle_expand_directory("root/src");

    cx.drag(["src/a.rs", "src/b.rs"], "");
    cx.assert_exists("a.rs");
    cx.assert_not_exists("src/a.rs");
    cx.assert_exists("b.rs");
    cx.assert_not_exists("src/b.rs");

    cx.undo().await;
    cx.assert_exists("src/a.rs");
    cx.assert_not_exists("a.rs");
    cx.assert_exists("src/b.rs");
    cx.assert_not_exists("b.rs");

    cx.redo().await;
    cx.assert_exists("a.rs");
    cx.assert_not_exists("src/a.rs");
    cx.assert_exists("b.rs");
    cx.assert_not_exists("src/b.rs");
}

#[gpui::test]
async fn two_sequential_undos(cx: &mut gpui::TestAppContext) {
    let mut cx = TestContext::new(cx).await;

    cx.rename("a.txt", "x.txt").await;
    cx.create("y.txt").await;

    cx.undo().await; // TODO(yara) should we have an assert fs state instead?
    cx.assert_not_exists("y.txt");
    cx.assert_exists("x.txt");

    cx.undo().await;
    cx.assert_not_exists("x.txt");
    cx.assert_exists("a.txt");
}

#[gpui::test]
async fn undo_without_history(cx: &mut gpui::TestAppContext) {
    let mut cx = TestContext::new(cx).await;

    cx.undo().await;
    cx.assert_fs_state_is(DEFAULT_TREE)
}
