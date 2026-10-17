#!/bin/sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "Run with sudo: sudo ./deploy/install.sh [path/to/sm0sh]" >&2
    exit 1
fi
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
binary=${1:-"$script_dir/../target/release/sm0sh"}
[ -f "$binary" ] || { echo "Binary not found: $binary" >&2; exit 1; }
"$binary" --version

if ! id sm0sh >/dev/null 2>&1; then
    useradd --system --home-dir /var/lib/sm0sh --shell /usr/sbin/nologin sm0sh
fi
install -d -m 0750 -o sm0sh -g sm0sh /var/lib/sm0sh
install -d -m 0750 -o root -g sm0sh /etc/sm0sh
install -m 0755 -o root -g root "$binary" /usr/local/bin/sm0sh
if [ ! -e /etc/sm0sh/sm0sh.toml ]; then
    install -m 0640 -o root -g sm0sh "$script_dir/sm0sh.toml" /etc/sm0sh/sm0sh.toml
fi
install -m 0644 -o root -g root "$script_dir/sm0sh.service" /etc/systemd/system/sm0sh.service
systemctl daemon-reload
echo "Installed. Edit /etc/sm0sh/sm0sh.toml, then run:"
echo "  sudo -u sm0sh /usr/local/bin/sm0sh --config /etc/sm0sh/sm0sh.toml --check-config"
echo "  sudo systemctl enable --now sm0sh"

