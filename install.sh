#!/bin/sh
# Harness installer — https://harness.lol
# Usage: curl -fsSL https://harness.lol/install.sh | sh
set -e

REPO="ayshptk/harness"
INSTALL_DIR="$HOME/.harness/bin"
BINARY="harness"

main() {
  detect_platform
  fetch_latest_version
  download_and_install
  post_install
}

detect_platform() {
  OS="$(uname -s)"
  ARCH="$(uname -m)"

  case "$OS" in
    Linux)  OS="linux" ;;
    Darwin) OS="darwin" ;;
    *)
      echo "Error: unsupported OS: $OS" >&2
      exit 1
      ;;
  esac

  case "$ARCH" in
    x86_64|amd64) ARCH="x86_64" ;;
    arm64|aarch64) ARCH="aarch64" ;;
    *)
      echo "Error: unsupported architecture: $ARCH" >&2
      exit 1
      ;;
  esac

  case "${OS}-${ARCH}" in
    linux-x86_64)   TARGET="x86_64-unknown-linux-gnu" ;;
    darwin-x86_64)  TARGET="x86_64-apple-darwin" ;;
    darwin-aarch64) TARGET="aarch64-apple-darwin" ;;
    *)
      echo "Error: no prebuilt binary for ${OS}-${ARCH}" >&2
      echo "Install from source: cargo install harnesscli" >&2
      exit 1
      ;;
  esac

  echo "Detected platform: ${TARGET}"
}

fetch_latest_version() {
  echo "Fetching latest release..."
  VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' \
    | sed 's/.*"tag_name": *"//;s/".*//')

  if [ -z "$VERSION" ]; then
    echo "Error: could not determine latest version" >&2
    echo "Check https://github.com/${REPO}/releases" >&2
    exit 1
  fi

  echo "Latest version: ${VERSION}"
}

download_and_install() {
  URL="https://github.com/${REPO}/releases/download/${VERSION}/harness-${TARGET}.tar.gz"
  TMPDIR=$(mktemp -d)
  trap 'rm -rf "$TMPDIR"' EXIT

  echo "Downloading ${URL}..."
  curl -fsSL "$URL" -o "${TMPDIR}/harness.tar.gz"

  echo "Installing to ${INSTALL_DIR}..."
  mkdir -p "$INSTALL_DIR"
  tar xzf "${TMPDIR}/harness.tar.gz" -C "$TMPDIR"
  mv "${TMPDIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
  chmod +x "${INSTALL_DIR}/${BINARY}"
}

post_install() {
  echo ""
  echo "harness installed to ${INSTALL_DIR}/${BINARY}"

  case ":$PATH:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
      echo ""
      echo "Add harness to your PATH:"
      echo ""
      echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
      echo ""
      echo "Add the line above to your shell profile (~/.bashrc, ~/.zshrc, etc.)"
      ;;
  esac

  if command -v "${INSTALL_DIR}/${BINARY}" >/dev/null 2>&1; then
    echo ""
    "${INSTALL_DIR}/${BINARY}" --version
  fi
}

main
