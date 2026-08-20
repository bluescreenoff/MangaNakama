# Machine setup (fresh clone / new machine)

1. Rust (GNU target — no Visual Studio needed):
   `rustup-init.exe --default-host x86_64-pc-windows-gnu --default-toolchain stable --profile minimal -y`
   (rustup-init from https://win.rustup.rs/x86_64)
2. Portable C toolchain (not committed, ~1 GB unpacked):
   download `w64devkit-x64-*.7z.exe` from https://github.com/skeeto/w64devkit/releases,
   extract so that `toolchain/w64devkit/bin/gcc.exe` exists.
3. Build: `./build.sh` (or `./build.sh --release`). Run: `./build.sh --run`.

Vendored: `vendor/libmypaint` (v1.6.1, patches logged in `vendor/PATCHES.md`).
