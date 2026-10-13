# Changelog

What changed in each release, for people using Kuvo Box. Add changes people would notice under
Unreleased, in plain words about what they'll see, as part of the change itself. Before a
release, Unreleased is renamed to the version; the release workflow publishes that section as
the release notes, and the app's update window shows it.

## Unreleased

- **Kuvo is now Kuvo Box,** the first of a family of Kuvo apps. Nothing else changes: the `kuvo` command, your containers, images and volumes, and your settings all stay as they are.

### Fixes

- Setting Kuvo Box up again after deleting its data folder no longer stops with "Docker didn't start in time".

## 0.1.3

### What's new

- **Images.** A new Images tab lists every image with its size and the containers using it. Pull an image from the toolbar (⌘N), and remove unused or untagged images in one step. When a pull leaves an older version behind, Kuvo Box tells you which tag it used to be.
- **Volumes.** A new Volumes tab shows every volume, its size, and which containers mount it and where. Leftovers from removed containers and old Compose projects are grouped under Unused, ready to remove.
- **Storage.** Kuvo Box › Storage… shows how much space Kuvo Box takes on your Mac and what's using it. Reclaim Space cleans up what you choose.
- **Sidebar tabs.** Switch between Containers, Images and Volumes at the top of the sidebar, or with ⌘1, ⌘2 and ⌘3.

### Fixes

- **Disk space now comes back to your Mac.** Deleting images, containers or volumes used to free space only inside the VM; Kuvo Box's disk on your Mac never shrank. Now the space comes back within seconds. Space built up under earlier versions is returned the first time 0.1.3 starts.
- **`docker run` output no longer stops early.** Containers run without `-i` or `-t` had their output cut off after half a second.
- **First launch works behind VPNs and firewalls that block the VM's DNS.** Kuvo Box falls back to public DNS servers when your Mac's DNS doesn't answer. If nothing answers, the error now says the VM can't reach the internet, instead of "could not fetch e2fsprogs".
- `kuvo new --image nginx`, without a tag, now pulls `latest` instead of every tag.

## 0.1.2

This release changes how Kuvo Box is packaged, not the app itself.

- **One download.** Each release now has a single disk image, `Kuvo.dmg`, plus the update feed. The zip and extra files are gone.
- **Updates install from the same disk image.** The in-app updater now downloads the same notarized `Kuvo.dmg` you'd download yourself.

## 0.1.1

### What's new

- **Use Kuvo Box for the `docker` command.** The first time Docker is up, Kuvo Box offers to make itself the default for the `docker` command by creating a Docker context named `kuvo`. Docker, Compose and other tools then use Kuvo Box in every terminal. To undo it, run `docker context use default`; if you have Docker Desktop, opening it switches back too.
- **A clearer empty window.** Until you run your first container, the window shows two steps: point `docker` at Kuvo Box, with a button that does it, then a command to run. The first step is checked off once it's done.

## 0.1.0

Kuvo Box's first public release. Kuvo Box runs the real Docker engine in a lightweight Linux VM on your Mac, with a native app to see what it's doing.

### Highlights

- **The real Docker engine.** `dockerd` from Alpine Linux, not a reimplementation, so the `docker` command, Compose, BuildKit and your scripts work unchanged.
- **Quick to set up.** The first launch downloads about 40 MB and sets up the VM in a minute or two. After that, Docker is ready in a few seconds.
- **Containers at a glance.** The sidebar lists every container, with Compose services grouped under their project. Start, stop, restart or remove them, one at a time or a whole project at once, with or without its volumes.
- **Live resources.** CPU, memory and network graphs update every second while the window is on screen, and stop when it's hidden, so Kuvo Box uses almost nothing in the background.
- **Logs.** Logs keep their colors. A Compose project's services are merged into one stream, each labeled in its own color. Long lines can wrap, and timestamps can be shown or hidden.
- **Terminal.** Press ⌃` for a shell in the selected container, next to its logs, or open it in your own terminal app.
- **Archive.** Stop a container and keep it for a while; Kuvo Box removes it after the period you choose in Settings.
- **Menu bar.** Every container one click away, with the window closed.
- **Works like Docker on Linux.** Published ports appear on `localhost`, `/Users` is shared at the same path inside the VM so bind mounts just work, and Intel (`linux/amd64`) images run through Rosetta.
- **The `kuvo` command.** Create containers to work in, run commands, open shells and copy files, with plain output and real exit codes that suit coding agents.
- **Light and dark.** Follows macOS, or pick one.
- **Automatic updates.** Kuvo Box checks for new versions once a day and asks before installing.
