//! The side panel as someone uses it, against the stand-in box (nothing real is reached, no git is
//! run): opening it beside a conversation and dragging it wider, Changes as the box groups them, a
//! file's diff, staging and discarding (asked first), committing and the push that didn't go
//! through, the folder's files read a folder at a time with one that can't be read, and branches:
//! switching, a new one, a worktree's Merge into… and a pull request.
//!
//! The views read the box at once here, as the follows would hand them what it says.

use gpui::{Bounds, Entity, Focusable as _, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext, point, px, size};
use kuvo::panel::Kind;
use kuvo::panel::changes::CommitSaid;
use kuvo::panel::files::Shown;
use kuvo::panel::{Load, branches::Said};
use kuvo::stand_in::{Replies, StandIn, git::website};
use kuvo::workspace::Workspace;
use kuvo_box::BoxClient;
use kuvo_ui::{Appearance, Theme};
use serde_json::json;

const NOW: f64 = 1_794_650_000_000.0;

fn open<'a>(cx: &'a mut TestAppContext, stand_in: StandIn, session: &str) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let client = BoxClient::new(stand_in);
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(820.)));
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
        workspace.open_read_at_once(session, window, cx);
    });
    cx.run_until_parked();
    (workspace, cx)
}

/// `open`, in a window tall enough for every changed file's header with the first ones open.
fn open_tall<'a>(cx: &'a mut TestAppContext, stand_in: StandIn, session: &str) -> (Entity<Workspace>, &'a mut VisualTestContext) {
    let (workspace, cx) = open(cx, stand_in, session);
    cx.simulate_resize(size(px(1280.), px(2400.)));
    cx.run_until_parked();
    (workspace, cx)
}

#[track_caller]
fn bounds(cx: &mut VisualTestContext, selector: &str) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
}

#[track_caller]
fn click(cx: &mut VisualTestContext, selector: &str) {
    let at = bounds(cx, selector).unwrap_or_else(|| panic!("{selector} isn't on screen"));
    cx.simulate_click(at.center(), Modifiers::default());
    cx.run_until_parked();
}

fn sent(stand_in: &StandIn, route: &str) -> Vec<serde_json::Value> {
    stand_in.seen_at("POST", route).into_iter().filter_map(|seen| seen.body).collect()
}

#[gpui::test]
fn the_panel_opens_beside_the_conversation_and_keeps_its_width(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    assert!(bounds(cx, "side-panel").is_none(), "closed until it's asked for");

    // ⌥⌘0, as the Mac app's Show Panel: the New tab page, nothing open yet.
    cx.simulate_keystrokes("cmd-alt-0");
    cx.run_until_parked();
    assert!(bounds(cx, "new-tab").is_some());
    assert!(bounds(cx, "tile-files").is_some(), "the session works in a folder");
    // The tiles are a grid: two equal columns, each tile in its column, a lone last one keeping
    // its column's width, and the section's title over the tiles' left edge.
    let tiles: Vec<_> = ["tile-side-chat", "tile-terminal", "tile-browser", "tile-files"].iter().filter_map(|id| bounds(cx, id)).collect();
    assert!(tiles.len() >= 3, "a last row with a lone tile: {}", tiles.len());
    let grid = bounds(cx, "new-tab-tiles").unwrap();
    for (i, tile) in tiles.iter().enumerate() {
        assert_eq!(tile.size.width, tiles[0].size.width, "tile {i} as wide as the first");
        assert_eq!(tile.left(), tiles[i % 2].left(), "tile {i} in its column");
    }
    assert_eq!(tiles[0].left(), grid.left());
    assert_eq!(tiles[1].right(), grid.right());
    assert_eq!(tiles[1].left() - tiles[0].right(), tiles[2].top() - tiles[0].bottom(), "the same gap across and down");
    // ⌥⌘I: the session's folder's changes, as the box groups them, in a tab of their own.
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    let panel = bounds(cx, "side-panel").expect("the panel");
    let staged = bounds(cx, "change-Staged:src/components/SignupForm.tsx").expect("the sign-up form, staged");
    let unstaged = bounds(cx, "change-Unstaged:src/components/SignupForm.tsx").expect("and its changes since, unstaged");
    let new = bounds(cx, "change-Untracked:src/components/SignupForm.test.tsx").expect("a new file");
    assert!(staged.top() < unstaged.top() && unstaged.top() < new.top());
    let asked = stand_in.seen_at("GET", "kuvo/git/files");
    assert!(asked.last().unwrap().path.contains("directory=%2FUsers%2Fsam%2FProjects%2Fwebsite"), "asked of the session's folder");

    // Dragged wider by its edge.
    let edge = bounds(cx, "panel-edge").unwrap().center();
    cx.simulate_mouse_down(edge, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(point(edge.x - px(100.), edge.y), Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(point(edge.x - px(100.), edge.y), MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let wider = bounds(cx, "side-panel").unwrap();
    assert_eq!(wider.size.width, panel.size.width + px(100.));

    // Closed and opened again, as wide as it was; another conversation's folder in it.
    click(cx, "toggle-panel");
    assert!(bounds(cx, "side-panel").is_none());
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_limits", window, cx));
    click(cx, "toggle-panel");
    assert_eq!(bounds(cx, "side-panel").unwrap().size.width, wider.size.width);
    let place = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).place().cloned()).unwrap();
    assert_eq!(place.directory, "/Users/sam/Projects/api");
    assert!(bounds(cx, "change-Staged:src/components/SignupForm.tsx").is_none(), "the API has nothing changed");
}

#[gpui::test]
fn a_session_started_with_the_panel_open_shows_its_folder_once_the_box_lists_it(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init(Appearance::Light, cx);
        kuvo::bind_keys(cx);
    });
    let stand_in = StandIn::at(Replies::Never, NOW);
    let client = BoxClient::new(stand_in.clone());
    let window = cx.add_window(move |window, cx| Workspace::new(client, false, window, cx));
    let workspace = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(window.into(), cx).into_mut();
    cx.simulate_resize(size(px(1280.), px(820.)));
    workspace.update(cx, |workspace, cx| {
        workspace.set_now(NOW, cx);
        workspace.read_at_once(cx);
        workspace.toggle_panel(cx);
    });
    let place = |workspace: &Entity<Workspace>, cx: &mut VisualTestContext| workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).place().map(|place| place.directory.clone()));
    // On the landing view, the project a new one starts in; picking another moves it there.
    assert_eq!(place(&workspace, cx).as_deref(), Some("/Users/sam/Projects/website"));
    workspace.update(cx, |workspace, cx| workspace.pick_place(kuvo_box::models::StartIn::Project("/Users/sam/Projects/api".into()), cx));
    assert_eq!(place(&workspace, cx).as_deref(), Some("/Users/sam/Projects/api"));

    let field = workspace.read_with(cx, |workspace, cx| workspace.landing().read(cx).field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Add a health check route");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert!(workspace.read_with(cx, |workspace, _| workspace.opened().is_some()), "it opens before the box lists it");
    // The box's list comes, with the new session in it: the panel shows its folder.
    workspace.update(cx, |workspace, cx| workspace.read_at_once(cx));
    cx.run_until_parked();
    assert_eq!(place(&workspace, cx).as_deref(), Some("/Users/sam/Projects/api"));
}

