#!/bin/sh
# shellcheck disable=SC3043
set -eu

REPO="scriptology/open-course-cli"
BIN_NAME="open-course-cli"

main() {
    detect_platform
    say "Installing $BIN_NAME for $TARGET..."

    ensure curl
    ensure mkdir
    ensure tar

    get_release_version
    set_install_dir

    DOWNLOAD_URL="https://github.com/$REPO/releases/download/$RELEASE/$BIN_NAME-$RELEASE-$TARGET.tar.gz"
    TMP_DIR="$(mktemp -d)"
    ARCHIVE="$TMP_DIR/$BIN_NAME.tar.gz"

    if ! curl --proto '=https' --tlsv1.2 -LsSf "$DOWNLOAD_URL" -o "$ARCHIVE"; then
        err "Failed to download $DOWNLOAD_URL"
    fi

    tar -xzf "$ARCHIVE" -C "$TMP_DIR"

    if [ ! -f "$TMP_DIR/$BIN_NAME" ]; then
        err "Archive did not contain $BIN_NAME"
    fi

    chmod +x "$TMP_DIR/$BIN_NAME"

    if [ ! -d "$INSTALL_DIR" ]; then
        mkdir -p "$INSTALL_DIR" || err "Could not create $INSTALL_DIR"
    fi

    if [ -w "$INSTALL_DIR" ]; then
        mv "$TMP_DIR/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
        ln -sf "$INSTALL_DIR/$BIN_NAME" "$INSTALL_DIR/opencourse"
    else
        say "Installing to $INSTALL_DIR requires sudo."
        sudo mv "$TMP_DIR/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
        sudo ln -sf "$INSTALL_DIR/$BIN_NAME" "$INSTALL_DIR/opencourse"
    fi

    rm -rf "$TMP_DIR"

    case ":$PATH:" in
        *":$INSTALL_DIR:"*)
            say "Installed $BIN_NAME $RELEASE to $INSTALL_DIR"
            say "You can run: open-course-cli or opencourse"
            ;;
        *)
            say "Installed $BIN_NAME $RELEASE to $INSTALL_DIR"
            say "You can run: open-course-cli or opencourse"
            say "Add it to your PATH:"
            say "  export PATH=\"$INSTALL_DIR:\$PATH\""
            ;;
    esac
}

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Darwin)
            case "$ARCH" in
                arm64|aarch64)
                    TARGET="aarch64-apple-darwin"
                    ;;
                x86_64|amd64)
                    TARGET="x86_64-apple-darwin"
                    ;;
                *)
                    err "Unsupported architecture: $ARCH"
                    ;;
            esac
            ;;
        Linux)
            case "$ARCH" in
                x86_64|amd64)
                    TARGET="x86_64-unknown-linux-gnu"
                    ;;
                *)
                    err "Unsupported architecture: $ARCH"
                    ;;
            esac
            ;;
        *)
            err "Unsupported OS: $OS"
            ;;
    esac
}

get_release_version() {
    if [ -n "${VERSION:-}" ]; then
        RELEASE="$VERSION"
        say "Release: $RELEASE"
        return
    fi

    API_URL="https://api.github.com/repos/$REPO/releases/latest"
    TAG=$(curl --proto '=https' --tlsv1.2 -sSf "$API_URL" | sed -n 's/.*"tag_name": "\([^"]*\)".*/\1/p')
    if [ -z "$TAG" ]; then
        err "Could not determine latest release"
    fi
    RELEASE="$TAG"
    say "Release: $RELEASE"
}

set_install_dir() {
    if [ -n "${INSTALL_DIR:-}" ]; then
        return
    fi

    if [ -d "/usr/local/bin" ] && [ -w "/usr/local/bin" ]; then
        INSTALL_DIR="/usr/local/bin"
    else
        INSTALL_DIR="$HOME/.local/bin"
    fi
}

ensure() {
    if ! command -v "$1" >/dev/null 2>&1; then
        err "Required command not found: $1"
    fi
}

say() {
    printf "%s\n" "$1"
}

err() {
    printf "Error: %s\n" "$1" >&2
    exit 1
}

main "$@"
