.PHONY: build release install uninstall clean

PREFIX ?= /usr/local
BINARY_NAME = picto

build:
	cargo build

release:
	cargo build --release

install:
	@test -f target/release/$(BINARY_NAME) || { echo "Run 'make release' first"; exit 1; }
	install -d $(DESTDIR)$(PREFIX)/bin
	install -m 755 target/release/$(BINARY_NAME) $(DESTDIR)$(PREFIX)/bin/$(BINARY_NAME)
	@echo "Installed $(BINARY_NAME) to $(DESTDIR)$(PREFIX)/bin/$(BINARY_NAME)"

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/$(BINARY_NAME)
	@echo "Uninstalled $(BINARY_NAME)"

clean:
	cargo clean