#[gpui::test]
fn each_file_shows_its_diff_under_its_header_as_the_box_sends_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, cx) = open_tall(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();

    // The first files show open at once, each read in the scope the box listed it with.
    let asked = sent(&stand_in, "kuvo/git/changes");
    assert!(asked.contains(&json!({ "directory": website(), "scope": "staged", "path": "src/components/SignupForm.tsx" })), "in the scope the box listed it with");
    let staged = "diff-Staged:src/components/SignupForm.tsx";
    assert!(bounds(cx, staged).is_some());
    assert!(bounds(cx, "change-Untracked:src/components/SignupForm.test.tsx").is_some());
    assert!(bounds(cx, "diff-Untracked:src/components/SignupForm.test.tsx").is_none(), "the rest a click away");
    // Its long unchanged stretch is folded; a click opens it.
    let fold = format!("{staged}-unchanged-34");
    assert!(bounds(cx, &fold).is_some(), "the unchanged lines between its changes fold");
    click(cx, &fold);
    assert!(bounds(cx, &fold).is_none());
    // Its header folds it, and opens it again.
    click(cx, "change-Staged:src/components/SignupForm.tsx");
    assert!(bounds(cx, staged).is_none());
    click(cx, "change-Staged:src/components/SignupForm.tsx");
    assert!(bounds(cx, staged).is_some());
    // Staged while open, it stays open as the staged file it became, read in its new scope.
    click(cx, "stage-Unstaged:src/lib/validate.ts");
    assert!(bounds(cx, "diff-Staged:src/lib/validate.ts").is_some(), "still open");
    assert_eq!(sent(&stand_in, "kuvo/git/changes").last().unwrap()["scope"], json!("staged"), "read again in its new scope");
}

#[gpui::test]
fn staging_sends_back_what_the_box_listed_and_a_discard_is_asked_first(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (_workspace, cx) = open_tall(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();

    click(cx, "stage-Unstaged:src/lib/validate.ts");
    assert_eq!(sent(&stand_in, "kuvo/git/stage"), [json!({ "root": website(), "path": "src/lib/validate.ts", "status": "modified", "scope": "unstaged" })]);
    assert!(bounds(cx, "change-Staged:src/lib/validate.ts").is_some(), "the list read again shows it staged");
    assert!(bounds(cx, "change-Unstaged:src/lib/validate.ts").is_none());

    click(cx, "unstage-Staged:src/lib/validate.ts");
    assert_eq!(sent(&stand_in, "kuvo/git/unstage").len(), 1);
    assert!(bounds(cx, "change-Unstaged:src/lib/validate.ts").is_some());

    // Discard asks first; Cancel sends nothing.
    click(cx, "discard-Untracked:src/components/SignupForm.test.tsx");
    assert!(bounds(cx, "discard-confirm").is_some());
    click(cx, "discard-cancel");
    assert!(bounds(cx, "discard-confirm").is_none());
    assert!(sent(&stand_in, "kuvo/git/discard").is_empty());

    click(cx, "discard-Unstaged:src/styles/legacy.css");
    click(cx, "discard-confirmed");
    assert_eq!(
        sent(&stand_in, "kuvo/git/discard"),
        [json!({ "root": website(), "path": "src/styles/legacy.css", "status": "deleted", "scope": "unstaged", "stamp": "unstaged:src/styles/legacy.css:1" })],
        "with what it was shown, so the box refuses it once it changed"
    );
}

// The stand-in refuses what the box refuses: a discard that doesn't say what the file was when it
// was listed, so a change here that dropped the stamp would fail these tests.
#[gpui::test]
fn a_discard_with_no_stamp_is_refused_as_the_box_refuses_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, _cx) = open(cx, stand_in.clone(), "ses_signup");
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let listed = client.git_files(&website()).unwrap().files.into_iter().find(|file| file.path == "src/styles/legacy.css").unwrap();
    let mut bare = listed.clone();
    bare.stamp = None;
    let refused = client.discard(&website(), &bare).unwrap_err();
    assert!(refused.to_string().contains("wasn’t discarded"), "{refused}");
    let mut stale = listed.clone();
    stale.stamp = Some("an older one".into());
    assert!(client.discard(&website(), &stale).unwrap_err().to_string().contains("changed since it was listed"));
    assert!(client.git_files(&website()).unwrap().files.iter().any(|file| file.path == "src/styles/legacy.css"), "still there");
    // As it was listed: it goes.
    client.discard(&website(), &listed).unwrap();
    assert!(!client.git_files(&website()).unwrap().files.iter().any(|file| file.path == "src/styles/legacy.css"));
}

