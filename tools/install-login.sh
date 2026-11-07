#!/bin/sh
# Install Olo's own login screen (olo-login, in GDM's place).
#
#   tools/install-login.sh            install it beside GDM (asks for sudo)
#   tools/install-login.sh --switch   ...and make it the login screen from
#                                     the next start (GDM stays installed)
#   tools/install-login.sh --revert   GDM the login screen again
#
# Installs, as root, what a login screen's user (olo-greeter, not the
# person) can run and read: the programs its session and the person's
# need first (olo-login, olo-session, olo-app-server, olo-wlproxy,
# olo-greeter) into /usr/local/libexec/olo, Olo's data (icons, fonts,
# themes, wallpapers) into /usr/local/share/olo, its PAM services
# (data/pam: password, fingerprint, face, and its own session's), its
# user and folders (data/sysusers, data/tmpfiles) and its unit
# (data/systemd/olo-login.service, display-manager.service when
# switched). The person's session still runs their own programs (the
# login screen's entry names ~/.local/bin/olo-session).
#
# Switching changes what the computer starts into: it's for the person
# to choose, and --revert undoes it. Nothing is switched without
# --switch.
set -eu
cd "$(dirname "$0")/.."
as_root() {
    if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi
}

case "${1:-}" in
--revert)
    as_root systemctl disable olo-login.service
    as_root systemctl enable gdm.service
    echo "GDM is the login screen again from the next start."
    exit 0
    ;;
esac

cargo build --release -p olo-login -p olo-greeter -p olo-session -p app_server -p olo-wlproxy
LIBEXEC=/usr/local/libexec/olo
SHARE=/usr/local/share/olo
as_root install -d -m 755 "$LIBEXEC" "$SHARE"
for name in olo-login olo-greeter olo-session olo-app-server olo-wlproxy; do
    [ -x "target/release/$name" ] || continue
    # Under a temporary name, then renamed (a running one keeps its file).
    as_root install -m 755 "target/release/$name" "$LIBEXEC/.$name.new"
    as_root mv -f "$LIBEXEC/.$name.new" "$LIBEXEC/$name"
done
as_root cp -r data/icons data/brand data/fonts data/wallpapers "$SHARE/"
for theme in data/themes/*/; do
    theme=${theme%/}
    as_root mkdir -p "$SHARE/themes/${theme##*/}"
    as_root cp -r "$theme/icons" "$theme/brand" "$SHARE/themes/${theme##*/}/"
done
for service in olo-login olo-login-fingerprint olo-login-face olo-login-greeter; do
    as_root install -D -m 644 "data/pam/$service" "/etc/pam.d/$service"
done
as_root install -D -m 644 data/sysusers/olo-greeter.conf /etc/sysusers.d/olo-greeter.conf
as_root install -D -m 644 data/tmpfiles/olo-greeter.conf /etc/tmpfiles.d/olo-greeter.conf
as_root systemd-sysusers /etc/sysusers.d/olo-greeter.conf
as_root systemd-tmpfiles --create /etc/tmpfiles.d/olo-greeter.conf
as_root install -D -m 644 data/systemd/olo-login.service /etc/systemd/system/olo-login.service
as_root systemctl daemon-reload
echo "Olo's login screen is installed (not switched to)."

if [ "${1:-}" = --switch ]; then
    as_root systemctl disable gdm.service
    as_root systemctl enable olo-login.service
    echo "Olo's login screen starts from the next start; tools/install-login.sh --revert goes back to GDM."
fi
