# =============================================================================
# CODE QUALITY
# =============================================================================

.PHONY: python-code-pretty
python-code-pretty: ## Format Python code using black and isort
	@echo "🎨 Formatting code with black and isort..."
	@uv run --with isort isort --profile black .
	@uv run --with black black .

# =============================================================================
# BUILD
# =============================================================================

LOONGSHIELD_DIR ?= third_party/loongshield
LOONGSHIELD_ABS_DIR := $(abspath $(LOONGSHIELD_DIR))

.PHONY: loongshield
loongshield: ## Build bundled loongshield when the host is supported
	@if [ ! -f "$(LOONGSHIELD_ABS_DIR)/Makefile" ]; then \
		echo "==> Skipping loongshield: submodule not initialized at $(LOONGSHIELD_ABS_DIR)"; \
		echo "    Run: git submodule update --init --recursive src/agent-sec-core/third_party/loongshield"; \
	elif ! $(MAKE) -s -C $(LOONGSHIELD_ABS_DIR) env-check >/dev/null 2>&1; then \
		echo "==> Skipping loongshield: current host does not satisfy loongshield build requirements"; \
	else \
		$(MAKE) -C $(LOONGSHIELD_ABS_DIR) bootstrap; \
	fi

.PHONY: build-sandbox
build-sandbox: ## Build linux-sandbox binary
	cd linux-sandbox && cargo build --release

.PHONY: build-all
build-all: loongshield build-sandbox ## Build linux-sandbox and try to build bundled loongshield

# =============================================================================
# INSTALL
# =============================================================================

PREFIX      ?= /usr/local
SKILL_DIR   ?= /usr/share/anolisa/skills/agent-sec-core

.PHONY: install-sandbox
install-sandbox: build-sandbox ## Install linux-sandbox binary only
	install -d -m 0755 $(DESTDIR)$(PREFIX)/bin
	install -p -m 0755 linux-sandbox/target/release/linux-sandbox $(DESTDIR)$(PREFIX)/bin/

.PHONY: install-loongshield
install-loongshield: ## Install bundled loongshield when the host is supported
	@if [ ! -f "$(LOONGSHIELD_ABS_DIR)/Makefile" ]; then \
		echo "==> Skipping loongshield install: submodule not initialized at $(LOONGSHIELD_ABS_DIR)"; \
	elif ! $(MAKE) -s -C $(LOONGSHIELD_ABS_DIR) env-check >/dev/null 2>&1; then \
		echo "==> Skipping loongshield install: current host does not satisfy loongshield build requirements"; \
	else \
		$(MAKE) -C $(LOONGSHIELD_ABS_DIR) DESTDIR="$(DESTDIR)" PREFIX="$(PREFIX)" install; \
	fi

.PHONY: install
install: build-sandbox install-loongshield ## Install linux-sandbox, skill files, and loongshield when available
	install -d -m 0755 $(DESTDIR)$(PREFIX)/bin
	install -p -m 0755 linux-sandbox/target/release/linux-sandbox $(DESTDIR)$(PREFIX)/bin/
	install -p -m 0755 tools/sign-skill.sh $(DESTDIR)$(PREFIX)/bin/
	install -d -m 0755 $(DESTDIR)$(SKILL_DIR)/scripts
	install -d -m 0755 $(DESTDIR)$(SKILL_DIR)/references
	cp -rp skill/scripts/* $(DESTDIR)$(SKILL_DIR)/scripts/
	cp -rp skill/references/* $(DESTDIR)$(SKILL_DIR)/references/
	cp skill/SKILL.md $(DESTDIR)$(SKILL_DIR)/
	find $(DESTDIR)$(SKILL_DIR) -type f -name '*.sh' -exec chmod 0755 {} +
	find $(DESTDIR)$(SKILL_DIR) -type f -name '*.py' -exec chmod 0755 {} +

.PHONY: help
help: ## Show this help message
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := help