#[gpui::test]
fn a_discard_waiting_for_its_confirm_goes_once_its_file_changes(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open_tall(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    click(cx, "discard-Unstaged:src/lib/validate.ts");
    assert!(bounds(cx, "discard-confirm").is_some());

    // An agent edits it again meanwhile: the list the follow hands on has it changed.
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let mut files = client.git_files(&website()).unwrap().files;
    let edited = files.iter_mut().find(|file| file.path == "src/lib/validate.ts").unwrap();
    edited.additions = Some(12);
    edited.stamp = Some("unstaged:src/lib/validate.ts:2".into());
    let files: Vec<serde_json::Value> = files
        .iter()
        .map(|file| {
            let mut value = json!({ "path": file.path, "group": format!("{:?}", file.group).to_lowercase(), "status": file.status, "scope": file.scope, "additions": file.additions, "deletions": file.deletions });
            if let Some(discard) = &file.discard {
                value["discard"] = json!(discard);
                value["stamp"] = json!(file.stamp);
            }
            value
        })
        .collect();
    stand_in.set_git_files(&website(), files);
    let view = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).changes().unwrap());
    view.update(cx, |view, cx| view.take(client.git_files(&website()), cx));
    cx.run_until_parked();
    assert!(bounds(cx, "discard-confirm").is_none(), "an old confirm never discards what came after");
    assert!(sent(&stand_in, "kuvo/git/discard").is_empty());
}

#[gpui::test]
fn a_files_button_does_what_the_box_says_whatever_group_it_is_in(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    // The box's word alone picks the button: here one that goes against the file's group, and one
    // this app doesn't know.
    stand_in.set_git_files(
        &website(),
        vec![
            json!({ "path": "a.ts", "group": "unstaged", "status": "modified", "scope": "unstaged", "stage": "unstage" }),
            json!({ "path": "b.ts", "group": "unstaged", "status": "modified", "scope": "unstaged", "stage": "shelve" }),
            json!({ "path": "c.ts", "group": "staged", "status": "modified", "scope": "staged", "stage": "resolve" }),
        ],
    );
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let view = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).changes().unwrap());
    view.update(cx, |view, cx| view.take(client.git_files(&website()), cx));
    cx.run_until_parked();
    assert!(bounds(cx, "unstage-Unstaged:a.ts").is_some());
    assert!(bounds(cx, "stage-Unstaged:a.ts").is_none());
    assert!(bounds(cx, "stage-Unstaged:b.ts").is_none() && bounds(cx, "unstage-Unstaged:b.ts").is_none(), "a word it doesn't know draws no button");
    assert!(bounds(cx, "change-Unstaged:b.ts").is_some(), "and drops no row");
    click(cx, "unstage-Unstaged:a.ts");
    assert_eq!(sent(&stand_in, "kuvo/git/unstage"), [json!({ "root": website(), "path": "a.ts", "status": "modified", "scope": "unstaged" })]);
    // A conflict's is marked resolved by staging it.
    click(cx, "stage-Staged:c.ts");
    assert_eq!(sent(&stand_in, "kuvo/git/stage").len(), 1);
}

#[gpui::test]
fn a_binary_files_diff_says_so_as_the_box_does(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    stand_in.set_git_files(&website(), vec![json!({ "path": "public/favicon.png", "group": "unstaged", "status": "modified", "scope": "unstaged" })]);
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let view = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).changes().unwrap());
    view.update(cx, |view, cx| view.take(client.git_files(&website()), cx));
    cx.run_until_parked();
    // (The first file shows open.)
    assert_eq!(view.read_with(cx, |view, _| view.opened("Unstaged:public/favicon.png").and_then(|opened| opened.unshown)), Some("A binary file: no lines to show."));
    assert!(bounds(cx, "diff-Unstaged:public/favicon.png").is_none());

    // And only the box's word: git's words for a binary file in a patch the box doesn't call one
    // (here with no lines either) aren't read as it.
    stand_in.set_git_files(&website(), vec![json!({ "path": "notes.txt", "group": "unstaged", "status": "modified", "scope": "unstaged" })]);
    stand_in.set_git_patch(&website(), "notes.txt", "diff --git a/notes.txt b/notes.txt\nBinary files a/notes.txt and b/notes.txt differ\n");
    view.update(cx, |view, cx| view.take(client.git_files(&website()), cx));
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.opened("Unstaged:notes.txt").and_then(|opened| opened.unshown)), None);
}

#[gpui::test]
fn a_merge_under_way_and_files_staged_elsewhere_are_said(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    stand_in.set_git_state(&website(), 2, Some("merging"));
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    assert!(bounds(cx, "changes-underway").is_some());
    let files = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).changes().unwrap().read(cx).files().ready().cloned()).unwrap();
    assert_eq!(kuvo::panel::changes::commit_takes(&files), "Commits the 3 staged files, 2 of them outside this folder.");
}

#[gpui::test]
fn a_commit_goes_with_its_message_and_a_push_that_didnt_go_offers_push(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).changes().unwrap());
    let field = view.read_with(cx, |view, _| view.message().clone());

    // Nothing written: Commit does nothing.
    click(cx, "commit");
    assert!(sent(&stand_in, "kuvo/git/commit").is_empty());

    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Keep the form when an email is taken");
    click(cx, "commit-push");
    stand_in.fail_next_push();
    click(cx, "commit");
    assert_eq!(sent(&stand_in, "kuvo/git/commit"), [json!({ "directory": website(), "message": "Keep the form when an email is taken", "push": true })]);
    // Committed (what was staged is gone from the list), but not pushed: Push, never the commit again.
    assert!(bounds(cx, "change-Staged:src/components/SignupForm.tsx").is_none());
    assert!(matches!(view.read_with(cx, |view, _| view.said().cloned()), Some(CommitSaid::NotPushed(_))));
    assert_eq!(field.read_with(cx, |field, _| field.text().to_owned()), "", "the message went with the commit");
    click(cx, "push-again");
    assert_eq!(sent(&stand_in, "kuvo/git/push"), [json!({ "directory": website() })]);
    assert_eq!(view.read_with(cx, |view, _| view.said().cloned()), Some(CommitSaid::Pushed));
}

