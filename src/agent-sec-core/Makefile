# =============================================================================
# CODE QUALITY
# =============================================================================

.PHONY: python-code-pretty
python-code-pretty: ## Format Python code using black and isort
	@echo "🎨 Formatting code with black and isort..."
	@uv run --project agent-sec-cli isort --profile black --skip-glob "*/backend-skill/templates/*" .
	@uv run --project agent-sec-cli black --force-exclude "backend-skill/templates/" .

# =============================================================================
# TEST
# =============================================================================

.PHONY: test-python
test-python: ## Run Python unit and integration tests
	@echo "🧪 Running Python tests..."
	cd agent-sec-cli && uv sync
	uv run --project agent-sec-cli pytest tests/ --ignore=tests/e2e/ -v
	uv run --project agent-sec-cli pytest tests/e2e/cli -v

.PHONY: test-rust
test-rust: ## Run Rust sandbox tests
	@echo "🧪 Running Rust tests..."
	cd linux-sandbox && cargo test -- --skip bwrap_seccomp

.PHONY: test
test: test-python test-rust ## Run all tests

# =============================================================================
# BUILD
# =============================================================================

.PHONY: build-sandbox
build-sandbox: ## Build linux-sandbox binary
	cd linux-sandbox && cargo build --release

.PHONY: build-cli
build-cli: ## Build agent-sec-cli wheel with maturin (Rust + Python)
	cd agent-sec-cli && uv run maturin build --release -i python3.11 --manylinux off

.PHONY: setup
setup: ## Install all dependencies (including dev), create .venv
	cd agent-sec-cli && uv sync

.PHONY: build-openclaw-plugin
build-openclaw-plugin: ## Build openclaw-plugin TypeScript sources
	cd openclaw-plugin && npm install && npm run build

.PHONY: build-all
build-all: build-sandbox build-cli build-openclaw-plugin ## Build all components (used by rpmbuild)

.PHONY: download-deps
download-deps: ## Download ALL Python deps for agent-sec-cli wheel (requires network)
	uv pip download --dest agent-sec-cli/target/wheels/ --no-cache \
		agent-sec-cli/target/wheels/agent_sec_cli-*.whl

.PHONY: stage-cli
stage-cli: ## Install all wheels to local staging dir (requires uv)
	install -d -m 0755 $(CLI_STAGED_SITE)
	@for whl in agent-sec-cli/target/wheels/*.whl; do \
		uv pip install --target $(CLI_STAGED_SITE) --no-deps --no-cache "$$whl"; \
	done

# =============================================================================
# INSTALL
# =============================================================================

PREFIX             ?= /usr/local
SKILL_DIR          ?= /usr/share/anolisa/skills
OPENCLAW_PLUGIN_DIR ?= /opt/agent-sec/openclaw-plugin
WHEEL_DIR          ?= /opt/agent-sec/wheels
CLI_STAGED_SITE    ?= _staged/site-packages
PYTHON_SITEARCH    ?= /usr/lib64/python3.11/site-packages

.PHONY: install-sandbox
install-sandbox: ## Install linux-sandbox binary only
	install -d -m 0755 $(DESTDIR)$(PREFIX)/bin
	install -p -m 0755 linux-sandbox/target/release/linux-sandbox $(DESTDIR)$(PREFIX)/bin/

.PHONY: install-tool
install-tool: ## Install sign-skill.sh to PREFIX/bin
	install -d -m 0755 $(DESTDIR)$(PREFIX)/bin
	install -p -m 0755 tools/sign-skill.sh $(DESTDIR)$(PREFIX)/bin/

.PHONY: install
install: install-all ## Install all components (alias for install-all)

.PHONY: install-cli
install-cli: ## Install agent-sec-cli wheel (for dev/debug)
	pip3 install agent-sec-cli/target/wheels/agent_sec_cli-*.whl

.PHONY: install-cli-site
install-cli-site: ## Copy staged agent-sec-cli + deps to system site-packages
	install -d -m 0755 $(DESTDIR)$(PYTHON_SITEARCH)
	cp -rp $(CLI_STAGED_SITE)/. $(DESTDIR)$(PYTHON_SITEARCH)/
	# Move uv-generated entry point from site-packages/bin/ to /usr/bin/
	install -d -m 0755 $(DESTDIR)/usr/bin
	install -p -m 0755 $(DESTDIR)$(PYTHON_SITEARCH)/bin/agent-sec-cli $(DESTDIR)/usr/bin/agent-sec-cli
	rm -rf $(DESTDIR)$(PYTHON_SITEARCH)/bin

.PHONY: install-skills
install-skills: ## Install skill files to SKILL_DIR
	install -d -m 0755 $(DESTDIR)$(SKILL_DIR)
	cp -rp skills/. $(DESTDIR)$(SKILL_DIR)/
	find $(DESTDIR)$(SKILL_DIR) -type f -name '*.sh' -exec chmod 0755 {} +
	find $(DESTDIR)$(SKILL_DIR) -type f -name '*.py' -exec chmod 0755 {} +

.PHONY: install-openclaw-plugin
install-openclaw-plugin: ## Install openclaw-plugin to target directory
	install -d -m 0755 $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)
	install -d -m 0755 $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/dist
	install -d -m 0755 $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/scripts
	cp openclaw-plugin/openclaw.plugin.json $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/
	cp openclaw-plugin/package.json $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/
	cp -r openclaw-plugin/dist/* $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/dist/
	cp -r openclaw-plugin/scripts/* $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/scripts/
	chmod 0755 $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/scripts/*.sh

.PHONY: install-cosh-hook
install-cosh-hook: ## Install cosh hooks (linux-sandbox + code_scanner_hook)
	install -d -m 0755 $(DESTDIR)$(PREFIX)/bin
	install -p -m 0755 linux-sandbox/target/release/linux-sandbox $(DESTDIR)$(PREFIX)/bin/
	install -d -m 0755 $(DESTDIR)/usr/local/lib/anolisa/cosh_hooks
	cp -rp cosh_hooks/. $(DESTDIR)/usr/local/lib/anolisa/cosh_hooks/

.PHONY: install-all
install-all: install-cli install-cosh-hook install-openclaw-plugin install-skills ## Install all components (local dev)

.PHONY: install-all-for-rpmbuild
install-all-for-rpmbuild: install-cli-site install-cosh-hook install-openclaw-plugin install-skills ## Install all components (used by rpmbuild)

.PHONY: help
help: ## Show this help message
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := help

