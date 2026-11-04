#!/usr/bin/env bash
# Builds the release binary for one target on a runner of that architecture,
# checks it answers its own version, and packs it into dist/. CI runs it on
# every push so a tag is never the first build of a target.
set -euo pipefail

target="${1:?usage: build-release.sh <target triple>}"
name="kaneo-workflow-mcp"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
test -n "$version"

case "$target" in
  *-linux-musl)
    # aws-lc-sys compiles C for the TLS stack and needs the musl compiler.
    if ! command -v musl-gcc >/dev/null; then
      sudo apt-get update -qq
      sudo apt-get install -y -qq musl-tools
    fi
    # cc-rs looks for <arch>-linux-musl-gcc, which Ubuntu does not ship, and
    # falls back to musl-gcc only on x86_64: name it for every target.
    export "CC_${target//-/_}=musl-gcc"
    ;;
  *-apple-darwin) ;;
  *) echo "unsupported target $target" >&2; exit 1 ;;
esac

rustup target add "$target"
cargo build --release --locked --target "$target"

bin="target/$target/release/$name"
# Each target is built on its own architecture, so the binary runs here.
answer="$("$bin" --version)"
if [ "$answer" != "$name $version" ]; then
  echo "the binary answers \"$answer\", expected \"$name $version\"" >&2
  exit 1
fi

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp "$bin" LICENSE README.md "$stage/"
mkdir -p dist
archive="dist/$name-v$version-$target.tar.gz"
tar -C "$stage" -czf "$archive" "$name" LICENSE README.md
echo "$archive"