#[gpui::test]
fn files_open_a_folder_at_a_time_and_one_that_cant_be_read_says_why(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-f");
    cx.run_until_parked();
    let root = website();
    assert!(bounds(cx, &format!("tree-{root}/src")).is_some());
    assert!(bounds(cx, &format!("tree-{root}/README.md")).is_some());
    assert!(bounds(cx, &format!("tree-{root}/src/main.tsx")).is_none(), "a folder is read as it opens");

    click(cx, &format!("tree-{root}/src"));
    assert!(stand_in.seen_at("GET", "kuvo/folders").iter().any(|seen| seen.path.contains("%2Fsrc&files=1")));
    assert!(bounds(cx, &format!("tree-{root}/src/main.tsx")).is_some());

    click(cx, &format!("tree-{root}/secrets"));
    assert!(bounds(cx, &format!("unreadable-{root}/secrets")).is_some(), "never shown as empty");
    let rows = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).files().unwrap().read(cx).rows().to_vec());
    assert!(rows.iter().any(|row| matches!(row, kuvo::panel::files::Row::Note { read: kuvo::panel::files::Read::Unreadable(why), .. } if why.reason.as_deref() == Some("Permission denied"))));

    // A file picked opens in a tab of its own, in the Files tab's place, the folder's files beside it.
    click(cx, &format!("tree-{root}/src/components"));
    let form = format!("{root}/src/components/SignupForm.tsx");
    click(cx, &format!("tree-{form}"));
    assert_eq!(showing(&workspace, cx), Some(Kind::File(form.clone())));
    let file = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).file()).unwrap();
    assert!(matches!(file.read_with(cx, |file, _| file.read().clone()), Load::Ready(Shown::Text { lines: 58, .. })));
    assert!(bounds(cx, "file-viewer").is_some() && bounds(cx, "file-tree").is_some());
    assert!(bounds(cx, &format!("tree-{root}/src/components/Button.tsx")).is_some(), "the tree as it was left");
    // Another from the tree beside it: a tab of its own too, the first kept.
    click(cx, &format!("tree-{root}/README.md"));
    assert_eq!(showing(&workspace, cx), Some(Kind::File(format!("{root}/README.md"))));
    let tabs = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).strip().unwrap().tabs().len());
    assert_eq!(tabs, 2);
    // The tree hides by its button, and comes back.
    click(cx, "file-tree-toggle");
    assert!(bounds(cx, "file-tree").is_none() && bounds(cx, "file-viewer").is_some());
    click(cx, "file-tree-toggle");

    // One the box says isn't text (a picture) says so in the viewer's place; the box is asked for
    // the file as a viewer shows it, never for its bytes to look through.
    click(cx, &format!("tree-{root}/public"));
    click(cx, &format!("tree-{root}/public/favicon.png"));
    let file = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).file()).unwrap();
    assert!(matches!(file.read_with(cx, |file, _| file.read().clone()), Load::Ready(Shown::Unshown(_))));
    assert!(bounds(cx, "files-open-image").is_some());
    assert!(stand_in.seen_at("GET", "kuvo/file").is_empty());
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let path = URL_SAFE_NO_PAD.encode(format!("{root}/public/favicon.png"));
    assert!(stand_in.seen_at("GET", "kuvo/file/text").iter().any(|seen| seen.path.starts_with(&format!("kuvo/file/text?path64={path}&max="))));
}

#[gpui::test]
fn branches_switch_and_a_new_one_starts_where_this_one_is(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    workspace.update(cx, |workspace, cx| workspace.show_panel(Kind::Branches, cx));
    cx.run_until_parked();
    assert!(bounds(cx, "branch-fix/signup-taken-email").is_some());
    assert!(bounds(cx, "branch-merge").is_none(), "Merge into… is a worktree's");

    click(cx, "branch-main");
    assert_eq!(sent(&stand_in, "kuvo/git/switch"), [json!({ "directory": website(), "branch": "main", "create": false })]);

    click(cx, "branch-new");
    cx.simulate_input("feature/password-hints");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(sent(&stand_in, "kuvo/git/switch").last().unwrap(), &json!({ "directory": website(), "branch": "feature/password-hints", "create": true }));
    assert!(bounds(cx, "branch-feature/password-hints").is_some());
}

#[gpui::test]
fn a_worktrees_branch_merges_where_the_box_says_and_says_why_it_cant(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_dark");
    workspace.update(cx, |workspace, cx| workspace.show_panel(Kind::Branches, cx));
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).branches().unwrap());

    stand_in.merges_conflict(&["src/styles/theme.css"]);
    click(cx, "branch-merge");
    let said = view.read_with(cx, |view, _| view.said().cloned());
    assert_eq!(said, Some(Said::Failed("main changed some of the same lines: src/styles/theme.css. Nothing was merged.".into())));

    stand_in.merges_conflict(&[]);
    click(cx, "branch-merge");
    assert_eq!(view.read_with(cx, |view, _| view.said().cloned()), Some(Said::Done("Merged into main.".into())));
    assert_eq!(sent(&stand_in, "kuvo/git/merge").len(), 2);
}

#[gpui::test]
fn a_pull_request_starts_from_the_boxs_draft_and_is_made_from_the_session(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    workspace.update(cx, |workspace, cx| workspace.show_panel(Kind::Branches, cx));
    cx.run_until_parked();
    click(cx, "branch-pull-request");
    assert!(bounds(cx, "pull-request").is_some());
    let view = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).branches().unwrap());
    let title = view.read_with(cx, |view, cx| view.writing().unwrap().title.read(cx).text().to_owned());
    assert_eq!(title, "Keep the sign-up form when an email is taken", "the box's draft");

    click(cx, "pull-request-create");
    assert_eq!(
        sent(&stand_in, "kuvo/git/pull-request"),
        [json!({ "directory": website(), "title": "Keep the sign-up form when an email is taken", "body": "- Keep what was typed when the email is taken\n- Say so beside the email field", "session": "ses_signup" })]
    );
    assert!(matches!(view.read_with(cx, |view, _| view.said().cloned()), Some(Said::PullRequest { url: Some(_), .. })));
}

#[gpui::test]
fn a_pull_request_isnt_offered_where_the_box_says_none_can_be_made(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    // The API's session is on main, the branch pull requests go into.
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_limits");
    workspace.update(cx, |workspace, cx| workspace.show_panel(Kind::Branches, cx));
    cx.run_until_parked();
    assert!(bounds(cx, "branch-current").is_some());
    assert!(bounds(cx, "branch-pull-request").is_none(), "never offered only to be refused");
    assert!(sent(&stand_in, "kuvo/git/pull-request/draft").is_empty());
}

