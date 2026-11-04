#!/bin/bash
# Set up Olo on a Fedora machine (Fedora 44 or 45, x86_64 or aarch64;
# Workstation, or an atomic desktop such as Silverblue or Aurora), from a
# fresh clone:
#
#   git clone https://github.com/benmyles/olo && cd olo && tools/setup.sh
#
# Run it as yourself; it asks for sudo when it needs root. It says what
# machine it's on (Fedora's version, the processor, memory, graphics and
# login screen), installs what Olo needs (the compilers and libraries it
# builds with, a recent Rust through rustup, Git LFS and the files it
# keeps, the drivers and services a session uses, the dev tools' Python
# imaging), builds it, puts it on the login screen's session menu (GDM's
# or SDDM's) and installs its udev rules (tools/install-session.sh), then
# says what's left. Safe to run again: each step only does what isn't
# done yet. `--check` only reports.
set -euo pipefail
cd "$(dirname "$0")/.."
CHECK=0
[ "${1:-}" = "--check" ] && CHECK=1

STEPS=7
step=0
bold=$(tput bold 2>/dev/null || true)
plain=$(tput sgr0 2>/dev/null || true)
say() { step=$((step + 1)); echo; echo "${bold}[$step/$STEPS] $*${plain}"; }
ok() { echo "  ✓ $*"; }
todo() { echo "  • $*"; }
fail() { echo "  ✗ $*" >&2; exit 1; }

# What Olo needs, each as a check and the package that provides it
# (dnf takes capabilities such as pkgconfig(gbm), so a remix's own builds
# of a library count). Building: compilers, bindgen's libclang, the
# libraries Olo links (PipeWire's and SPA's headers for sound and screen
# capture) and Git LFS (the voice model built in, the themes' icons and
# the wallpapers are kept with it). Running: Mesa's drivers (app_server
# draws with EGL through GBM, the apps with Vulkan), what a session talks
# to, and the fonts its look is made of. Developing: Pillow for the
# system Python (tools/dev/tsession's screenshots, tools/journal.py).
NEEDS=(
    "cmd:cc gcc" "cmd:c++ gcc-c++" "cmd:clang clang" "lib:libclang.so clang-libs" "cmd:pkg-config pkgconf-pkg-config"
    "cmd:make make" "cmd:perl perl" "cmd:python3 python3" "cmd:m4 m4" "cmd:git git" "cmd:git-lfs git-lfs"
    "pc:wayland-server" "pc:xkbcommon" "pc:libinput" "pc:libudev" "pc:gbm" "pc:egl" "pc:libdrm" "pc:libseat"
    "pc:pixman-1" "pc:libdisplay-info" "pc:freetype2" "pc:harfbuzz" "pc:fontconfig" "pc:zlib" "pc:liblzma" "pc:bzip2"
    "pc:libpipewire-0.3" "pc:libspa-0.2"
    "glob:/usr/lib64/dri/*.so mesa-dri-drivers" "lib:libvulkan.so.1 vulkan-loader"
    "glob:/usr/share/vulkan/icd.d/*.json mesa-vulkan-drivers" "py:PIL python3-pillow"
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
        glob:*) compgen -G "$what" > /dev/null ;;
        py:*) /usr/bin/python3 -c "import $what" > /dev/null 2>&1 ;;
        font:*) fc-list "${what//_/ }" family 2>/dev/null | grep -q . ;;
    esac
}
# The oldest Rust that builds Olo: the newest `rust-version` any of its
# crates asks for (filmcraft's, 1.95; `cargo metadata` lists them), past
# edition 2024's let chains (1.88). Fedora 45's own is 1.98.
RUST_MIN=1.95

say "This machine"
[ "$(id -u)" = 0 ] && fail "run this as yourself, not root (it uses sudo when it must)"
. /etc/os-release
case " ${ID:-} ${ID_LIKE:-} " in
    *" fedora "*) ok "${PRETTY_NAME:-Fedora} on $(uname -m)" ;;
    *) fail "Olo is set up for Fedora; this is ${PRETTY_NAME:-unknown}" ;;
esac
case $(uname -m) in
    x86_64 | aarch64) ;;
    *) fail "Olo builds for x86_64 and aarch64; this is $(uname -m)" ;;
esac
# (A remix numbers its releases as Fedora does.)
release=${VERSION_ID%%.*}
if [[ $release =~ ^[0-9]+$ ]] && [ "$release" -lt 44 ]; then
    todo "Fedora $release is older than Olo is set up for (44 and 45): its libraries may be too old to build it"
