# Maintainer: Rohan S <rohans2314@gmail.com>

# Local-source PKGBUILD: builds flow-state from this checkout.
# From the repo root, run:  makepkg -si
#
# To turn this into an AUR/`-git` package later, drop the build from
# $startdir and add a real `source=("$pkgname::git+<url>")` plus a pkgver()
# that reads `git describe`.

pkgname=flow-state
pkgver=0.1.0
pkgrel=1
pkgdesc='Distraction-free LaTeX writing app with a typewriter-style editor'
arch=('x86_64')
url='https://github.com/Rohanator2314/flow-state'
license=('GPL-3.0-or-later')
depends=('gcc-libs' 'glibc')
makedepends=('cargo')
optdepends=(
  'texlive-latex: compile the LaTeX preview (pdflatex/xelatex)'
  'poppler: render the compiled PDF into the preview pane (pdftoppm)'
)
# Built from the local working tree, not a downloaded tarball.
source=()
sha256sums=()
options=('!lto')

prepare() {
  # Vendor/verify dependencies against the committed Cargo.lock so the build
  # is reproducible and works offline.
  cd "$startdir"
  export RUSTUP_TOOLCHAIN=stable
  cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
  cd "$startdir"
  export RUSTUP_TOOLCHAIN=stable
  export CARGO_TARGET_DIR="$startdir/target"
  cargo build --frozen --release --all-features
}

package() {
  cd "$startdir"

  # Main binary.
  install -Dm755 "target/release/flow-state" "$pkgdir/usr/bin/flow-state"

  # Short alias: `fs` runs the same editor.
  ln -s flow-state "$pkgdir/usr/bin/fs"

  # License.
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"

  # Desktop launcher (no icon shipped yet — launchers fall back to a generic
  # glyph). Generated inline so the package stays self-contained.
  install -d "$pkgdir/usr/share/applications"
  cat > "$pkgdir/usr/share/applications/$pkgname.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=flow-state
GenericName=Text Editor
Comment=Distraction-free LaTeX writing app with a typewriter-style editor
Exec=flow-state %F
Terminal=false
Categories=Office;WordProcessor;TextEditor;
MimeType=text/plain;text/x-tex;
Keywords=writing;latex;editor;
StartupNotify=true
EOF

  # Optional halloy-format themes the user can copy into
  # ~/.config/flow-state/themes/ (only Ferra is built in).
  if compgen -G "themes/*.toml" >/dev/null; then
    install -Dm644 -t "$pkgdir/usr/share/$pkgname/themes" themes/*.toml
  fi
}