#[gpui::test]
fn the_list_follows_the_box_as_it_changes(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    // What a follow hands on (an agent's edit, staged elsewhere), taken as it would be.
    stand_in.set_git_files(&website(), vec![json!({ "path": "src/main.tsx", "group": "conflicted", "status": "modified", "scope": "uncommitted" })]);
    let client = workspace.read_with(cx, |workspace, _| workspace.client().clone());
    let view = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).changes().unwrap());
    view.update(cx, |view, cx| view.take(client.git_files(&website()), cx));
    cx.run_until_parked();
    assert!(bounds(cx, "change-Conflicted:src/main.tsx").is_some());
    assert!(bounds(cx, "discard-Conflicted:src/main.tsx").is_none(), "a conflict is resolved, not discarded");
    assert!(bounds(cx, "stage-Conflicted:src/main.tsx").is_some(), "Mark as Resolved");
    assert!(bounds(cx, "change-Staged:src/components/SignupForm.tsx").is_none());
}

/// The kind of tab showing in the conversation in view.
fn showing(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<Kind> {
    workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).active().map(|tab| tab.kind.clone()))
}

fn tab_of(workspace: &Entity<Workspace>, cx: &mut VisualTestContext, kind: &Kind) -> u64 {
    workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).strip().unwrap().tabs().iter().find(|tab| &tab.kind == kind).map(|tab| tab.id)).expect("its tab")
}

#[gpui::test]
fn each_conversation_keeps_its_own_tabs_and_the_panel_follows_the_one_in_view(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    assert_eq!(showing(&workspace, cx), Some(Kind::Changes));

    // Another conversation: its own strip, on its New tab page.
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_limits", window, cx));
    cx.run_until_parked();
    assert_eq!(showing(&workspace, cx), Some(Kind::New));
    assert!(bounds(cx, "new-tab").is_some());
    assert!(bounds(cx, "change-Staged:src/components/SignupForm.tsx").is_none());

    // Back: its Changes tab is there as it was left.
    workspace.update_in(cx, |workspace, window, cx| workspace.open_read_at_once("ses_signup", window, cx));
    cx.run_until_parked();
    assert_eq!(showing(&workspace, cx), Some(Kind::Changes));
    assert!(bounds(cx, "change-Staged:src/components/SignupForm.tsx").is_some());
}

#[gpui::test]
fn tabs_keep_their_names_while_the_strip_has_room(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    for kind in [Kind::Changes, Kind::Files, Kind::Subagents, Kind::SideChat(None)] {
        workspace.update(cx, |workspace, cx| workspace.show_panel(kind, cx));
    }
    cx.run_until_parked();
    let panel = bounds(cx, "side-panel").expect("the panel");
    let tabs: Vec<u64> = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).strip().unwrap().tabs().iter().map(|tab| tab.id).collect());
    assert_eq!(tabs.len(), 4);
    let widths: Vec<Pixels> = tabs.iter().map(|id| bounds(cx, &format!("panel-tab-{id}")).expect("each tab").size.width).collect();
    // Four short names fit a panel this wide: none is squeezed to an icon's width.
    assert!(widths.iter().sum::<Pixels>() < panel.size.width, "they fit: {widths:?} in {:?}", panel.size.width);
    // Each as wide as its name: Subagents' tab wider than Files', none pinned to one width.
    assert!(widths[2] > widths[1] + px(15.), "each as wide as its name: {widths:?}");
    assert!(widths.iter().all(|width| *width > px(72.)), "none squeezed: {widths:?}");

    // More than fit: the strip scrolls, the one showing in view with its name.
    for file in ["src/lib/validate.ts", "src/components/SignupForm.tsx"] {
        workspace.update_in(cx, |workspace, window, cx| workspace.preview_file(&format!("{}/{file}", website()), window, cx));
        cx.run_until_parked();
    }
    let showing = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).active().unwrap().id);
    let tab = bounds(cx, &format!("panel-tab-{showing}")).expect("the file's tab");
    let strip = bounds(cx, "panel-strip").unwrap();
    assert!(tab.left() >= strip.left() && tab.right() <= strip.right(), "the tab showing is in view: {tab:?} in {strip:?}");
}

#[gpui::test]
fn plus_opens_the_new_tab_page_a_tile_takes_its_place_and_closing_the_last_tab_leaves_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-0");
    cx.run_until_parked();
    assert!(bounds(cx, "panel-new-tab").is_none(), "nothing but the New tab page: no + beside it");

    click(cx, "tile-files");
    assert_eq!(showing(&workspace, cx), Some(Kind::Files));
    assert!(bounds(cx, &format!("tree-{}/src", website())).is_some());
    let strip = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).strip().unwrap().tabs().len());
    assert_eq!(strip, 1, "the tile took the New tab page's place");

    click(cx, "panel-new-tab");
    assert_eq!(showing(&workspace, cx), Some(Kind::New));
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    assert_eq!(showing(&workspace, cx), Some(Kind::Changes), "it took the New tab page's place too");

    let files = tab_of(&workspace, cx, &Kind::Files);
    let changes = tab_of(&workspace, cx, &Kind::Changes);
    click(cx, &format!("panel-tab-close-{changes}"));
    assert_eq!(showing(&workspace, cx), Some(Kind::Files));
    click(cx, &format!("panel-tab-close-{files}"));
    assert_eq!(showing(&workspace, cx), Some(Kind::New), "never an empty panel");

    click(cx, "panel-close");
    assert!(bounds(cx, "side-panel").is_none());
    assert!(!workspace.read_with(cx, |workspace, _| workspace.is_panel_open()));
}

