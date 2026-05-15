# Makefile for ws-ckpt

INSTALL_PROFILE ?= system
DESTDIR ?=
ifeq ($(INSTALL_PROFILE),user)
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
INSTALL_SYSTEMD ?= 0
INSTALL_CONFIG ?= 0
USER_COSH_SKILLS_DIR ?= $(HOME)/.copilot-shell/skills
else
PREFIX ?= /usr
BINDIR ?= /usr/local/bin
INSTALL_SYSTEMD ?= 1
INSTALL_CONFIG ?= 1
endif

LIBDIR ?= $(PREFIX)/lib
DATADIR ?= $(PREFIX)/share
SYSCONFDIR ?= /etc
ifeq ($(INSTALL_PROFILE),user)
RUNTIME_SKILLS_DIR ?= $(USER_COSH_SKILLS_DIR)/ws-ckpt
else
RUNTIME_SKILLS_DIR ?= $(DATADIR)/anolisa/skills/ws-ckpt
endif
ADAPTER_DIR ?= $(DATADIR)/anolisa/adapters/ws-ckpt
SYSTEMD_SYSTEM_DIR ?= $(LIBDIR)/systemd/system
CONFIG_DIR ?= $(SYSCONFDIR)/ws-ckpt

.PHONY: build install uninstall clean test help

build:
	cd src && cargo build --release --workspace

install: build
	install -d -m 0755 "$(DESTDIR)$(BINDIR)"
	install -p -m 0755 src/target/release/ws-ckpt "$(DESTDIR)$(BINDIR)/"
	install -d -m 0755 "$(DESTDIR)$(RUNTIME_SKILLS_DIR)"
	cp -pr src/skills/ws-ckpt/. "$(DESTDIR)$(RUNTIME_SKILLS_DIR)/"
	install -d -m 0755 "$(DESTDIR)$(ADAPTER_DIR)"
	install -p -m 0644 adapter-manifest.json "$(DESTDIR)$(ADAPTER_DIR)/manifest.json"
	@if [ "$(INSTALL_SYSTEMD)" = "1" ]; then \
		install -d -m 0755 "$(DESTDIR)$(SYSTEMD_SYSTEM_DIR)"; \
		install -p -m 0644 src/systemd/ws-ckpt.service \
			"$(DESTDIR)$(SYSTEMD_SYSTEM_DIR)/ws-ckpt.service"; \
	fi
	@if [ "$(INSTALL_CONFIG)" = "1" ]; then \
		install -d -m 0755 "$(DESTDIR)$(CONFIG_DIR)"; \
		install -p -m 0644 src/config.toml.sample \
			"$(DESTDIR)$(CONFIG_DIR)/config.toml.sample"; \
	fi

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/ws-ckpt"
	rm -rf "$(DESTDIR)$(RUNTIME_SKILLS_DIR)"
	rm -rf "$(DESTDIR)$(ADAPTER_DIR)"
	rm -f "$(DESTDIR)$(SYSTEMD_SYSTEM_DIR)/ws-ckpt.service"
	@if [ "$(INSTALL_CONFIG)" = "1" ]; then \
		rm -f "$(DESTDIR)$(CONFIG_DIR)/config.toml.sample"; \
	fi

clean:
	cd src && cargo clean

test:
	cd src && cargo test --workspace

help:
	@echo "Targets: build install uninstall clean test"
	@echo "Variables:"
	@echo "  INSTALL_PROFILE=user|system"
	@echo "  DESTDIR= staging root for packages"
	@echo "  PREFIX= install prefix"