fi
cpu=$(grep -m1 '^model name' /proc/cpuinfo 2> /dev/null | cut -d: -f2- | sed 's/^ *//' || true)
memory=$(awk '/^MemTotal:/ { printf "%.0f", $2 / 1048576 }' /proc/meminfo)
ok "$(nproc) cores${cpu:+ ($cpu)}, $memory GB of memory, $(df -h --output=avail . | tail -1 | tr -d ' ') free for the build"
# The login screen Olo's entry goes on.
greeter=$(basename "$(readlink -f /etc/systemd/system/display-manager.service 2> /dev/null)" .service)
case $greeter in
    gdm | sddm) ok "login screen: ${greeter^^}" ;;
    "" | display-manager) todo "no login screen is enabled: Olo is started from GDM's or SDDM's session menu" ;;
    *) todo "login screen: $greeter (Olo's entry is for GDM and SDDM, in /usr/share/wayland-sessions)" ;;
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

say "Git LFS's files"
# A clone made without Git LFS has its pointers where the voice model,
# the themes' icons and the wallpapers go; the build would take them in.
lfs_pointer() { head -c 40 "$1" 2> /dev/null | grep -q '^version https://git-lfs'; }
MODEL=vendor/deep_filter/models/DeepFilterNet3_onnx.tar.gz
lfs_missing() {
    if command -v git-lfs > /dev/null 2>&1; then
        # (Counted, not `grep -q`: with pipefail, grep leaving early
        # kills git-lfs with SIGPIPE, and the pipe says nothing's missing.)
        [ "$(git lfs ls-files 2> /dev/null | grep -c '^[0-9a-f]* - ' || true)" != 0 ]
    else
        lfs_pointer "$MODEL"
    fi
}
if [ -s "$MODEL" ] && ! lfs_pointer "$MODEL" && ! lfs_missing; then
    ok "all here ($(git lfs ls-files 2> /dev/null | wc -l) files)"
elif [ $CHECK = 1 ]; then
    todo "not fetched yet (git lfs install --local && git lfs pull)"
else
    git lfs install --local > /dev/null
    git lfs pull
    lfs_pointer "$MODEL" && fail "Git LFS's files didn't come: try git lfs pull"
    ok "fetched ($(git lfs ls-files | wc -l) files)"
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
    # (The live layout lint is left to the hooks, which run it on every
    # commit and push: on a machine being set up it's the login screen's
    # entry that matters, which comes last.)
    OLO_SKIP_UI_LINT=1 tools/install-session.sh ||
        fail "building or installing stopped (see above), so Olo isn't on the login screen yet: fix that and run tools/setup.sh again"
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
# Where the entry is (tools/install-session.sh: /usr/share's, where GDM
# and SDDM always look; an atomic system's in /usr/local/share), and
# whether the login screen reads that place: GDM reads /usr/local/share's
# only when its XDG_DATA_DIRS names it (systemd's environment, as a
# Flatpak's generator makes it).
entry=""
for dir in /usr/share/wayland-sessions /usr/local/share/wayland-sessions; do
    [ -f "$dir/olo.desktop" ] && entry=$dir/olo.desktop && break
done
case $greeter in gdm | sddm) on="${greeter^^}'s" ;; *) on="the" ;; esac
data_dirs=$(systemctl show-environment 2> /dev/null | sed -n 's/^XDG_DATA_DIRS=//p')
data_dirs=${data_dirs:-/usr/local/share:/usr/share}
if [ -z "$entry" ]; then
    todo "not on the session menu yet (no olo.desktop in /usr/share/wayland-sessions or /usr/local/share/wayland-sessions)"
elif exec_of=$(grep -m1 '^Exec=' "$entry" | cut -d= -f2) && [ ! -x "$exec_of" ]; then
    todo "$entry starts $exec_of, which isn't there: run tools/install-session.sh"
elif [ "$greeter" = gdm ] && [ "${entry#/usr/local/}" != "$entry" ] && [[ ":$data_dirs:" != *":/usr/local/share:"* && ":$data_dirs:" != *":/usr/local/share/:"* ]]; then
    todo "$entry is where GDM doesn't look here (its XDG_DATA_DIRS: $data_dirs): run tools/install-session.sh, which puts it in /usr/share/wayland-sessions"
else
    ok "Olo is on $on session menu ($entry, starting $exec_of)"
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
    case $greeter in
        gdm) echo "${bold}Done.${plain} Log out; on the login screen pick yourself, choose Olo Desktop with the gear at the bottom right, and sign in." ;;
        sddm) echo "${bold}Done.${plain} Log out; on the login screen choose Olo Desktop from the session menu at the bottom left, and sign in." ;;
        *) echo "${bold}Done.${plain} Log out, choose Olo Desktop on the login screen, and sign in." ;;
    esac
    echo "Later builds: Deskbar's green arrow installs them (or tools/install-session.sh)."
fi
