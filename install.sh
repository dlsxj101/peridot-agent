#!/usr/bin/env sh
set -eu

repo="${PERIDOT_REPO:-dlsxj101/peridot-agent}"
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
  base_url="https://github.com/$repo/releases/latest/download"
else
  base_url="https://github.com/$repo/releases/download/$version"
fi
asset="peridot-$arch-$os.tar.gz"
url="$base_url/$asset"
checksum_url="$base_url/SHA256SUMS"

tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

download() {
  from="$1"
  to="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$from" -o "$to"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$from" -O "$to"
  else
    echo "curl or wget is required" >&2
    exit 1
  fi
}

sha256_file() {
  file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    echo "sha256sum or shasum is required" >&2
    exit 1
  fi
}

echo "Downloading $url"
download "$url" "$tmp_dir/$asset"
download "$checksum_url" "$tmp_dir/SHA256SUMS"

expected="$(awk -v asset="$asset" '$2 == asset {print $1; found=1} END {if (!found) exit 1}' "$tmp_dir/SHA256SUMS")" || {
  echo "checksum not found for $asset" >&2
  exit 1
}
actual="$(sha256_file "$tmp_dir/$asset")"
if [ "$actual" != "$expected" ]; then
  echo "checksum mismatch for $asset" >&2
  exit 1
fi

tar -xzf "$tmp_dir/$asset" -C "$tmp_dir"
install -m 0755 "$tmp_dir/peridot$ext" "$bin_dir/peridot$ext"
if [ "$ext" = ".exe" ]; then
  ln -sf "$bin_dir/peridot$ext" "$bin_dir/peri$ext"
else
  ln -sf "$bin_dir/peridot" "$bin_dir/peri"
fi

echo "Installed peridot to $bin_dir/peridot$ext"
echo "Installed peri alias to $bin_dir/peri$ext"
