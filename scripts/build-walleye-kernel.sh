#!/usr/bin/env bash
# Builds a Pixie boot image for the Pixel 2 (walleye) with a kernel compiled
# from LineageOS source, configured for Pixie (devtmpfs, USB serial gadget,
# initramfs always used). Linux only; needs clang, lld, aarch64 binutils,
# bc, bison, flex, libssl-dev, dtc and lz4.
#
# Usage: scripts/build-walleye-kernel.sh [WORKDIR]
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(realpath -m "${1:-$root/out/walleye}")
src="$work/android_kernel_google_wahoo"
obj="$work/kernel-obj"
# lineage-22.2 as of 2026-10-01; pinned so builds are reproducible.
commit=ddfd598a

mkdir -p "$work"
if [ ! -d "$src" ]; then
  git clone --depth 1 -b lineage-22.2 https://github.com/LineageOS/android_kernel_google_wahoo "$src"
fi
git -C "$src" rev-parse --short HEAD | grep -q "^$commit" || echo "note: kernel source is not at pinned commit $commit"

export ARCH=arm64 SUBARCH=arm64 CROSS_COMPILE=aarch64-linux-gnu- CLANG_TRIPLE=aarch64-linux-gnu-
make -C "$src" O="$obj" CC=clang lineageos_muskie_defconfig
"$src/scripts/config" --file "$obj/.config" \
  -e DEVTMPFS -e DEVTMPFS_MOUNT \
  -e USB_CONFIGFS_SERIAL -e USB_CONFIGFS_ACM \
  -e RD_GZIP -e INITRAMFS_IGNORE_SKIP_FLAG \
  -d CC_WERROR -d DEBUG_INFO \
  --set-str LOCALVERSION "-pixie"
make -C "$src" O="$obj" CC=clang olddefconfig
make -C "$src" O="$obj" CC=clang -j"$(nproc)" Image.lz4-dtb

cd "$root"
cargo build --release --target aarch64-unknown-linux-musl -p pixie-init
cargo run --release -p pixie-image -- build \
  --kernel "$obj/arch/arm64/boot/Image.lz4-dtb" \
  --out "$work/pixie-walleye-boot.img"
