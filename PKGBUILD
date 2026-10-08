# Maintainer: Aurelien Navarre <aurelien.navarre@acquia.com>
pkgname=omaroute
pkgver=0.1.0
pkgrel=1
pkgdesc='Route links from each app to the browser of your choice (Omarchy/Hyprland)'
arch=(x86_64)
license=(MIT)
depends=(gtk4 gtk4-layer-shell)
makedepends=(cargo)
source=()

build() {
  cd "$startdir"
  cargo build --release --locked
}

package() {
  cd "$startdir"
  install -Dm755 target/release/omaroute "$pkgdir/usr/bin/omaroute"
  install -Dm644 data/omaroute.desktop "$pkgdir/usr/share/applications/omaroute.desktop"
}
