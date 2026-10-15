#!/bin/bash
set -euo pipefail

VERSION="${1:-}"
if [[ -z "$VERSION" ]]; then
    echo "Usage: $0 <version>"
    exit 1
fi

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

curl -fsSL "https://github.com/yarikov/kvn/releases/download/v${VERSION}/checksums.txt" -o "$TMPDIR/checksums.txt"

update_package() {
    local package="$1" asset="$2" pkgdesc_suffix="$3"
    local tarball="${asset}-${VERSION}-x86_64-linux.tar.gz"
    local sha256
    sha256=$(awk -v tarball="$tarball" '$2 == tarball { print $1 }' "$TMPDIR/checksums.txt")
    [[ -n "$sha256" ]] || { echo "No checksum for $tarball" >&2; exit 1; }

    # Clone AUR repo
    git clone "ssh://aur@aur.archlinux.org/${package}.git" "$TMPDIR/$package"
    cd "$TMPDIR/$package"

    # Generate PKGBUILD from template
    sed -e "s/{{PKGNAME}}/${package}/g" \
        -e "s/{{ASSET}}/${asset}/g" \
        -e "s/{{PKGDESC_SUFFIX}}/${pkgdesc_suffix}/g" \
        -e "s/{{VERSION}}/${VERSION}/g" \
        -e "s/{{SHA256SUM}}/${sha256}/g" \
        "${SCRIPT_DIR}/PKGBUILD.bin" > PKGBUILD

    # Copy .install file
    cp "${SCRIPT_DIR}/kvn-tui.install" kvn-tui.install

    # Generate .SRCINFO
    makepkg --printsrcinfo > .SRCINFO

    # Commit and push
    git add PKGBUILD .SRCINFO kvn-tui.install
    if git diff --cached --quiet; then
        echo "No changes to commit for $package"
        return 0
    fi
    git commit -m "chore(release): update to v${VERSION}"
    git push origin HEAD:master
}

update_package kvn-bin kvn ""
update_package kvn-tui-bin kvn-tui " (transitional, replaced by kvn-bin)"
