# Olo's release built as on Fedora 44, the oldest Fedora a release is
# for: its programs link Fedora's own libraries and glibc.
FROM registry.fedoraproject.org/fedora:44
RUN dnf -y install gcc gcc-c++ clang clang-libs clang-devel llvm-devel mold nasm pkgconf-pkg-config make perl m4 git git-lfs \
      'pkgconfig(wayland-server)' 'pkgconfig(xkbcommon)' 'pkgconfig(libinput)' 'pkgconfig(libudev)' 'pkgconfig(gbm)' 'pkgconfig(egl)' \
      'pkgconfig(libdrm)' 'pkgconfig(libseat)' 'pkgconfig(pixman-1)' 'pkgconfig(libdisplay-info)' 'pkgconfig(freetype2)' \
      'pkgconfig(harfbuzz)' 'pkgconfig(fontconfig)' 'pkgconfig(zlib)' 'pkgconfig(liblzma)' 'pkgconfig(bzip2)' \
      'pkgconfig(libpipewire-0.3)' 'pkgconfig(libspa-0.2)' elfutils-libelf-devel pam-devel \
      zig zstd xz binutils findutils which file diffutils python3 cmake openssl-devel sqlite-devel \
    && dnf clean all
ARG UID=1000
ARG GID=1000
RUN groupadd -g $GID builder && useradd -m -u $UID -g $GID builder
USER builder
ENV PATH=/home/builder/.cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain nightly-2026-11-14 \
    && rustup target add --toolchain nightly-2026-11-14 x86_64-unknown-linux-musl aarch64-unknown-linux-musl \
    && rustup component add --toolchain nightly-2026-11-14 rustfmt clippy rust-src
