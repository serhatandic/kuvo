/// Everything that runs inside the Linux VM. Kept as plain shell so it's easy
/// to read and audit. Stage 1 is packed into the initramfs; the rest is
/// written to the shared folder on every launch, so updates need no reinstall.
enum GuestScripts {
    /// vsock ports the guest listens on.
    static let dockerPort: UInt32 = 2375
    static let powerOffPort: UInt32 = 2376
    static let trimPort: UInt32 = 2377

    /// Name lookups go to the Mac's DNS proxy on the NAT gateway. A VPN, firewall
    /// or content filter on the Mac can leave it silent; when it stays silent but a
    /// public resolver answers, those are added so the VM can still reach the
    /// internet. A working proxy is never overridden, so names only it knows keep
    /// resolving.
    static let dnsFallback = #"""
    resolves() { nslookup -timeout=2 -retry=1 dl-cdn.alpinelinux.org $1 >/dev/null 2>&1; }
    ensure_dns() {
        for i in 1 2 3 4 5; do resolves && return 0; sleep 1; done
        for ns in 1.1.1.1 8.8.8.8; do
            resolves $ns || continue
            printf 'nameserver 1.1.1.1\nnameserver 8.8.8.8\n' >> /etc/resolv.conf
            return 0
        done
        return 1
    }
    """#

    /// Stage 1, `/init` in the initramfs. Attaches kernel modules, installs
    /// Alpine + Docker onto the disk on first boot, then switches to it.
    static let stage1 = #"""
    #!/bin/sh
    /usr/bin/busybox --install -s
    export PATH=/usr/sbin:/usr/bin:/sbin:/bin
    mount -t proc proc /proc
    mount -t sysfs sysfs /sys
    mount -t devtmpfs devtmpfs /dev

    step() { echo "kuvo: $*"; }
    fail() { echo "kuvo: error: $*"; sleep 1; poweroff -f; }
    \#(dnsFallback)

    modprobe -a virtio_blk virtio_net virtiofs loop squashfs
    mkdir -p /host /.modloop /newroot
    mount -t virtiofs kuvo /host || fail "shared folder missing"
    mount -o loop,ro /host/modloop /.modloop || fail "modules missing"
    rm -rf /usr/lib/modules && ln -s /.modloop/modules /usr/lib/modules
    modprobe ext4

    if mount -t ext4 /dev/vda /newroot 2>/dev/null; then
        # Disks installed before the rename carry the old marker; never reformat those.
        [ -f /newroot/etc/kuvo-installed ] || [ -f /newroot/etc/terrarium-installed ] || { umount /newroot; NEEDS_INSTALL=1; }
    else
        NEEDS_INSTALL=1
    fi

    if [ -n "$NEEDS_INSTALL" ]; then
        step "Connecting to the network"
        ip link set lo up && ip link set eth0 up
        udhcpc -i eth0 -f -q -n -t 20 -T 1 -s /usr/share/udhcpc/default.script >/dev/null || fail "no network"
        ensure_dns || fail "the VM can't reach the internet. A VPN, firewall or content filter on this Mac may be blocking it"

        step "Preparing disk"
        mkdir -p /stage && mount -t tmpfs tmpfs /stage
        tar -xzf /host/rootfs.tar.gz -C /stage
        cp /etc/resolv.conf /stage/etc/
        for d in dev proc sys; do mount --bind /$d /stage/$d; done
        chroot /stage apk add --quiet --no-progress e2fsprogs || fail "could not fetch e2fsprogs"
        chroot /stage mkfs.ext4 -q -F -L kuvo /dev/vda || fail "could not format disk"
        for d in dev proc sys; do umount /stage/$d; done
        umount /stage

        step "Installing Alpine Linux"
        mount -t ext4 /dev/vda /newroot || fail "could not mount disk"
        tar -xzf /host/rootfs.tar.gz -C /newroot
        cp /etc/resolv.conf /newroot/etc/
        for d in dev proc sys; do mount --bind /$d /newroot/$d; done

