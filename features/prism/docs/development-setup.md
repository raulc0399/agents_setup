# Prism development prerequisites

## Observed host

Checked on 2026-11-08 in `/workspace/agents_setup`: Debian 12 (bookworm),
`aarch64`, user `node` (UID 1000). `$HOME` is `/home/agent`, a symlink to
`/home/node`. These observations describe this development container, not a
supported-platform declaration.

| Component | Observed state |
| --- | --- |
| Rust | Installed user-locally; exact default toolchain `1.99.1-aarch64-unknown-linux-gnu` |
| rustc | `1.99.1 (58cf8f9c3 2026-10-13)` |
| Cargo | `1.99.1 (5f94df478 2026-08-27)` |
| rustfmt | `1.10.0-stable (58cf8f9c35 2026-10-13)` |
| Clippy | `0.1.99 (58cf8f9c35 2026-10-13)` |
| C compiler/linker, archiver and libc headers | User-local Zig `0.17.1` with Clang/LLVM `22.1.8`; C and Rust executable probes pass |
| Rust/Zig adapter | User-local `cargo-zigbuild 0.24.1` |
| pkg-config | Absent; optional for the selected bundled SQLite/rustls dependency approach |
| tmux | `3.3a` |
| Codex CLI | `codex-cli 0.167.0` |
| Terminal | `TERM=screen-256color`; matching `infocmp` entry exists; command stdout is not a TTY |

CLI version checks do not establish authentication, native plugin activation,
conversation binding, transport capabilities, or successful question delivery.
The terminal entry does not verify mouse input, resizing, or terminal cleanup.
Either Codex CLI or Claude Code CLI is sufficient; the check script requires at least one.

## Install development tools

On Debian, run the following as an administrator:

```sh
apt-get update
apt-get install -y build-essential pkg-config
```

This container has `apt-get`, but its package database and installation paths
are root-owned and neither `sudo` nor `doas` is installed. No administrator
credentials are available to this task. The supported Zig fallback below was
installed user-locally instead; no system package tree was modified.

