.PHONY: build release install install-dev uninstall uninstall-dev clean

PREFIX ?= /usr/local
BINARY_NAME = fiv

build:
	cargo build

release:
	cargo build --release

install:
	@test -f target/release/$(BINARY_NAME) || { echo "Run 'make release' first"; exit 1; }
	install -d $(DESTDIR)$(PREFIX)/bin
	install -m 755 target/release/$(BINARY_NAME) $(DESTDIR)$(PREFIX)/bin/$(BINARY_NAME)
	@echo "Installed $(BINARY_NAME) to $(DESTDIR)$(PREFIX)/bin/$(BINARY_NAME)"

install-dev:
	@test -f target/release/$(BINARY_NAME) || { echo "Run 'make release' first"; exit 1; }
	install -d $(DESTDIR)$(PREFIX)/bin
	install -m 755 target/release/$(BINARY_NAME) $(DESTDIR)$(PREFIX)/bin/$(BINARY_NAME)-dev
	@echo "Installed $(BINARY_NAME)-dev to $(DESTDIR)$(PREFIX)/bin/$(BINARY_NAME)-dev"

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/$(BINARY_NAME)
	@echo "Uninstalled $(BINARY_NAME)"

uninstall-dev:
	rm -f $(DESTDIR)$(PREFIX)/bin/$(BINARY_NAME)-dev
	@echo "Uninstalled $(BINARY_NAME)-dev"

clean:
	cargo clean
