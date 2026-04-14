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

.PHONY: build-sandbox
build-sandbox: ## Build linux-sandbox binary
	cd linux-sandbox && cargo build --release

.PHONY: build-cli
build-cli: ## Build agent-sec-cli wheel with maturin (Rust + Python)
	pip3 install maturin
	cd agent-sec-cli && maturin build --release

# =============================================================================
# INSTALL
# =============================================================================

PREFIX      ?= /usr/local
SKILL_DIR   ?= /usr/share/anolisa/skills/agent-sec-core

.PHONY: install-sandbox
install-sandbox: ## Install linux-sandbox binary only
	install -d -m 0755 $(DESTDIR)$(PREFIX)/bin
	install -p -m 0755 linux-sandbox/target/release/linux-sandbox $(DESTDIR)$(PREFIX)/bin/

.PHONY: install
install: ## Install linux-sandbox binary, sign-skill.sh and skill files
	install -d -m 0755 $(DESTDIR)$(PREFIX)/bin
	install -p -m 0755 linux-sandbox/target/release/linux-sandbox $(DESTDIR)$(PREFIX)/bin/
	install -p -m 0755 tools/sign-skill.sh $(DESTDIR)$(PREFIX)/bin/
	pip3 install --root=$(DESTDIR) --no-deps --no-cache-dir --prefix=/usr \
		agent-sec-cli/target/wheels/agent_sec_cli-*.whl
	install -d -m 0755 $(DESTDIR)$(SKILL_DIR)/scripts
	install -d -m 0755 $(DESTDIR)$(SKILL_DIR)/references
	cp -rp skill/references/* $(DESTDIR)$(SKILL_DIR)/references/
	cp skill/SKILL.md $(DESTDIR)$(SKILL_DIR)/
	find $(DESTDIR)$(SKILL_DIR) -type f -name '*.sh' -exec chmod 0755 {} +
	find $(DESTDIR)$(SKILL_DIR) -type f -name '*.py' -exec chmod 0755 {} +

.PHONY: help
help: ## Show this help message
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := help

