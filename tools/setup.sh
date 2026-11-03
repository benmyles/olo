#!/bin/bash
# Set up Olo on a Fedora machine (x86_64 or aarch64; Workstation, or an
# atomic desktop such as Silverblue or Aurora), from a fresh clone:
#
#   git clone https://github.com/benmyles/olo && cd olo && tools/setup.sh
#
# Run it as yourself; it asks for sudo when it needs root. It checks and
# installs what Olo needs (packages, a recent Rust), builds it, puts it
# on the login screen's session menu and installs its udev rules
# (tools/install-session.sh), then says what's left. Safe to run again:
# each step only does what isn't done yet. `--check` only reports.
set -euo pipefail
cd "$(dirname "$0")/.."
CHECK=0
[ "${1:-}" = "--check" ] && CHECK=1

STEPS=6
step=0
bold=$(tput bold 2>/dev/null || true)
plain=$(tput sgr0 2>/dev/null || true)
say() { step=$((step + 1)); echo; echo "${bold}[$step/$STEPS] $*${plain}"; }
ok() { echo "  ✓ $*"; }
todo() { echo "  • $*"; }
fail() { echo "  ✗ $*" >&2; exit 1; }

# What Olo needs, each as a check and the package that provides it
# (dnf takes capabilities such as pkgconfig(gbm), so a remix's own builds
# of a library count). Building: compilers, bindgen's libclang and the
# libraries Olo links. Running: what a session talks to, and the fonts
# its look is made of.
NEEDS=(
    "cmd:cc gcc" "cmd:c++ gcc-c++" "cmd:clang clang" "lib:libclang.so clang-libs" "cmd:pkg-config pkgconf-pkg-config"
    "cmd:make make" "cmd:perl perl" "cmd:python3 python3" "cmd:m4 m4" "cmd:git git"
    "pc:wayland-server" "pc:xkbcommon" "pc:libinput" "pc:libudev" "pc:gbm" "pc:egl" "pc:libdrm" "pc:libseat"
    "pc:pixman-1" "pc:libdisplay-info" "pc:freetype2" "pc:harfbuzz" "pc:fontconfig" "pc:zlib" "pc:liblzma" "pc:bzip2"
    "file:/usr/libexec/fprintd fprintd" "cmd:pipewire pipewire"
    "cmd:wireplumber wireplumber" "file:/usr/libexec/xdg-desktop-portal xdg-desktop-portal"
    "file:/usr/libexec/xdg-desktop-portal-gtk xdg-desktop-portal-gtk" "cmd:gnome-keyring-daemon gnome-keyring"
    "cmd:pkexec polkit" "cmd:NetworkManager NetworkManager" "file:/usr/libexec/bluetooth/bluetoothd bluez"
    "font:Nimbus_Sans urw-base35-nimbus-sans-fonts" "font:Noto_Sans google-noto-sans-fonts"
    "font:Noto_Sans_Mono google-noto-sans-mono-fonts" "font:Noto_Sans_CJK_JP google-noto-sans-cjk-fonts"
    "font:Noto_Sans_Symbols google-noto-sans-symbols-fonts" "font:DejaVu_Sans dejavu-sans-fonts"
    "font:DejaVu_Sans_Mono dejavu-sans-mono-fonts" "font:Liberation_Sans liberation-sans-fonts"
    "font:Liberation_Mono liberation-mono-fonts" "font:JetBrains_Mono jetbrains-mono-fonts"
)
have_it() {
    local what=${1#*:}
    case $1 in
        cmd:*) command -v "$what" > /dev/null 2>&1 ;;
        file:*) [ -e "$what" ] ;;
        lib:*) compgen -G "/usr/lib64/$what*" > /dev/null || compgen -G "/usr/lib/$what*" > /dev/null ;;
        pc:*) command -v pkg-config > /dev/null 2>&1 && pkg-config --exists "$what" ;;
        font:*) fc-list "${what//_/ }" family 2>/dev/null | grep -q . ;;
    esac
}
# The oldest Rust that builds Olo (let chains, edition 2024).
RUST_MIN=1.88

say "This machine"
[ "$(id -u)" = 0 ] && fail "run this as yourself, not root (it uses sudo when it must)"
. /etc/os-release
case " ${ID:-} ${ID_LIKE:-} " in
    *" fedora "*) ok "${PRETTY_NAME:-Fedora} on $(uname -m)" ;;
    *) fail "Olo is set up for Fedora; this is ${PRETTY_NAME:-unknown}" ;;
esac
ATOMIC=0
if [ -e /run/ostree-booted ]; then
    ATOMIC=1
    ok "an atomic system: packages are layered with rpm-ostree"
fi
if [ -d /sys/class/drm ]; then
    for card in /sys/class/drm/card[0-9]*; do
        [ -e "$card/device/driver" ] || continue
        case "$card" in *-*) continue ;; esac
        driver=$(basename "$(readlink -f "$card/device/driver")")
        boot=""
        [ "$(cat "$card/device/boot_vga" 2>/dev/null)" = 1 ] && boot=" (boot GPU: Olo draws with it)"
        ok "graphics: $(basename "$card") $driver$boot"
        if [ "$driver" = nvidia ] && [ "$(cat /sys/module/nvidia_drm/parameters/modeset 2>/dev/null)" != Y ]; then
            todo "NVIDIA's driver needs kernel mode setting: add nvidia-drm.modeset=1 to the kernel command line"
        fi
    done
