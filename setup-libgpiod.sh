#!/usr/bin/env bash

set -euxo pipefail

apt-get update
apt-get --assume-yes install --no-install-recommends build-essential autoconf-archive libtool pkg-config wget xz-utils ca-certificates

LIBGPIOD_VERSION="${LIBGPIOD_VERSION:-2.1.3}"
DEB_ARCH="${CROSS_DEB_ARCH:-$(dpkg --print-architecture)}"

case "${DEB_ARCH}" in
  armhf) GNU_TRIPLE=arm-linux-gnueabihf ;;
  arm64) GNU_TRIPLE=aarch64-linux-gnu ;;
  amd64) GNU_TRIPLE=x86_64-linux-gnu ;;
  i386)  GNU_TRIPLE=i686-linux-gnu ;;
  *)     GNU_TRIPLE=$(dpkg-architecture -qDEB_TARGET_GNU_TYPE 2>/dev/null || dpkg-architecture -qDEB_HOST_GNU_TYPE) ;;
esac

cd /tmp
wget -q "https://mirrors.edge.kernel.org/pub/software/libs/libgpiod/libgpiod-${LIBGPIOD_VERSION}.tar.xz"
tar -xf "libgpiod-${LIBGPIOD_VERSION}.tar.xz"
cd "libgpiod-${LIBGPIOD_VERSION}"

./configure \
  --host="${GNU_TRIPLE}" \
  --prefix="/usr/${GNU_TRIPLE}" \
  --libdir="/usr/${GNU_TRIPLE}/lib" \
  --includedir="/usr/${GNU_TRIPLE}/include" \
  --enable-tools=no \
  --enable-static \
  --disable-shared

make -j"$(nproc)"
make install

mkdir -p "/usr/lib/${GNU_TRIPLE}/pkgconfig"
cp "/usr/${GNU_TRIPLE}/lib/pkgconfig/"*.pc "/usr/lib/${GNU_TRIPLE}/pkgconfig/"

echo "/usr/${GNU_TRIPLE}/lib" > "/etc/ld.so.conf.d/libgpiod-${GNU_TRIPLE}.conf"
ldconfig

rm -rf "/tmp/libgpiod-${LIBGPIOD_VERSION}"*