#[gpui::test]
fn in_a_narrow_window_the_panel_slides_over_the_conversation_and_a_click_beside_it_closes_it(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_resize(size(px(860.), px(760.)));
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    let sheet = bounds(cx, "panel-sheet").expect("over the conversation");
    let content = bounds(cx, "workspace-content").unwrap();
    assert!(bounds(cx, "panel-edge").is_none(), "not beside it");
    assert!(content.right() > sheet.left(), "the conversation keeps its width under it");
    assert!(bounds(cx, "change-Staged:src/components/SignupForm.tsx").is_some());

    let beside = point(content.left() + px(40.), content.center().y);
    cx.simulate_click(beside, Modifiers::default());
    cx.run_until_parked();
    assert!(!workspace.read_with(cx, |workspace, _| workspace.is_panel_open()));

    // A wide window again: beside it.
    cx.simulate_resize(size(px(1280.), px(820.)));
    cx.simulate_keystrokes("cmd-alt-0");
    cx.run_until_parked();
    assert!(bounds(cx, "panel-edge").is_some());
    assert!(bounds(cx, "panel-sheet").is_none());
}

#[gpui::test]
fn the_filter_and_the_new_tab_search_open_what_the_box_finds(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    let root = website();
    // Files: what the box finds for the filter, best first, in the tree's place.
    cx.simulate_keystrokes("cmd-alt-f");
    cx.run_until_parked();
    let filter = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).files().unwrap().read(cx).filter().clone());
    cx.update(|window, cx| window.focus(&filter.focus_handle(cx), cx));
    cx.simulate_input("signup");
    cx.run_until_parked();
    let asked = stand_in.seen_at("GET", "kuvo/files/find");
    assert!(asked.last().unwrap().path.contains("q=signup"), "the box is asked");
    let found = bounds(cx, &format!("found-{root}/src/components/SignupForm.tsx")).expect("the form");
    let test = bounds(cx, &format!("found-{root}/src/components/SignupForm.test.tsx")).expect("its test");
    assert!(found.top() < test.top(), "in the box's order");
    assert!(bounds(cx, &format!("tree-{root}/src")).is_none());
    let first = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).files().unwrap().read(cx).found().unwrap().files[0].path.clone());
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(showing(&workspace, cx), Some(Kind::File(first)), "Enter opens the first the box found");

    // The New tab page: the same, and a file found opens in its place.
    let tabs = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).strip().map(|strip| strip.tabs().to_vec()));
    assert!(bounds(cx, "panel-new-tab").is_some(), "+ beside {tabs:?}");
    click(cx, "panel-new-tab");
    cx.simulate_input("validate");
    cx.run_until_parked();
    click(cx, &format!("new-tab-found-{root}/src/lib/validate.ts"));
    assert_eq!(showing(&workspace, cx), Some(Kind::File(format!("{root}/src/lib/validate.ts"))));
    assert!(bounds(cx, "new-tab").is_none());
    // Closed and opened again: its tabs as they were, + beside them.
    click(cx, "panel-close");
    click(cx, "toggle-panel");
    let tabs = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).strip().map(|strip| strip.tabs().to_vec()));
    assert!(bounds(cx, "panel-new-tab").is_some(), "+ beside {tabs:?} once it's open again");
    click(cx, "panel-new-tab");
    cx.simulate_input("nothing-like-this");
    cx.run_until_parked();
    assert!(bounds(cx, "tile-files").is_none(), "what it found (none) in the tiles' place");
}

#[gpui::test]
fn a_file_the_conversation_names_opens_in_its_tab(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open_tall(cx, stand_in.clone(), "ses_signup");
    // (Its step's group opened, as you would.)
    let view = workspace.read_with(cx, |workspace, _| workspace.conversation().unwrap().clone());
    view.update(cx, |view, cx| view.open_step("msg_s2", 1, cx));
    cx.run_until_parked();
    click(cx, "tool-file-msg_s2:1");
    assert!(workspace.read_with(cx, |workspace, _| workspace.is_panel_open()));
    assert_eq!(showing(&workspace, cx), Some(Kind::File(format!("{}/src/components/SignupForm.tsx", website()))));
    assert!(bounds(cx, "file-viewer").is_some());
}

#[gpui::test]
fn the_subagents_tab_lists_those_at_work_and_done_and_opens_one_to_its_conversation(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_background("ses_signup", json!({ "commands": [], "subagents": [{ "session": "ses_sub", "since": NOW - 30_000.0, "title": "Check the other forms" }] }), &[]);
    stand_in.set_subagents_done("ses_signup", vec![json!({ "session": "ses_dark", "title": "Read the error codes", "started": NOW - 260_000_000.0, "ended": NOW - 259_000_000.0, "state": "finished", "model": "Claude Sonnet 4.6", "effort": "High" })]);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    workspace.update(cx, |workspace, cx| workspace.arrive(kuvo::panel::Arrival::Subagents(None), cx));
    cx.run_until_parked();
    assert_eq!(showing(&workspace, cx), Some(Kind::Subagents));
    let working = bounds(cx, "subagent-ses_sub").expect("at work");
    let done = bounds(cx, "subagent-ses_dark").expect("done");
    assert!(working.top() < done.top());
    assert_eq!(stand_in.seen_at("GET", "kuvo/work/subagents/ses_signup").len(), 1, "as the box lists them");

    click(cx, "subagent-ses_dark");
    assert!(bounds(cx, "subagent-transcript").is_some() && bounds(cx, "subagents-list").is_none());
    let rows = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).subagents().unwrap().read(cx).opened().map(str::to_owned));
    assert_eq!(rows.as_deref(), Some("ses_dark"));
    click(cx, "subagent-back");
    // One with a conversation: its rows drawn in the tab.
    stand_in.set_subagents_done("ses_signup", vec![json!({ "session": "ses_limits", "title": "Audit email rate limits", "started": NOW - 9_000.0, "ended": NOW - 8_000.0, "state": "finished" })]);
    workspace.update(cx, |workspace, cx| workspace.arrive(kuvo::panel::Arrival::Subagents(Some("ses_limits".into())), cx));
    cx.run_until_parked();
    let transcript = bounds(cx, "subagent-transcript").unwrap();
    let row = bounds(cx, "entry-msg_l1").expect("its first message drawn");
    assert!(row.size.height > px(0.) && row.left() >= transcript.left(), "in the tab");
    workspace.update(cx, |workspace, cx| workspace.arrive(kuvo::panel::Arrival::Subagents(None), cx));
    cx.run_until_parked();
    assert!(bounds(cx, "subagents-list").is_some());
    // Open, to talk to it: its conversation in the window.
    click(cx, "subagent-ses_dark");
    click(cx, "subagent-open");
    assert_eq!(workspace.read_with(cx, |workspace, _| workspace.opened().map(str::to_owned)).as_deref(), Some("ses_dark"));
}