fi

say "Packages"
missing=()
for need in "${NEEDS[@]}"; do
    read -r check package <<< "$need"
    have_it "$check" && continue
    [ -n "$package" ] || package="pkgconfig(${check#pc:})"
    missing+=("$package")
done
if [ ${#missing[@]} = 0 ]; then
    ok "all ${#NEEDS[@]} things it needs to build and run are there"
elif [ $CHECK = 1 ]; then
    todo "missing: ${missing[*]}"
elif [ $ATOMIC = 1 ]; then
    echo "  layering ${#missing[@]} packages: ${missing[*]}"
    sudo rpm-ostree install --idempotent --allow-inactive "${missing[@]}"
    echo
    echo "Restart into the new deployment, then run tools/setup.sh again."
    exit 0
else
    echo "  installing ${#missing[@]} packages: ${missing[*]}"
    sudo dnf install -y "${missing[@]}"
    ok "installed"
fi

say "Rust"
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
version_ok() { [ "$(printf '%s\n%s\n' "$RUST_MIN" "$1" | sort -V | head -1)" = "$RUST_MIN" ]; }
have=$(rustc --version 2>/dev/null | awk '{print $2}' || true)
if [ -n "$have" ] && version_ok "$have"; then
    ok "rustc $have"
elif [ $CHECK = 1 ]; then
    todo "needs Rust $RUST_MIN or newer (have: ${have:-none})"
elif command -v rustup > /dev/null 2>&1 && rustup show > /dev/null 2>&1; then
    rustup update stable
    ok "rustc $(rustc --version | awk '{print $2}')"
else
    command -v rustup-init > /dev/null 2>&1 || sudo dnf install -y rustup
    rustup-init -y --profile minimal --no-modify-path
    . "$HOME/.cargo/env"
    ok "rustc $(rustc --version | awk '{print $2}') (rustup, in ~/.cargo)"
fi

say "Building and installing"
if [ $CHECK = 1 ]; then
    if [ -x "$HOME/.local/bin/olo-session" ]; then ok "installed in ~/.local/bin"; else todo "not built and installed yet"; fi
else
    tools/install-session.sh
    ok "built, and installed in ~/.local/bin"
fi
# Builds leave the last build's artifacts behind under target/ (hundreds
# of GB in a week): a daily sweep keeps only what a recent build used.
if systemctl --user is-enabled olo-sweep.timer > /dev/null 2>&1; then
    ok "target/ swept daily (olo-sweep.timer)"
elif [ $CHECK = 1 ]; then
    todo "no daily sweep of target/ yet (tools/dev/sweep --install)"
else
    tools/dev/sweep --install > /dev/null
    ok "target/ swept daily (olo-sweep.timer)"
fi
# Every commit formats what it changes and lints the UI, every push lints
# the apps it changed live (tools/hooks).
if [ -L "$(git rev-parse --git-path hooks)/pre-commit" ]; then
    ok "commits formatted and their UI linted (tools/hooks)"
elif [ $CHECK = 1 ]; then
    todo "no formatting and layout lint on commits yet (tools/hooks/install)"
else
    tools/hooks/install > /dev/null
    ok "commits formatted and their UI linted (tools/hooks)"
fi
# kache caches rustc's outputs (~/.cargo/config.toml names it as the
# wrapper); its login service keeps the cache under its size.
if command -v kache > /dev/null 2>&1; then
    if kache daemon --json 2>/dev/null | grep -q '"service": *"installed"' || systemctl --user is-enabled kache.service > /dev/null 2>&1; then
        ok "kache's daemon runs at login"
    elif [ $CHECK = 1 ]; then
        todo "kache's daemon isn't a login service yet (kache daemon install)"
    else
        kache daemon install > /dev/null 2>&1 && ok "kache's daemon runs at login" || todo "kache daemon install failed; run it by hand"
    fi
fi

say "Login screen"
if [ -f /usr/local/share/wayland-sessions/olo.desktop ]; then
    ok "Olo is on the session menu ($(grep -m1 '^Exec=' /usr/local/share/wayland-sessions/olo.desktop | cut -d= -f2))"
else
    todo "not on the session menu yet"
fi
if [ -f /etc/udev/rules.d/70-olo-displays.rules ]; then
    ok "display brightness rules installed"
else
    todo "display brightness rules not installed"
fi

say "Sign-in and unlocking"
if command -v fprintd-list > /dev/null 2>&1 && fprintd-list "$(id -un)" 2>/dev/null | grep -q 'finger'; then
    ok "fingerprints enrolled: the lock screen takes them"
else
    todo "no fingerprints enrolled: Preferences ▸ Touch ID enrols them once you're in"
fi
echo
if [ $CHECK = 1 ]; then
    echo "Checked only; run without --check to set up."
else
    echo "${bold}Done.${plain} Log out, choose Olo on the login screen, and sign in."
    echo "Later builds: Deskbar's green arrow installs them (or tools/install-session.sh)."
fi
