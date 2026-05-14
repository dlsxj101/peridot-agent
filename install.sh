#!/usr/bin/env sh
set -eu

repo="${PERIDOT_REPO:-peridot-ai/peridot}"
version="${PERIDOT_VERSION:-latest}"
bin_dir="${PERIDOT_BIN_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-gnu" ;;
  MINGW* | MSYS* | CYGWIN*) os="pc-windows-msvc" ;;
  *)
    echo "unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *)
    echo "unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

ext=""
if [ "$os" = "pc-windows-msvc" ]; then
  ext=".exe"
fi

mkdir -p "$bin_dir"

if [ "$version" = "latest" ]; then
  url="https://github.com/$repo/releases/latest/download/peridot-$arch-$os.tar.gz"
else
  url="https://github.com/$repo/releases/download/$version/peridot-$arch-$os.tar.gz"
fi

tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

echo "Downloading $url"
if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$url" -o "$tmp_dir/peridot.tar.gz"
elif command -v wget >/dev/null 2>&1; then
  wget -q "$url" -O "$tmp_dir/peridot.tar.gz"
else
  echo "curl or wget is required" >&2
  exit 1
fi

tar -xzf "$tmp_dir/peridot.tar.gz" -C "$tmp_dir"
install -m 0755 "$tmp_dir/peridot$ext" "$bin_dir/peridot$ext"
if [ "$ext" = ".exe" ]; then
  ln -sf "$bin_dir/peridot$ext" "$bin_dir/peri$ext"
else
  ln -sf "$bin_dir/peridot" "$bin_dir/peri"
fi

echo "Installed peridot to $bin_dir/peridot$ext"
echo "Installed peri alias to $bin_dir/peri$ext"
