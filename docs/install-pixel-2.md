# Installing Pixie on a Pixel 2

Pixie boots on the Pixel 2 (codename **walleye**) from a single `boot.img`.
The safe way to try it is `fastboot boot`: it runs Pixie once from RAM and
leaves Android installed. Restarting the phone takes you back to Android.

## Before you start

- **Bootloader must be unlockable.** Only Pixel 2 phones bought from Google can
  unlock; Verizon ones cannot. Check Settings → System → Developer options →
  *OEM unlocking*. If that switch is greyed out, Pixie cannot run on this phone.
- **Unlocking erases the phone.** Back up anything you want to keep.
- On your computer, install:
  - [Android platform tools](https://developer.android.com/tools/releases/platform-tools) (`adb`, `fastboot`)
  - [Rust](https://rustup.rs), then `rustup target add aarch64-unknown-linux-musl`

## 1. Get the stock boot image

Pixie reuses Google's Pixel 2 kernel, taken from the official factory image.

1. On the phone, note Settings → About phone → *Build number*.
2. Download the matching **walleye** factory image from
   <https://developers.google.com/android/images#walleye>
   (Android 11 is the last release; use that if unsure).
3. Unzip it, then unzip the `image-walleye-*.zip` inside. Keep `boot.img`.

## 2. Build Pixie

From this repository:

```sh
cargo build --release --target aarch64-unknown-linux-musl -p pixie-init
cargo run --release -p pixie-image -- repack --stock path/to/boot.img --out pixie-boot.img
```

`repack` keeps Google's kernel and device trees, renames the kernel's
`skip_initramfs` parameter so the Pixel 2 bootloader cannot bypass Pixie, and
replaces the ramdisk with Pixie. It should print `patched 1 skip_initramfs`
(or more); if it prints 0, stop and report it.

`cargo run --release -p pixie-image -- info pixie-boot.img` shows what is inside.

## 3. Unlock the bootloader (once)

1. Settings → About phone → tap *Build number* 7 times to enable Developer options.
2. Developer options → turn on *OEM unlocking* and *USB debugging*.
3. Connect USB, then:

```sh
adb reboot bootloader
fastboot flashing unlock     # confirm on the phone with volume + power; this ERASES the phone
```

The phone resets and reboots into Android. Set it up quickly (skip accounts),
re-enable USB debugging, then `adb reboot bootloader` again.

## 4. Boot Pixie

```sh
fastboot boot pixie-boot.img
```

After the unlocked-bootloader warning, the Pixie screen appears: a status bar
(time, battery), the conversation, and button hints.

- **Volume up**: take a photo (test pattern for now)
- **Volume down**: battery level
- **Power**: time

### Typing to Pixie over USB

Pixie turns the USB port into a serial line (if the kernel supports the USB
ACM gadget; the screen says so if it does not).

- macOS: `screen /dev/tty.usbmodem* 115200`
- Linux: `screen /dev/ttyACM0 115200`
- Windows: PuTTY on the new COM port, 115200 baud

Type `help`. `reboot` returns to Android, `bootloader` returns to fastboot,
`poweroff` turns the phone off.

## If something goes wrong

- **Black screen or frozen:** hold Power for about 10 seconds to force a
  restart. Because you used `fastboot boot`, the phone comes back to Android.
- **The phone boots straight into Android:** the bootloader skipped Pixie's
  ramdisk. Run `pixie-image info` on your `pixie-boot.img` and check the
  `skip_initramfs` count is 0.
- **Restoring stock completely:** `fastboot flash boot boot.img` with the
  stock image, or flash the whole factory image with its `flash-all` script.
  `fastboot flashing lock` relocks (and erases) the phone.

Only use `fastboot flash boot pixie-boot.img` (permanent) once `fastboot boot`
works reliably.

## Building the kernel from source (Linux, optional)

`scripts/build-walleye-kernel.sh` builds the Pixel 2 kernel from LineageOS
source with Pixie's settings (devtmpfs, USB serial gadget, initramfs always
used) and writes `out/walleye/pixie-walleye-boot.img`. Use this instead of
steps 1–2 if you want the USB serial line guaranteed or a newer kernel.

## What is not there yet

- The camera is a fake that saves a colour-bar image; the real camera needs
  a driver skill for the Pixel's camera hardware.
- Touch input, Wi-Fi, calls and the on-device model (Gemma 4 E2B) are next.
  The model and guardrail are keyword stand-ins.
