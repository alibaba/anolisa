# Makefile for ws-ckpt

INSTALL_PROFILE ?= system
DESTDIR ?=
ifeq ($(INSTALL_PROFILE),user)
PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
COSH_SKILLS_DIR ?= $(HOME)/.copilot-shell/skills/ws-ckpt
else
PREFIX ?= /usr
BINDIR ?= /usr/local/bin
COSH_SKILLS_DIR ?= /usr/share/anolisa/skills/ws-ckpt
endif

LIBDIR ?= $(PREFIX)/lib
DATADIR ?= $(PREFIX)/share
SYSCONFDIR ?= /etc
RUNTIME_DIR ?= /usr/share/anolisa/runtime
RUNTIME_SKILLS_DIR ?= $(RUNTIME_DIR)/skills/ws-ckpt

ADAPTER_DIR ?= $(DATADIR)/anolisa/adapters/ws-ckpt
PLUGINS_DIR ?= $(RUNTIME_DIR)/ws-ckpt/plugins
SYSTEMD_SYSTEM_DIR ?= $(LIBDIR)/systemd/system
CONFIG_DIR ?= $(SYSCONFDIR)/ws-ckpt

.PHONY: build install uninstall clean test help

build:
	cd src && cargo build --release --workspace

install: build
ifeq ($(INSTALL_PROFILE),user)
	@echo "User mode: ws-ckpt daemon requires root, skipping install."
else
	@# 1. Install files
	install -d -m 0755 "$(DESTDIR)$(BINDIR)"
	install -p -m 0755 src/target/release/ws-ckpt "$(DESTDIR)$(BINDIR)/"
	install -d -m 0755 "$(DESTDIR)$(RUNTIME_SKILLS_DIR)"
	install -d -m 0755 "$(DESTDIR)$(COSH_SKILLS_DIR)"
	cp -pr src/skills/ws-ckpt/. "$(DESTDIR)$(RUNTIME_SKILLS_DIR)"
	cp -pr src/skills/ws-ckpt/. "$(DESTDIR)$(COSH_SKILLS_DIR)"
	install -d -m 0755 "$(DESTDIR)$(ADAPTER_DIR)"
	install -p -m 0644 adapter-manifest.json "$(DESTDIR)$(ADAPTER_DIR)/manifest.json"
	install -d -m 0755 "$(DESTDIR)$(PLUGINS_DIR)"
	cp -pr src/plugins/. "$(DESTDIR)$(PLUGINS_DIR)/"
	install -d -m 0755 "$(DESTDIR)$(SYSTEMD_SYSTEM_DIR)"
	install -p -m 0644 src/systemd/ws-ckpt.service \
		"$(DESTDIR)$(SYSTEMD_SYSTEM_DIR)/ws-ckpt.service"
	install -d -m 0755 "$(DESTDIR)$(CONFIG_DIR)"
	install -p -m 0644 src/config.toml.sample \
		"$(DESTDIR)$(CONFIG_DIR)/config.toml.sample"
	@# 2. Post-install: reload+enable+restart daemon (skip when staging)
	@if [ -z "$(DESTDIR)" ]; then \
		systemctl daemon-reload; \
		systemctl enable ws-ckpt.service 2>/dev/null || true; \
		systemctl restart ws-ckpt.service 2>/dev/null || true; \
		echo "ws-ckpt installed, started and enabled at boot."; \
		echo "Run 'systemctl status ws-ckpt' to check service status."; \
		echo "Run 'ws-ckpt --help' for usage."; \
	fi
endif

uninstall:
ifeq ($(INSTALL_PROFILE),user)
	@echo "User mode: nothing was installed, skipping uninstall."
else
	@# 1. Pre-removal: recover workspaces and stop daemon (skip when staging)
	@if [ -z "$(DESTDIR)" ]; then \
		if [ -x "$(BINDIR)/ws-ckpt" ]; then \
			"$(BINDIR)/ws-ckpt" recover --all --force 2>&1 || \
				echo "WARNING: recover workspaces failed"; \
		fi; \
		systemctl stop ws-ckpt.service 2>/dev/null || true; \
		systemctl disable ws-ckpt.service 2>/dev/null || true; \
	fi
	@# 2. Remove installed files
	rm -f "$(DESTDIR)$(BINDIR)/ws-ckpt"
	rm -rf "$(DESTDIR)$(RUNTIME_SKILLS_DIR)"
	rm -rf "$(DESTDIR)$(COSH_SKILLS_DIR)"
	rm -rf "$(DESTDIR)$(ADAPTER_DIR)"
	rm -rf "$(DESTDIR)$(PLUGINS_DIR)"
	rm -f "$(DESTDIR)$(SYSTEMD_SYSTEM_DIR)/ws-ckpt.service"
	rm -f "$(DESTDIR)$(CONFIG_DIR)/config.toml.sample"
	@# 3. Post-removal: cleanup BtrfsLoop backend (skip when staging)
	@if [ -z "$(DESTDIR)" ]; then \
		umount /mnt/btrfs-workspace 2>/dev/null || true; \
		for img in /var/lib/ws-ckpt/btrfs-data.img /data/ws-ckpt/btrfs-data.img; do \
			losetup -j "$$img" 2>/dev/null | \
				cut -d: -f1 | xargs -r losetup -d 2>/dev/null || true; \
			rm -f "$$img" 2>/dev/null || true; \
		done; \
		rmdir /mnt/btrfs-workspace 2>/dev/null || true; \
		rmdir /data/ws-ckpt 2>/dev/null || true; \
		rm -f /var/lib/ws-ckpt/state.json /var/lib/ws-ckpt/daemon.lock 2>/dev/null || true; \
		rmdir /var/lib/ws-ckpt 2>/dev/null || true; \
		systemctl daemon-reload 2>/dev/null || true; \
	fi
endif

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
