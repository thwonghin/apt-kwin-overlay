# Maintainer: Hin Wong <fraywong@gmail.com>
pkgname=apt-kwin-overlay
pkgver=0.1.0
pkgrel=1
pkgdesc="Native KWin/Wayland backend for Awakened PoE Trade"
arch=('x86_64')
url="https://github.com/thwonghin/apt-kwin-overlay"
license=('MIT')
depends=('gtk4' 'gtk4-layer-shell' 'webkitgtk-6.0')
makedepends=('git' 'rust' 'nodejs' 'npm')
source=("$pkgname::git+https://github.com/thwonghin/apt-kwin-overlay.git")
sha256sums=('SKIP')

pkgver() {
  cd "$pkgname"
  printf "r%s.%s" "$(git rev-list --count HEAD)" "$(git rev-parse --short HEAD)"
}

prepare() {
  cd "$pkgname"
  git submodule update --init --recursive
}

build() {
  cd "$pkgname"
  ./scripts/build-renderer.sh
  cargo build --release --locked
}

package() {
  cd "$pkgname"
  install -Dm755 target/release/apt-kwin-overlay "$pkgdir/usr/bin/apt-kwin-overlay"
  install -d "$pkgdir/usr/share/apt-kwin-overlay"
  cp -r vendor/awakened-poe-trade/renderer/dist "$pkgdir/usr/share/apt-kwin-overlay/dist"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
}
