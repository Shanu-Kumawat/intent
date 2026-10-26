# Intent — build, test, install.
#
# Default install is user-local (no root needed):
#     make build
#     make install                     # PREFIX defaults to $(HOME)/.local
#     make uninstall
#
# System-wide (opt-in; note it shadows a future pacman-managed install):
#     sudo make install PREFIX=/usr/local
#
# Packagers: make install DESTDIR=$(pkgdir) PREFIX=/usr — install never
# compiles; run `make build` as the build user first.

CARGO           ?= cargo
CARGO_TARGET_DIR ?= target
TARGET_DIR      := $(CARGO_TARGET_DIR)
RELEASE_DIR     := $(TARGET_DIR)/release

PREFIX ?= $(HOME)/.local
BINDIR  := $(DESTDIR)$(PREFIX)/bin
MANDIR  := $(DESTDIR)$(PREFIX)/share/man/man1
BASHCOMP := $(DESTDIR)$(PREFIX)/share/bash-completion/completions
ZSHCOMP  := $(DESTDIR)$(PREFIX)/share/zsh/site-functions
FISHCOMP := $(DESTDIR)$(PREFIX)/share/fish/vendor_completions.d
APPS     := $(DESTDIR)$(PREFIX)/share/applications
ICONS    := $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps

BINS := $(RELEASE_DIR)/intent $(RELEASE_DIR)/intent-gui
MANPAGES := docs/intent.1 docs/intent-gui.1

.PHONY: build test check install uninstall clean help

build:
	$(CARGO) build --release --locked

test:
	$(CARGO) test --workspace --locked

# Alias that reads like CI language; runs the suite plus fmt/clippy gates.
check: test
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --locked -- -D warnings

# Install requires prebuilt release binaries — never compile here, so a
# sudo'd install can never leave root-owned build artifacts behind.
install: | require-bins
	install -d $(BINDIR) $(MANDIR) $(BASHCOMP) $(ZSHCOMP) $(FISHCOMP) $(APPS) $(ICONS)
	install -m755 $(RELEASE_DIR)/intent      $(BINDIR)/intent
	install -m755 $(RELEASE_DIR)/intent-gui  $(BINDIR)/intent-gui
	install -m644 $(MANPAGES)                $(MANDIR)
	mkdir -p $(TARGET_DIR)/completions
	$(RELEASE_DIR)/intent completions bash > $(TARGET_DIR)/completions/intent.bash
	$(RELEASE_DIR)/intent completions zsh  > $(TARGET_DIR)/completions/_intent
	$(RELEASE_DIR)/intent completions fish > $(TARGET_DIR)/completions/intent.fish
	install -m644 $(TARGET_DIR)/completions/intent.bash  $(BASHCOMP)/intent
	install -m644 $(TARGET_DIR)/completions/_intent      $(ZSHCOMP)/_intent
	install -m644 $(TARGET_DIR)/completions/intent.fish  $(FISHCOMP)/intent.fish
	install -m644 packaging/intent-gui.desktop         $(APPS)/intent-gui.desktop
	install -m644 packaging/intent.svg                 $(ICONS)/intent.svg
ifeq ($(DESTDIR),)
	-gtk-update-icon-cache -qtf $(PREFIX)/share/icons/hicolor 2>/dev/null || true
	-update-desktop-database $(PREFIX)/share/applications 2>/dev/null || true
endif

uninstall:
	rm -f $(BINDIR)/intent $(BINDIR)/intent-gui
	rm -f $(MANDIR)/intent.1 $(MANDIR)/intent-gui.1
	rm -f $(BASHCOMP)/intent $(ZSHCOMP)/_intent $(FISHCOMP)/intent.fish
	rm -f $(APPS)/intent-gui.desktop $(ICONS)/intent.svg
ifeq ($(DESTDIR),)
	-gtk-update-icon-cache -qtf $(PREFIX)/share/icons/hicolor 2>/dev/null || true
	-update-desktop-database $(PREFIX)/share/applications 2>/dev/null || true
endif

require-bins:
	@test -x $(RELEASE_DIR)/intent -a -x $(RELEASE_DIR)/intent-gui || { \
		echo "make: release binaries missing — run 'make build' as your user first" >&2; \
		exit 1; \
	}

clean:
	$(CARGO) clean

help:
	@echo "targets: build test check install uninstall clean"
	@echo "variables: PREFIX (default ~/.local), DESTDIR (packagers), CARGO, CARGO_TARGET_DIR"