#[gpui::test]
fn a_side_chat_asks_aside_follows_up_in_its_thread_and_opens_again_from_the_outputs(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.answer_side_questions(&["Postgres raises 23505 atomically.", "Yes: both count."]);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-0");
    cx.run_until_parked();
    click(cx, "tile-side-chat");
    assert_eq!(showing(&workspace, cx), Some(Kind::SideChat(None)));
    let view = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).side_chat()).unwrap();
    let field = view.read_with(cx, |view, _| view.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Why check 23505?");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(sent(&stand_in, "kuvo/agents/api/kuvo/btw/ses_signup"), [json!({ "question": "Why check 23505?" })]);
    assert_eq!(showing(&workspace, cx), Some(Kind::SideChat(Some("btw_1".into()))), "the tab is its thread's now");
    assert!(bounds(cx, "side-question-btw_1").is_some());

    // A follow-up goes in the same thread.
    cx.simulate_input("And the bucket?");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    assert_eq!(sent(&stand_in, "kuvo/agents/api/kuvo/btw/ses_signup").last().unwrap(), &json!({ "question": "And the bucket?", "thread": "btw_1" }));
    let thread = view.read_with(cx, |view, _| view.thread().map(|thread| thread.questions.len()));
    assert_eq!(thread, Some(2));

    // Closed, it's in the conversation's Outputs, and opens its tab again.
    let tab = tab_of(&workspace, cx, &Kind::SideChat(Some("btw_1".into())));
    click(cx, &format!("panel-tab-close-{tab}"));
    click(cx, "conversation-activity-button");
    click(cx, "activity-side-chat-btw_1");
    assert_eq!(showing(&workspace, cx), Some(Kind::SideChat(Some("btw_1".into()))));
}

/// A long side chat ends above its floating composer (11-23): whatever comes in after it's asked
/// (the question, its answer, the composer's height as it's measured), it stays at its end.
#[gpui::test]
fn a_long_side_chat_keeps_its_last_answer_above_the_composer(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let answer = "The unique index is the one place that settles it, so the code inserts first and treats 23505 as taken. Querying first leaves a gap two sign-ups can both pass.";
    stand_in.answer_side_questions(&[answer; 8]);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    // Opened with its questions asked before it's first drawn (the composer's height not measured
    // yet), each question and its answer coming in after.
    workspace.update(cx, |workspace, cx| {
        workspace.show_panel(Kind::SideChat(None), cx);
        let view = workspace.panel().read(cx).side_chat().unwrap();
        for ix in 1..=8 {
            let field = view.read(cx).field().clone();
            field.update(cx, |field, cx| field.set_text(&format!("Why check 23505 instead of querying first? ({ix})"), cx));
            view.update(cx, |view, cx| view.ask(cx));
        }
    });
    cx.run_until_parked();
    let view = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).side_chat()).unwrap();
    // (Drawn twice: the composer's height is measured as it's laid out.)
    cx.update(|window, _| window.refresh());
    cx.run_until_parked();
    let (asked, last) = view.read_with(cx, |view, _| view.thread().map(|thread| (thread.questions.len(), thread.questions.last().map(|question| question.id.clone())))).expect("its questions");
    let last = last.expect("its last question");
    assert_eq!(asked, 8);
    let long = view.read_with(cx, |view, _| view.scroll().max_offset().y);
    assert!(long > px(200.), "longer than the panel: {long:?}");
    let composer = bounds(cx, "side-chat-composer").expect("the composer");
    let question = bounds(cx, &format!("side-question-{last}")).expect("the last question");
    assert!(question.bottom() <= composer.top(), "the last question ends above the composer: {question:?}, {composer:?}");
    let answered = bounds(cx, "side-chat-end").expect("the transcript's end");
    assert!(answered.top() <= composer.top() + px(1.), "its answer ends above the composer: {answered:?}, {composer:?}");
}

#[gpui::test]
fn a_side_chat_asks_on_the_model_picked_with_a_file_attached_and_leaves_the_conversations_model(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.answer_side_questions(&["The debounce is 30 seconds."]);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-0");
    cx.run_until_parked();
    click(cx, "tile-side-chat");
    // The conversation's composer's paperclip and its one model chip: on the conversation's own model.
    assert!(bounds(cx, "side-chat-attach").is_some());
    assert!(bounds(cx, "side-chat-model").is_some() && bounds(cx, "effort-chip").is_none());
    let view = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).side_chat()).unwrap();
    assert_eq!(view.read_with(cx, |view, _| view.chip_label()).as_deref(), Some("Claude Opus 5.5"));

    // Another model, picked in the conversation's picker.
    click(cx, "side-chat-model");
    click(cx, "model-row-anthropic/claude-sonnet-5-5");
    assert_eq!(view.read_with(cx, |view, _| view.chip_label()).as_deref(), Some("Claude Sonnet 5.5"));
    // And its effort, in the picker's Reasoning, said on the chip.
    click(cx, "side-chat-model");
    click(cx, "model-effort-low");

    // A file dropped on it, shown above the field.
    let folder = tempfile::tempdir().unwrap();
    let notes = folder.path().join("notes.md");
    std::fs::write(&notes, "Debounce: 30s").unwrap();
    {
        use gpui::{ExternalPaths, FileDropEvent, InputEvent as _};
        let position = bounds(cx, "side-chat-composer").unwrap().center();
        cx.update(|window, cx| { window.dispatch_event(FileDropEvent::Entered { position, paths: ExternalPaths([notes.clone()].into_iter().collect()) }.to_platform_input(), cx); });
        cx.run_until_parked();
        cx.update(|window, cx| { window.dispatch_event(FileDropEvent::Submit { position }.to_platform_input(), cx); });
        cx.run_until_parked();
    }
    assert!(bounds(cx, "attachments").is_some_and(|strip| strip.top() >= bounds(cx, "side-chat-composer").unwrap().top()), "in the side chat's composer");

    let field = view.read_with(cx, |view, _| view.field().clone());
    cx.update(|window, cx| window.focus(&field.focus_handle(cx), cx));
    cx.simulate_input("Which delay?");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    // The model and the file go to the box with the question.
    let asked = sent(&stand_in, "kuvo/agents/api/kuvo/btw/ses_signup");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["question"], json!("Which delay?"));
    assert_eq!(asked[0]["model"]["providerID"], json!("anthropic"));
    assert_eq!(asked[0]["model"]["id"], json!("claude-sonnet-5-5"));
    assert_eq!(asked[0]["model"]["variant"], json!("low"));
    assert_eq!(asked[0]["files"].as_array().map(Vec::len), Some(1));
    assert_eq!(asked[0]["files"][0]["name"], json!("notes.md"));
    // Shown with its file, the composer emptied; the conversation stays on its own model.
    assert!(bounds(cx, "side-file-btw_1-0").is_some());
    assert!(view.read_with(cx, |view, _| view.attached().is_empty()));
    assert!(stand_in.seen_at("POST", "api/session/ses_signup/model").is_empty());
}