        step "Installing Docker"
        chroot /newroot apk add --quiet --no-progress docker socat e2fsprogs \
            || fail "could not install Docker"
        ln -sf /mnt/host/inittab /newroot/etc/inittab
        for d in dev proc sys; do umount /newroot/$d; done
        touch /newroot/etc/kuvo-installed
        sync
        step "Starting Docker"
    fi

    mkdir -p /newroot/mnt/host /newroot/.modloop
    for m in /host:/mnt/host /.modloop:/.modloop /dev:/dev /proc:/proc /sys:/sys; do
        mount --move "${m%%:*}" "/newroot${m#*:}"
    done
    exec switch_root /newroot /sbin/init
    """#

    /// busybox init config for the installed system: supervises dockerd and
    /// the vsock relays, runs boot/shutdown hooks. The docker relay keeps a
    /// connection open after the client stops sending (`docker run` without
    /// stdin does that right away); socat's default closes it 0.5s later and
    /// cuts off the container's output.
    static let inittab = """
    ::sysinit:/bin/sh /mnt/host/boot.sh
    ::respawn:/bin/sh -c 'exec /usr/bin/dockerd --shutdown-timeout 3 >>/mnt/host/docker.log 2>&1'
    ::respawn:/usr/bin/socat -t 315360000 VSOCK-LISTEN:\(dockerPort),fork,reuseaddr UNIX-CONNECT:/var/run/docker.sock
    ::respawn:/usr/bin/socat VSOCK-LISTEN:\(powerOffPort),reuseaddr EXEC:/sbin/poweroff
    ::respawn:/usr/bin/socat VSOCK-LISTEN:\(trimPort),reuseaddr,fork SYSTEM:'sync; fstrim /'
    ::respawn:/usr/sbin/ntpd -n -p pool.ntp.org
    ::shutdown:/bin/sh /mnt/host/shutdown.sh

    """

    /// Runs once per boot, before dockerd starts.
    static let boot = #"""
    #!/bin/sh
    mount -o remount,rw /
    mkdir -p /dev/pts /dev/shm /run /sys/fs/cgroup /Users
    mount -t devpts devpts /dev/pts
    mount -t tmpfs shm /dev/shm
    mount -t tmpfs run /run
    mount -t cgroup2 cgroup2 /sys/fs/cgroup
    rm -rf /lib/modules && ln -s /.modloop/modules /lib/modules
    hostname kuvo

    for m in vmw_vsock_virtio_transport overlay bridge br_netfilter veth nf_tables nft_compat \
             nf_nat nf_conntrack iptable_nat iptable_filter xt_MASQUERADE xt_conntrack xt_addrtype; do
        modprobe "$m" 2>/dev/null
    done
    sysctl -qw net.ipv4.ip_forward=1

    ip link set lo up && ip link set eth0 up
    udhcpc -i eth0 -q -n -t 20 -T 1 -s /usr/share/udhcpc/default.script >/dev/null
    ip -4 -o addr show eth0 | awk '{ split($4, a, "/"); print a[1] }' > /mnt/host/ip
    # In the background, so a Mac that's offline doesn't hold up the engine.
    \#(dnsFallback)
    ensure_dns &

    # Same paths as on the Mac, so bind mounts like -v $PWD:/app just work.
    mount -t virtiofs users /Users

    # Run x86-64 binaries through Rosetta when the Mac provides it.
    mkdir -p /mnt/rosetta
    if mount -t virtiofs rosetta /mnt/rosetta 2>/dev/null; then
        modprobe binfmt_misc 2>/dev/null
        mount -t binfmt_misc binfmt_misc /proc/sys/fs/binfmt_misc 2>/dev/null
        printf '%s' ':rosetta:M::\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\x3e\x00:\xff\xff\xff\xff\xff\xfe\xfe\x00\xff\xff\xff\xff\xff\xff\xff\xff\xfe\xff\xff\xff:/mnt/rosetta/rosetta:CF' \
            > /proc/sys/fs/binfmt_misc/register
    fi

    # Hand blocks freed while Kuvo wasn't watching back to the Mac (see Engine.trim).
    fstrim / &
    """#

    static let shutdown = #"""
    #!/bin/sh
    killall dockerd 2>/dev/null
    for i in $(seq 40); do pidof dockerd >/dev/null || break; sleep 0.25; done
    sync
    """#
}
