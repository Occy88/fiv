# Fiv - Fast Image Viewer

[![CI](https://github.com/Occy88/fiv/actions/workflows/ci.yml/badge.svg)](https://github.com/Occy88/fiv/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/Occy88/fiv/branch/main/graph/badge.svg)](https://codecov.io/gh/Occy88/fiv)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A fast, lightweight image viewer built in Rust.

## Features

- **Instant navigation** - Images are preloaded in the background as you browse
- **Smooth scrolling** - Hold arrow keys to rapidly flip through images
- **Lightweight** - Minimal memory usage with smart caching
- **Wide format support** - JPEG, PNG, GIF, BMP, WebP

## Installation

### From Package Managers

**Debian/Ubuntu:**
```bash
# Download the latest .deb from releases
sudo dpkg -i fiv_*.deb
```

**Fedora/RHEL:**
```bash
# Download the latest .rpm from releases
sudo rpm -i fiv-*.rpm
```

**Arch Linux (AUR):**
```bash
yay -S fiv
```

**macOS (Homebrew):**
```bash
brew tap Occy88/tap
brew install fiv
```

### From Source

```bash
# Clone the repository
git clone https://github.com/Occy88/fiv.git
cd fiv

# Build and install
make release
sudo make install
```

## Usage

```bash
# View images in current directory
fiv

# View images in a specific directory
fiv /path/to/images
```

### Controls

| Key | Action |
|-----|--------|
| `Right` / `D` / `Space` | Next image |
| `Left` / `A` | Previous image |
| `Home` | First image |
| `End` | Last image |
| `Q` / `Escape` | Quit |

**Tip:** Hold navigation keys for rapid scrolling.

## Building

Requirements:
- Rust 1.70+
- Linux: `libxkbcommon-dev`, `libwayland-dev`

```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test
```

## License

MIT License - see [LICENSE](LICENSE) for details.
