#!/bin/sh
# Install Olo's own login screen (olo-login, in GDM's place).
#
#   tools/install-login.sh            install it beside GDM (asks for sudo)
#   tools/install-login.sh --switch   ...and make it the login screen from
#                                     the next start (GDM stays installed)
#   tools/install-login.sh --revert   GDM the login screen again
#   tools/install-login.sh --plymouth          Olo's boot splash too
#   tools/install-login.sh --revert-plymouth   Fedora's boot splash again
#
# Installs, as root, what a login screen's user (olo-greeter, not the
# person) can run and read: the programs its session and the person's
# need first (olo-login, olo-session, olo-app-server, olo-wlproxy,
# olo-greeter) into /usr/local/libexec/olo, Olo's data (icons, fonts,
# themes, wallpapers) into /usr/local/share/olo, its PAM services
# (data/pam: password, fingerprint, face, and its own session's) and the
# PAM module the password one names (pam_olo_keyring: the password
# unlocks Olo's keyring), its
# user and folders (data/sysusers, data/tmpfiles) and its unit
# (data/systemd/olo-login.service, display-manager.service when
# switched). The person's session still runs their own programs (the
# login screen's entry names ~/.local/bin/olo-session).
#
# Switching changes what the computer starts into: it's for the person
# to choose, and --revert undoes it. Nothing is switched without
# --switch.
#
# --plymouth makes Olo's boot splash (crates/app_server/src/plymouth.rs)
# at the scale the computer's own screen is at in Olo, as the person
# running this (their settings), installs it as Plymouth's theme, tells
# Plymouth to draw device pixels (DeviceScale=1: the theme's pictures
# are made for the screen, never stretched) and makes the start-up image
# (initramfs) again, the splash being in it. Run it again after changing
# that screen's scale. --revert-plymouth puts plymouthd.conf back as it
# was and makes the start-up image again.
set -eu
cd "$(dirname "$0")/.."
as_root() {
    if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi
}

PLYMOUTH_CONF=/etc/plymouth/plymouthd.conf
PLYMOUTH_THEME=/usr/share/plymouth/themes/olo

case "${1:-}" in
--revert)
    as_root systemctl disable olo-login.service
    as_root systemctl enable gdm.service
    echo "GDM is the login screen again from the next start."
    exit 0
    ;;
--plymouth)
    cargo build --release -p app_server
    made=$(mktemp -d /var/tmp/olo-plymouth.XXXXXX)
    trap 'rm -rf "$made"' EXIT
    target/release/olo-app-server --plymouth-theme "$made"
    as_root rm -rf "$PLYMOUTH_THEME"
    as_root install -d -m 755 "$PLYMOUTH_THEME"
    as_root install -m 644 "$made"/* "$PLYMOUTH_THEME/"
    # (The first time, what was there kept for --revert-plymouth.)
    if [ -e "$PLYMOUTH_CONF" ] && [ ! -e "$PLYMOUTH_CONF.before-olo" ]; then
        as_root cp -p "$PLYMOUTH_CONF" "$PLYMOUTH_CONF.before-olo"
    fi
    printf '%s\n' "# Olo's boot splash (tools/install-login.sh --plymouth;" \
        "# --revert-plymouth puts back what was here)." \
        "[Daemon]" "Theme=olo" "DeviceScale=1" | as_root tee "$PLYMOUTH_CONF" >/dev/null
    as_root dracut --force --regenerate-all
    echo "Olo's boot splash shows from the next start; tools/install-login.sh --revert-plymouth goes back."
    exit 0
    ;;
--revert-plymouth)
    if [ -e "$PLYMOUTH_CONF.before-olo" ]; then
        as_root mv -f "$PLYMOUTH_CONF.before-olo" "$PLYMOUTH_CONF"
    else
        as_root rm -f "$PLYMOUTH_CONF"
    fi
    as_root rm -rf "$PLYMOUTH_THEME"
    as_root dracut --force --regenerate-all
    echo "Fedora's boot splash shows again from the next start."
    exit 0
    ;;
esac

cargo build --release -p olo-login -p olo-greeter -p olo-session -p app_server -p olo-wlproxy -p olo-pam-keyring
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
# The login password unlocks Olo's keyring (olo-login's PAM service names it).
as_root install -D -m 755 target/release/libpam_olo_keyring.so /usr/lib64/security/pam_olo_keyring.so
for service in olo-login olo-login-fingerprint olo-login-face olo-login-greeter; do
    as_root install -D -m 644 "data/pam/$service" "/etc/pam.d/$service"
done
as_root install -D -m 644 data/sysusers/olo-greeter.conf /etc/sysusers.d/olo-greeter.conf
as_root install -D -m 644 data/tmpfiles/olo-greeter.conf /etc/tmpfiles.d/olo-greeter.conf
as_root systemd-sysusers /etc/sysusers.d/olo-greeter.conf
as_root systemd-tmpfiles --create /etc/tmpfiles.d/olo-greeter.conf
# A display manager to SELinux, as GDM is: olo-login runs as xdm_t (from
# which the person's session may start, through the system's olo-session),
# its folders the display manager's (Olo Setup's plan.rs has the same).
if [ -e /sys/fs/selinux/enforce ] && command -v semanage >/dev/null; then
    for rule in "xdm_exec_t /usr/local/libexec/olo/olo-login" "xdm_var_run_t /run/olo-login(/.*)?" "xdm_var_lib_t /var/lib/olo-greeter(/.*)?"; do
        kind=${rule%% *}; path=${rule#* }
        as_root semanage fcontext -a -t "$kind" "$path" 2>/dev/null || as_root semanage fcontext -m -t "$kind" "$path"
    done
    as_root restorecon -R "$LIBEXEC" /run/olo-login /var/lib/olo-greeter
fi
as_root install -D -m 644 data/systemd/olo-login.service /etc/systemd/system/olo-login.service
as_root systemctl daemon-reload
echo "Olo's login screen is installed (not switched to)."

if [ "${1:-}" = --switch ]; then
    as_root systemctl disable gdm.service
    as_root systemctl enable olo-login.service
    echo "Olo's login screen starts from the next start; tools/install-login.sh --revert goes back to GDM."
fi