Install the pinned Rust toolchain using the [official rustup installer](https://rust-lang.org/tools/install/):

```sh
(
    set -eu
    prism_install_dir=$(mktemp -d "${TMPDIR:-/tmp}/prism-rustup.XXXXXXXX")
    trap 'rm -rf "$prism_install_dir"' EXIT HUP INT TERM
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$prism_install_dir/rustup-init.sh" &&
        sh "$prism_install_dir/rustup-init.sh" -y --profile minimal --default-toolchain 1.99.1 --component rustfmt,clippy --no-modify-path
)
. "$HOME/.cargo/env"
```

For an existing rustup installation:

```sh
rustup toolchain install 1.99.1 --profile minimal --component rustfmt,clippy
```

The observed setup selected `rustup default 1.99.1` for the current user.
The repository's Rust crate pins the same version in
`features/prism/rust-toolchain.toml`. The installer
was run with `--no-modify-path`; source `$HOME/.cargo/env` in each build shell.
No global CLI configuration was changed by the Rust setup.

## User-local compiler fallback on this host

The official [Zig download index](https://ziglang.org/download/index.json)
identified stable `0.17.1`. Its aarch64 Linux archive was downloaded over HTTPS
and matched SHA-256
`a1546d5646eb184437918d6635ca6b762a0e889eac09cef720ee7a1ed3fe7795`.
Zig supplies a compiler, linker, archiver and libc headers without root access.

```sh
(
    set -eu
    prism_install_dir=$(mktemp -d "${TMPDIR:-/tmp}/prism-zig.XXXXXXXX")
    trap 'rm -rf "$prism_install_dir"' EXIT HUP INT TERM
    curl --proto '=https' --tlsv1.2 -sSf https://ziglang.org/download/0.17.1/zig-aarch64-linux-0.17.1.tar.xz -o "$prism_install_dir/zig.tar.xz"
    printf '%s  %s\n' 'a1546d5646eb184437918d6635ca6b762a0e889eac09cef720ee7a1ed3fe7795' "$prism_install_dir/zig.tar.xz" | sha256sum -c -
    python3 - "$prism_install_dir/zig.tar.xz" "$prism_install_dir/zig.tar" <<'PY'
import lzma
import shutil
import sys

with lzma.open(sys.argv[1], "rb") as source, open(sys.argv[2], "wb") as target:
    shutil.copyfileobj(source, target)
PY
    mkdir -p "$HOME/.local/opt" "$HOME/.local/bin"
    tar -xf "$prism_install_dir/zig.tar" -C "$HOME/.local/opt"
    uv tool install cargo-zigbuild==0.24.1 --with ziglang==0.17.1
)
```

Python decompression is used because this host lacks `xz`. The
[upstream cargo-zigbuild package](https://github.com/rust-cross/cargo-zigbuild)
provides a prebuilt Rust/Zig adapter through its supported Python package route;
`uv` was already installed. Direct `zig cc` rejects rustc's AArch64
`--fix-cortex-a53-843419` argument. The upstream adapter handles that interface.
The following installed wrappers delegate to those tools without custom argument
rewriting:

```sh
cat > "$HOME/.local/bin/prism-zig-cc" <<'EOF'
#!/bin/sh
export CARGO_ZIGBUILD_ZIG_PATH="$HOME/.local/opt/zig-aarch64-linux-0.17.1/zig"
exec "$HOME/.local/bin/cargo-zigbuild" zig cc -- "$@"
EOF
cat > "$HOME/.local/bin/prism-zig-ar" <<'EOF'
#!/bin/sh
exec "$HOME/.local/opt/zig-aarch64-linux-0.17.1/zig" ar "$@"
EOF
chmod +x "$HOME/.local/bin/prism-zig-cc" "$HOME/.local/bin/prism-zig-ar"
```

Select the fallback in each build shell:

```sh
. "$HOME/.cargo/env"
export CC="$HOME/.local/bin/prism-zig-cc"
export AR="$HOME/.local/bin/prism-zig-ar"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="$HOME/.local/bin/prism-zig-cc"
```

These settings apply to this aarch64 GNU/Linux host. They do not change Cargo or
CLI global configuration. Standard `cargo test`, `cargo clippy`, and `cargo build`
can use this environment. `cargo test --locked --lib` passed against the crate's
`Cargo.toml` and `Cargo.lock` using this fallback, including bundled SQLite.
That scaffold run contained zero tests; it verifies compilation and linking,
not application behavior or native-session integration.

## Reproduce checks

```sh
cd /workspace/agents_setup/features/prism
. "$HOME/.cargo/env"
export CC="$HOME/.local/bin/prism-zig-cc"
export AR="$HOME/.local/bin/prism-zig-ar"
sh -n scripts/check-development.sh
sh scripts/check-development.sh
```

The script checks the exact Rust version, formatter, linter, compiler, archiver,
and at least one of the Codex and Claude Code CLIs. It reports pkg-config separately because bundled
SQLite does not require it. It exits nonzero when a required tool is missing.
Once command checks pass, it compiles, archives, links and runs C/libc and
Rust/linker probes in a temporary directory, then removes its own probe files.
It does not install dependencies or alter CLI settings. The observed check passes
with the explicit fallback environment and fails without it because system `cc`
and `ar` are absent. Native-session integration was not exercised by this check.

## Development versus end-user dependencies

Building Prism requires Rust/Cargo, a native C compiler and linker, and native
headers. Development checks additionally use rustfmt and Clippy. The user-local
fallback also needs Zig and cargo-zigbuild; they are development tooling only.
`pkg-config` is optional with bundled SQLite and rustls, and remains absent on
this host. Reassess native discovery packages if crate dependencies change.

Using a prebuilt Prism binary does not require Rust, Cargo, rustfmt, Clippy, or
a compiler. It requires Codex CLI or Claude Code CLI with its native Prism
package activated, and the Prism desktop app on the host to show the workspace;
tmux is not required. Dynamic library requirements
must be checked against the released binary. Node/npm are tooling used for the
observed CLI installation; they are not established Prism runtime requirements.
