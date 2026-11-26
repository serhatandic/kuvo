//! A resident can subscribe to more roots than Linux's default inotify instance budget.

use kuvo_os::watch::{Backend, Notice, Options, Subscription, WatchHub};
use std::{fs, path::Path, time::Duration};

async fn changed(watch: &mut Subscription, path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match watch.recv().await.expect("watch closed") {
                Notice::Changed { paths } if paths.iter().any(|found| found == path) => return,
                Notice::Failed(error) => panic!("{error}"),
                _ => {}
            }
        }
    })
    .await
    .expect("missing watch event");
}

#[cfg(target_os = "linux")]
fn inotify_instances() -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|fd| {
            fs::read_link(fd.path()).is_ok_and(|path| path == Path::new("anon_inode:inotify"))
        })
        .count()
}

#[tokio::test]
async fn resident_shares_native_watcher_across_many_roots_and_overlapping_subscriptions() {
    let temp = tempfile::tempdir().unwrap();
    #[cfg(target_os = "linux")]
    let before = inotify_instances();
    let hub = WatchHub::default();
    let mut subscriptions = Vec::new();
    for index in 0..140 {
        let folder = temp.path().join(index.to_string());
        fs::create_dir(&folder).unwrap();
        let subscription = WatchHub::default()
            .subscribe(&folder, Options::default())
            .await
            .unwrap();
        assert_eq!(subscription.backend(), Backend::Native);
        subscriptions.push(subscription);
    }
    assert_eq!(hub.watched_folders().await.unwrap(), 140);
    #[cfg(target_os = "linux")]
    assert_eq!(inotify_instances(), before + 1);
    fs::write(temp.path().join("0/first"), "one").unwrap();
    fs::write(temp.path().join("139/last"), "two").unwrap();
    changed(&mut subscriptions[0], Path::new("first")).await;
    changed(&mut subscriptions[139], Path::new("last")).await;

    // Adding a recursive ancestor and then releasing it must not erase
    // the independently subscribed descendant's inotify registration.
    let parent = hub
        .subscribe(
            temp.path(),
            Options {
                recursive: true,
                create: false,
            },
        )
        .await
        .unwrap();
    fs::write(temp.path().join("139/covered"), "three").unwrap();
    changed(&mut subscriptions[139], Path::new("covered")).await;
    drop(parent);
    tokio::time::timeout(Duration::from_secs(10), async {
        while hub.watched_folders().await.unwrap() != 140 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    fs::write(temp.path().join("139/rearmed"), "four").unwrap();
    changed(&mut subscriptions[139], Path::new("rearmed")).await;
    #[cfg(target_os = "linux")]
    assert_eq!(inotify_instances(), before + 1);
}
