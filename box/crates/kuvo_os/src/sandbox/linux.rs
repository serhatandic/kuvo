use crate::platform::{Environment, Platform};
use landlock::{
    ABI, Access, AccessFs, CompatLevel, Compatible, PathBeneath, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus, Scope,
};
use rustix::{
    fd::{AsRawFd, OwnedFd},
    fs::{CWD, Mode, OFlags, ResolveFlags, fstat, openat2},
    mount::{
        FsMountFlags, FsOpenFlags, MountAttrFlags, MountPropagationFlags, MoveMountFlags,
        OpenTreeFlags, fsconfig_create, fsconfig_set_string, fsmount, fsopen, mount_change,
        move_mount, open_tree,
    },
    process::{getgid, getuid},
    thread::{
        CapabilitySet, CapabilitySets, UnshareFlags, set_capabilities, set_no_new_privs,
        unshare_unsafe,
    },
};
use std::{
    ffi::OsString,
    fs, io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug)]
pub enum Mount {
    /// Bind a file or directory onto itself (or mask a socket by binding /dev/null over it).
    Bind {
        source: PathBuf,
        target: PathBuf,
        read_only: bool,
    },
    EmptyDirectory {
        path: PathBuf,
        mode: u16,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum FileAccess {
    List,
    Read,
    HomeWrite,
    ReadWriteFile,
    Full,
}

#[derive(Debug)]
pub struct FileRule {
    pub path: PathBuf,
    pub access: FileAccess,
}

pub struct LinuxFence {
    /// Ordered mounts from the policy layer, before Landlock paths are opened.
    pub mounts: Vec<Mount>,
    pub rules: Vec<FileRule>,
}

#[derive(Debug)]
pub struct FenceCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Environment,
    pub cwd: PathBuf,
}

/// Enter the sandbox and replace the single-threaded fence-exec shim. On error the shim must exit.
/// Mount targets and grants come from the trusted policy layer. Stdio is inherited.
pub fn fence_exec(policy: &LinuxFence, command: &FenceCommand) -> io::Error {
    match setup(policy, command) {
        Ok(()) => Command::new(&command.program)
            .args(&command.args)
            .env_clear()
            .envs(&command.env)
            .current_dir(&command.cwd)
            .exec(),
        Err(error) => error,
    }
}

fn setup(policy: &LinuxFence, command: &FenceCommand) -> io::Result<()> {
    if Platform::current() != Platform::Linux {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "sandbox is unavailable on WSL",
        ));
    }
    if !command.program.is_absolute() || !command.cwd.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "fence-exec requires an absolute executable and working directory",
        ));
    }
    if fs::read_dir("/proc/self/task")?.take(2).count() != 1 {
        return Err(io::Error::other("fence-exec must be single-threaded"));
    }
    let abi = landlock_abi()?;
    enter_namespace().map_err(|error| {
        io::Error::new(error.kind(), format!("sandbox mount namespace: {error}"))
    })?;
    for mount in &policy.mounts {
        apply_mount(mount).map_err(|error| {
            io::Error::new(error.kind(), format!("sandbox mount {mount:?}: {error}"))
        })?;
    }
    set_no_new_privs(true)?;
    set_capabilities(
        None,
        CapabilitySets {
            effective: CapabilitySet::empty(),
            permitted: CapabilitySet::empty(),
            inheritable: CapabilitySet::empty(),
        },
    )?;
    // SAFETY: close_range marks inherited fds above stderr close-on-exec, with no buffer arguments.
    if unsafe { libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 4_u32) } < 0 {
        return Err(io::Error::last_os_error());
    }
    restrict(&policy.rules, abi)
}

fn enter_namespace() -> io::Result<()> {
    let uid = getuid().as_raw();
    let gid = getgid().as_raw();
    // SAFETY: this shim is single-threaded and does not unshare its file descriptor table.
    unsafe { unshare_unsafe(UnshareFlags::NEWUSER) }?;
    fs::write("/proc/self/uid_map", format!("{uid} {uid} 1\n"))?;
    fs::write("/proc/self/setgroups", "deny\n")?;
    fs::write("/proc/self/gid_map", format!("{gid} {gid} 1\n"))?;
    // SAFETY: only the mount namespace changes; no descriptors are shared across threads.
    unsafe { unshare_unsafe(UnshareFlags::NEWNS) }?;
    mount_change(
        "/",
        MountPropagationFlags::PRIVATE | MountPropagationFlags::REC,
    )?;
    Ok(())
}

