//! The Linux launcher runs from the main thread of a fresh executable. The child performs
//! real filesystem operations after the launcher has replaced itself.

#[cfg(not(target_os = "linux"))]
fn main() {}

#[cfg(target_os = "linux")]
fn main() {
    use kuvo_os::{
        platform::{Environment, Platform},
        sandbox::{FenceCommand, FileAccess, FileRule, LinuxFence, Mount, fence_exec},
    };
    use std::{
        env, fs,
        io::ErrorKind,
        path::{Path, PathBuf},
        process::Command,
    };

    let exe = env::current_exe().unwrap().canonicalize().unwrap();
    let mut args = env::args_os().skip(1);
    let Some(mode) = args.next() else {
        if Platform::current() == Platform::Wsl {
            assert!(!kuvo_os::sandbox::supported_platform());
            println!("fence-exec skipped on WSL (Auto unavailable)");
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        fs::create_dir(root.join("masked")).unwrap();
        fs::create_dir(root.join("project")).unwrap();
        fs::write(root.join("masked/socket-dir-marker"), "outside namespace").unwrap();
        fs::write(root.join("project/config"), "frozen").unwrap();
        fs::write(root.join("secret"), "outside Landlock grants").unwrap();
        let output = Command::new(&exe)
            .arg("shim")
            .arg(&root)
            .env_clear()
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "shim failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "sandboxed child verified\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("project/result")).unwrap(),
            "allowed"
        );
        assert_eq!(
            fs::read_to_string(root.join("project/config")).unwrap(),
            "frozen"
        );
        assert_eq!(
            fs::read_to_string(root.join("masked/socket-dir-marker")).unwrap(),
            "outside namespace"
        );
        println!("1 passed (Linux namespace mounts and Landlock in the same launched process)");
        return;
    };
    let root = PathBuf::from(args.next().unwrap());
    if mode == "shim" {
        let config = root.join("project/config");
        let masked = root.join("masked");
        let mut policy = LinuxFence {
            mounts: vec![
                Mount::Bind {
                    source: config.clone(),
                    target: config,
                    read_only: true,
                },
                Mount::EmptyDirectory {
                    path: masked.clone(),
                    mode: 0o700,
                },
            ],
            rules: vec![
                FileRule {
                    path: exe.clone(),
                    access: FileAccess::Read,
                },
                FileRule {
                    path: root.join("project"),
                    access: FileAccess::Full,
                },
                FileRule {
                    path: masked,
                    access: FileAccess::Read,
                },
            ],
        };
        // A GNU test executable also needs its dynamic loader and standard libraries.
        for directory in ["/lib", "/lib64", "/usr/lib", "/usr/lib64"] {
            if let Ok(path) = Path::new(directory).canonicalize()
                && !policy.rules.iter().any(|rule| rule.path == path)
            {
                policy.rules.push(FileRule {
                    path,
                    access: FileAccess::Read,
                });
            }
        }
        let command = FenceCommand {
            program: exe,
            args: vec!["payload".into(), root.clone().into_os_string()],
            env: Environment::new(),
            cwd: root.join("project"),
        };
        eprintln!("{}", fence_exec(&policy, &command));
        std::process::exit(126);
    }
    assert_eq!(mode, "payload");
    assert_eq!(env::current_dir().unwrap(), root.join("project"));
    assert_eq!(
        fs::read(root.join("secret")).unwrap_err().kind(),
        ErrorKind::PermissionDenied
    );
    assert_eq!(
        fs::read(root.join("masked/socket-dir-marker"))
            .unwrap_err()
            .kind(),
        ErrorKind::NotFound
    );
    assert_eq!(fs::read_dir(root.join("masked")).unwrap().count(), 0);
    assert!(fs::write("config", "overwrite").is_err());
    fs::write("result", "allowed").unwrap();
    println!("sandboxed child verified");
}
