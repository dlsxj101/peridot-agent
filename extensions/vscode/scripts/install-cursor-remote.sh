#!/usr/bin/env bash
set -euo pipefail

publisher="dlsxj101"
extension="peridot-vscode"
version="${1:-}"
target="${2:-}"

if [[ -z "$version" ]]; then
  script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
  package_json="${script_dir}/../package.json"
  if [[ -f "$package_json" ]] && command -v node >/dev/null 2>&1; then
    version="$(node -p "require('${package_json}').version")"
  fi
fi

if [[ -z "$version" ]]; then
  echo "usage: $0 <version> [target-platform]" >&2
  echo "example: $0 0.5.17 linux-x64" >&2
  exit 2
fi

if [[ -z "$target" ]]; then
  case "$(uname -s)" in
    Linux*) os="linux" ;;
    Darwin*) os="darwin" ;;
    *) echo "unsupported OS for auto target detection: $(uname -s)" >&2; exit 2 ;;
  esac

  case "$(uname -m)" in
    x86_64 | amd64) arch="x64" ;;
    aarch64 | arm64) arch="arm64" ;;
    *) echo "unsupported arch for auto target detection: $(uname -m)" >&2; exit 2 ;;
  esac

  target="${os}-${arch}"
fi

cursor_server="${CURSOR_SERVER_BIN:-}"
if [[ -z "$cursor_server" ]]; then
  cursor_server="$(
    find "$HOME/.cursor-server/bin" -path '*/bin/cursor-server' -type f -printf '%T@ %p\n' 2>/dev/null \
      | sort -nr \
      | awk 'NR == 1 { $1=""; sub(/^ /, ""); print }'
  )"
fi

if [[ -z "$cursor_server" || ! -x "$cursor_server" ]]; then
  echo "could not find cursor-server; set CURSOR_SERVER_BIN=/path/to/cursor-server" >&2
  exit 1
fi

extensions_dir="${CURSOR_EXTENSIONS_DIR:-$HOME/.cursor-server/extensions}"
mkdir -p "$extensions_dir"

tmp="$(mktemp -t "${extension}-${version}-${target}.XXXXXX.vsix")"
trap 'rm -f "$tmp"' EXIT

url="https://marketplace.visualstudio.com/_apis/public/gallery/publishers/${publisher}/vsextensions/${extension}/${version}/vspackage?targetPlatform=${target}"

echo "[peridot] downloading ${publisher}.${extension} ${version} (${target})"
curl --compressed --fail --location --show-error --silent --output "$tmp" "$url"

if ! head -c 4 "$tmp" | LC_ALL=C grep -q $'PK\003\004'; then
  echo "downloaded file is not a decoded VSIX ZIP: $tmp" >&2
  echo "try again, or download the VSIX from the GitHub Release asset." >&2
  exit 1
fi

echo "[peridot] installing with ${cursor_server}"
"$cursor_server" \
  --extensions-dir "$extensions_dir" \
  --install-extension "$tmp" \
  --force

echo "[peridot] installed ${publisher}.${extension}@${version}; reload Cursor to activate it"