fn landlock_abi() -> io::Result<ABI> {
    // SAFETY: the version query takes no address or writable buffer.
    let version =
        unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, 0_usize, 0_usize, 1_u32) };
    Ok(match version {
        1 => ABI::V1,
        2 => ABI::V2,
        3 => ABI::V3,
        4 => ABI::V4,
        5 => ABI::V5,
        6.. => ABI::V6,
        _ => {
            return Err(io::Error::other(format!(
                "Landlock unavailable: {}",
                io::Error::last_os_error()
            )));
        }
    })
}

fn pin(path: &Path) -> io::Result<OwnedFd> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "sandbox policy requires absolute paths",
        ));
    }
    // No symlink may redirect a mount or a grant after the policy was assembled.
    Ok(openat2(
        CWD,
        path,
        OFlags::PATH | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )?)
}

fn apply_mount(mount: &Mount) -> io::Result<()> {
    let (detached, target) = match mount {
        Mount::Bind {
            source,
            target,
            read_only,
        } => {
            let source = pin(source)?;
            let target = pin(target)?;
            let detached = open_tree(
                &source,
                "",
                OpenTreeFlags::OPEN_TREE_CLONE
                    | OpenTreeFlags::OPEN_TREE_CLOEXEC
                    | OpenTreeFlags::AT_EMPTY_PATH,
            )?;
            if *read_only {
                readonly_mount(&detached)?;
            }
            (detached, target)
        }
        Mount::EmptyDirectory { path, mode } => {
            let target = pin(path)?;
            let setup = fsopen("tmpfs", FsOpenFlags::FSOPEN_CLOEXEC)?;
            fsconfig_set_string(&setup, "mode", format!("{:o}", mode & 0o7777))?;
            fsconfig_set_string(&setup, "size", "64k")?;
            fsconfig_create(&setup)?;
            let detached = fsmount(
                &setup,
                FsMountFlags::FSMOUNT_CLOEXEC,
                MountAttrFlags::MOUNT_ATTR_NOEXEC
                    | MountAttrFlags::MOUNT_ATTR_NODEV
                    | MountAttrFlags::MOUNT_ATTR_NOSUID,
            )?;
            (detached, target)
        }
    };
    move_mount(
        &detached,
        "",
        &target,
        "",
        MoveMountFlags::MOVE_MOUNT_F_EMPTY_PATH | MoveMountFlags::MOVE_MOUNT_T_EMPTY_PATH,
    )?;
    Ok(())
}

fn readonly_mount(mount: &OwnedFd) -> io::Result<()> {
    // The kernel's mount_attr has four u64 fields. Only set MOUNT_ATTR_RDONLY.
    let attr = [1_u64, 0, 0, 0];
    // SAFETY: mount is a live fd, the path is empty and NUL-terminated, and attr is the advertised
    // structure size. AT_EMPTY_PATH applies the operation to the detached mount.
    let result = unsafe {
        libc::syscall(
            libc::SYS_mount_setattr,
            mount.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH,
            attr.as_ptr(),
            size_of_val(&attr),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn restrict(grants: &[FileRule], abi: ABI) -> io::Result<()> {
    let all = AccessFs::from_all(abi);
    let mut rules = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(all)
        .map_err(io::Error::other)?;
    if abi == ABI::V6 {
        rules = rules
            .scope(Scope::AbstractUnixSocket | Scope::Signal)
            .map_err(io::Error::other)?;
    }
    let mut rules = rules.create().map_err(io::Error::other)?;
    for rule in grants {
        let fd = pin(&rule.path)?;
        let mut access = match rule.access {
            FileAccess::List => AccessFs::ReadDir.into(),
            FileAccess::Read => AccessFs::from_read(abi),
            FileAccess::HomeWrite => {
                all & !(AccessFs::RemoveFile
                    | AccessFs::RemoveDir
                    | AccessFs::MakeChar
                    | AccessFs::MakeBlock
                    | AccessFs::Refer)
            }
            FileAccess::ReadWriteFile => {
                (AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::Truncate) & all
            }
            FileAccess::Full => all,
        };
        if fstat(&fd)?.st_mode & libc::S_IFMT != libc::S_IFDIR {
            access &= AccessFs::from_file(abi);
        }
        if !access.is_empty() {
            rules = rules
                .add_rule(PathBeneath::new(fd, access))
                .map_err(io::Error::other)?;
        }
    }
    let result = rules.restrict_self().map_err(io::Error::other)?;
    if result.ruleset != RulesetStatus::FullyEnforced {
        return Err(io::Error::other("Landlock rules were not fully enforced"));
    }
    Ok(())
}
