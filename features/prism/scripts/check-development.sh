#!/bin/sh
set -eu

RUST_VERSION=1.99.1
prism_cc=${CC:-cc}
prism_ar=${AR:-ar}
failed=0

check_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        printf 'FAIL: missing %s on PATH\n' "$1" >&2
        failed=1
        return
    fi

    if ! "$@"; then
        printf 'FAIL: %s could not report its version\n' "$1" >&2
        failed=1
    fi
}

check_command rustc --version
check_command cargo --version
check_command rustfmt --version
check_command cargo-clippy --version
check_command "$prism_cc" --version
check_command "$prism_ar" --version
check_command tmux -V
check_command codex --version

if command -v pkg-config >/dev/null 2>&1; then
    check_command pkg-config --version
else
    printf 'INFO: pkg-config is absent; install it for dependencies that discover system libraries through it.\n'
fi

if command -v rustc >/dev/null 2>&1; then
    actual_version=$(rustc --version | awk '{print $2}')
    if [ "$actual_version" != "$RUST_VERSION" ]; then
        printf 'FAIL: expected Rust %s, found %s\n' "$RUST_VERSION" "$actual_version" >&2
        failed=1
    fi
fi

if [ "$failed" -ne 0 ]; then
    printf 'Development prerequisites are incomplete; see docs/development-setup.md.\n' >&2
    exit 1
fi

probe_dir=$(mktemp -d "${TMPDIR:-/tmp}/prism-development.XXXXXXXX")
trap 'rm -rf "$probe_dir"' EXIT HUP INT TERM

printf '#include <stdio.h>\nint main(void) { return puts("C compiler and libc: OK") < 0; }\n' > "$probe_dir/compiler.c"
"$prism_cc" -c "$probe_dir/compiler.c" -o "$probe_dir/compiler.o"
"$prism_ar" rcs "$probe_dir/libcompiler.a" "$probe_dir/compiler.o"
"$prism_cc" "$probe_dir/libcompiler.a" -o "$probe_dir/compiler"
"$probe_dir/compiler"

printf 'fn main() { println!("Rust compiler and linker: OK"); }\n' > "$probe_dir/linker.rs"
rustc -C "linker=$prism_cc" --crate-name prism_development_probe "$probe_dir/linker.rs" -o "$probe_dir/linker"
"$probe_dir/linker"

printf 'Development prerequisites passed; native session integration and interactive terminal behavior require separate checks.\n'
