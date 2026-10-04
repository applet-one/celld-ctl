#!/bin/sh
# Install reusable host components. Instance configuration is deliberately separate.
set -eu
[ "$(id -u)" = 0 ] || { echo 'Run as root' >&2; exit 1; }
REPO=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
[ -x "$REPO/target/release/celld-ctl" ] || { echo 'Build: cargo build --release --locked' >&2; exit 1; }
command -v caddy >/dev/null
command -v sshd >/dev/null
getent passwd celld >/dev/null || useradd --system --home-dir /var/lib/celld --shell /usr/sbin/nologin celld
getent passwd cella-deploy >/dev/null || useradd --system --home-dir /var/empty/cella-deploy --shell /bin/sh cella-deploy
install -d -o root -g root -m 755 /var/empty/cella-deploy /etc/ssh/cella-deploy
install -d -o root -g root -m 700 /etc/celld-ctl /etc/celld/cells /var/backups/celld-ctl
install -d -o root -g root -m 755 /var/lib/celld /var/lib/celld-ctl /var/lib/celld-ctl/public /usr/local/lib/celld/releases
install -o root -g root -m 755 "$REPO/target/release/celld-ctl" /usr/local/bin/celld-ctl
install -d -o root -g root -m 755 /usr/local/libexec
install -o root -g root -m 755 "$REPO/scripts/celld-run" /usr/local/libexec/celld-run
install -o root -g root -m 755 "$REPO/scripts/celld-deploy-key" /usr/local/sbin/celld-deploy-key
install -o root -g root -m 644 "$REPO/examples/systemd/celld-cell@.service" /etc/systemd/system/celld-cell@.service
install -o root -g root -m 600 "$REPO/examples/ssh/sshd_config" /etc/celld-ctl/sshd_config
install -o root -g root -m 644 "$REPO/examples/systemd/cella-sshd.service" /etc/systemd/system/cella-sshd.service
[ ! -L /etc/celld-ctl/ssh-host-ed25519-key ] || { echo "Refusing symlink host key" >&2; exit 1; }
[ -e /etc/celld-ctl/ssh-host-ed25519-key ] || ssh-keygen -q -t ed25519 -N "" -C "cella transport host" -f /etc/celld-ctl/ssh-host-ed25519-key
chown root:root /etc/celld-ctl/ssh-host-ed25519-key
chmod 600 /etc/celld-ctl/ssh-host-ed25519-key
visudo -cf "$REPO/examples/ssh/cella-deploy.sudoers"
install -o root -g root -m 440 "$REPO/examples/ssh/cella-deploy.sudoers" /etc/sudoers.d/cella-deploy
[ ! -L /etc/ssh/cella-deploy/authorized_keys ] || { echo "Refusing symlink authorized_keys" >&2; exit 1; }
[ -e /etc/ssh/cella-deploy/authorized_keys ] || install -o root -g root -m 644 /dev/null /etc/ssh/cella-deploy/authorized_keys
chown root:root /etc/ssh/cella-deploy/authorized_keys
chmod 644 /etc/ssh/cella-deploy/authorized_keys
# Per-app journal namespaces bound verbose node logs without changing host journal policy.
install -d -o root -g root -m 755 /etc/systemd/journald@celld.conf.d
install -o root -g root -m 644 "$REPO/examples/systemd/journal-limits.conf" /etc/systemd/journald@celld.conf.d/limits.conf
[ -e /var/lib/celld-ctl/public/index.html ] || install -o root -g root -m 644 "$REPO/examples/caddy/index.html" /var/lib/celld-ctl/public/index.html
/usr/bin/install -d -o root -g root -m 755 /run/sshd
/usr/sbin/sshd -t -f /etc/celld-ctl/sshd_config
systemctl daemon-reload
systemctl enable --now cella-sshd
systemctl reload cella-sshd
printf '%s\n' 'Installed loopback-only deployment SSH on port 2222. Configure a private TCP access path for remote clients.' 'Configure /etc/celld-ctl/config.json and root-only /etc/celld/node.env before provisioning.'