#[gpui::test]
fn a_bots_conversation_opens_the_panel_from_the_title_bar_and_its_side_chats_open_there(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.answer_side_questions(&["It reads the queue first."]);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    // A side chat asked in the bot's conversation before.
    BoxClient::new(stand_in.clone()).ask_aside("ses_mira", "What do you check first?", None, None, &[]).unwrap();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.show_area(kuvo::area::Area::Bots, window, cx);
        workspace.open_bot_chat("bot_mira", "ses_mira", None, window, cx);
    });
    cx.run_until_parked();
    assert!(bounds(cx, "bot-chat-title").is_some());
    assert!(bounds(cx, "side-panel").is_none(), "closed until it's asked for");

    // The title bar's button opens it beside the bot's conversation, and closes it again.
    click(cx, "toggle-panel");
    assert!(bounds(cx, "side-panel").is_some(), "the panel beside the bot's conversation");
    assert!(bounds(cx, "new-tab").is_some());
    click(cx, "toggle-panel");
    assert!(bounds(cx, "side-panel").is_none());

    // Its Outputs list the side chat; a click opens it in the panel, the bot's conversation still there.
    click(cx, "conversation-activity-button");
    click(cx, "activity-side-chat-btw_1");
    assert!(bounds(cx, "side-panel").is_some(), "the side chat opens the panel");
    assert!(bounds(cx, "bot-chat-title").is_some(), "still in the bot's conversation");
    assert_eq!(showing(&workspace, cx), Some(Kind::SideChat(Some("btw_1".into()))));
    assert!(bounds(cx, "side-question-btw_1").is_some());

    // Back in Sessions, the session's own tabs, not the bot's.
    workspace.update_in(cx, |workspace, window, cx| workspace.show_area(kuvo::area::Area::Sessions, window, cx));
    cx.run_until_parked();
    assert!(bounds(cx, "side-panel").is_some());
    assert_ne!(showing(&workspace, cx), Some(Kind::SideChat(Some("btw_1".into()))));
}

#[gpui::test]
fn a_terminal_opens_as_a_tab_in_the_conversations_folder_and_keeps_its_shell_while_hidden(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-0");
    cx.run_until_parked();
    click(cx, "tile-terminal");
    assert!(matches!(showing(&workspace, cx), Some(Kind::Terminal(_))));
    assert_eq!(stand_in.seen_at("POST", "api/pty")[0].body, Some(json!({ "cwd": website() })), "in the conversation's folder");
    let terminal = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).terminal()).unwrap();
    stand_in.pty_print("pty_1", b"~/website $ ");
    cx.run_until_parked();
    assert!(terminal.read_with(cx, |view, _| view.screen_text().join("\n")).contains("~/website $"));

    // Another tab, and back: the same shell, what it printed meanwhile there.
    cx.simulate_keystrokes("cmd-alt-i");
    cx.run_until_parked();
    stand_in.pty_print("pty_1", b"done\r\n");
    let tab = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).strip().unwrap().tabs().iter().find(|tab| matches!(tab.kind, Kind::Terminal(_))).map(|tab| tab.id)).unwrap();
    click(cx, &format!("panel-tab-{tab}"));
    assert_eq!(stand_in.seen_at("POST", "api/pty").len(), 1, "no second shell");
    assert!(terminal.read_with(cx, |view, _| view.screen_text().join("\n")).contains("done"));
}

#[gpui::test]
fn a_macs_box_offers_no_browser_tab_until_it_has_a_browser_of_its_own(cx: &mut TestAppContext) {
    // This Mac's box: no browser of its own to show yet, so no tile.
    {
        let stand_in = StandIn::at(Replies::Never, NOW);
        let (_workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
        cx.simulate_keystrokes("cmd-alt-0");
        cx.run_until_parked();
        assert!(bounds(cx, "tile-browser").is_none(), "never one that can't open");
    }
}

#[gpui::test]
fn a_machines_browser_opens_as_a_tab_named_after_its_page(cx: &mut TestAppContext) {
    let stand_in = StandIn::at(Replies::Never, NOW);
    stand_in.set_elsewhere(true);
    stand_in.browser_pages(json!({ "active": "P", "pages": [{ "targetId": "P", "url": "http://localhost:3000/signup", "title": "", "loading": false, "canGoBack": false, "canGoForward": false, "working": false }] }));
    let (workspace, cx) = open(cx, stand_in.clone(), "ses_signup");
    cx.simulate_keystrokes("cmd-alt-0");
    cx.run_until_parked();
    click(cx, "tile-browser");
    assert!(matches!(showing(&workspace, cx), Some(Kind::Browser(_))));
    assert!(workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).browser()).is_some());
    let tab = tab_of(&workspace, cx, &Kind::Browser(0));
    let title = workspace.read_with(cx, |workspace, cx| workspace.panel().read(cx).browser().unwrap().read(cx).title());
    assert_eq!(title.as_ref(), "localhost:3000", "its tab is named as the page is");
    assert!(bounds(cx, &format!("panel-tab-{tab}")).is_some());
}
