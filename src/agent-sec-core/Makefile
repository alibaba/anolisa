# =============================================================================
# CODE QUALITY
# =============================================================================

.PHONY: python-code-pretty
python-code-pretty: ## Format Python code using black and isort
	@echo "🎨 Formatting code with black and isort..."
	@uv run --project agent-sec-cli isort --profile black --skip-glob "*/backend-skill/templates/*" .
	@uv run --project agent-sec-cli black --force-exclude "backend-skill/templates/" .

.PHONY: python-lint
python-lint: ## Run ruff lint check on all Python code (no fix)
	@echo "🔍 Running ruff lint check..."
	@uv run --project agent-sec-cli ruff check --config agent-sec-cli/pyproject.toml .

COMPARE_BRANCH ?= upstream/main

.PHONY: python-lint-ci
python-lint-ci: ## Run incremental ruff lint check (only changed lines vs main, non-blocking)
	@echo "🔍 Running incremental ruff lint check (diff-quality)..."
	@uv run --project agent-sec-cli ruff check --config agent-sec-cli/pyproject.toml --output-format=concise . > ruff_report.txt || true
	@sed -i '' 's|^\([^: ]*\.py\)|src/agent-sec-core/\1|' ruff_report.txt
	@cd "$(shell git rev-parse --show-toplevel)" && diff-quality --violations=flake8 --compare-branch=$(COMPARE_BRANCH) src/agent-sec-core/ruff_report.txt || true
	@rm -f ruff_report.txt

# =============================================================================
# BENCHMARK
# =============================================================================

PROMPT_SCAN_MODE ?= standard

.PHONY: benchmark-prompt-scan
benchmark-prompt-scan: ## Run prompt-scan benchmark (Chinese dataset)
	@echo "🔥 Warming up prompt-scanner (downloading ML model if not cached)..."
	@uv run --project agent-sec-cli agent-sec-cli scan-prompt warmup
	@echo "📊 Running prompt-scan benchmark (mode=$(PROMPT_SCAN_MODE))..."
	@uv run --project agent-sec-cli python3 benchmarks/prompt-scan/scripts/run_benchmark.py --mode $(PROMPT_SCAN_MODE)

.PHONY: benchmark
benchmark: benchmark-prompt-scan ## Run all benchmarks (currently: prompt-scan only)

# =============================================================================
# TEST
# =============================================================================

.PHONY: test-python
test-python: ## Run Python unit and integration tests
	@echo "🧪 Running Python tests..."
	cd agent-sec-cli && uv sync
	uv run --project agent-sec-cli pytest tests/ --ignore=tests/e2e/ -v
	uv run --project agent-sec-cli pytest tests/e2e/cli -v
	uv run --project agent-sec-cli pytest tests/e2e/daemon -v
	@echo "🧪 Running skill-ledger e2e tests..."
	uv run --project agent-sec-cli python3 tests/e2e/skill-ledger/e2e_test.py

.PHONY: test-prompt-scanner-e2e
test-prompt-scanner-e2e: ## Run prompt-scanner e2e tests (downloads ML model on first run)
	@echo "🔥 Warming up prompt-scanner (downloading ML model if not cached)..."
	uv run --project agent-sec-cli agent-sec-cli scan-prompt warmup
	@echo "🧪 Running prompt-scanner e2e tests..."
	uv run --project agent-sec-cli pytest tests/e2e/prompt-scanner/e2e_test.py

.PHONY: test-e2e-rpm
test-e2e-rpm: ## Run E2E tests against RPM-installed agent-sec-cli binary
	@echo "🧪 Running E2E tests on installed RPM..."
	@command -v agent-sec-cli >/dev/null 2>&1 || { echo "ERROR: agent-sec-cli not found on PATH"; exit 1; }
	@command -v pytest >/dev/null 2>&1 || pip3 install --quiet pytest
	python3 -m pytest tests/e2e/ \
		--import-mode=importlib \
		--ignore=tests/e2e/linux-sandbox \
		--ignore=tests/e2e/prompt-scanner \
		-k 'not test_error_event_writes_to_sqlite' \
		-v --tb=short
	@# linux-sandbox e2e skipped: requires privileged container

VENV_PYTHON ?= $(HOME)/.local/lib/anolisa/sec-core/venv/bin/python

.PHONY: test-e2e-source-build
test-e2e-source-build: ## Run E2E tests against source-build-installed agent-sec-cli
	@echo "🧪 Running E2E tests on source build..."
	@command -v agent-sec-cli >/dev/null 2>&1 || { echo "ERROR: agent-sec-cli not found on PATH"; exit 1; }
	@$(VENV_PYTHON) -m pytest --version >/dev/null 2>&1 || uv pip install --python $(VENV_PYTHON) --quiet pytest
	$(VENV_PYTHON) -m pytest tests/e2e/ \
		--import-mode=importlib \
		--ignore=tests/e2e/linux-sandbox \
		--ignore=tests/e2e/prompt-scanner \
		-k 'not test_error_event_writes_to_sqlite' \
		-v --tb=short
	@# linux-sandbox e2e skipped: requires privileged container

.PHONY: test-python-coverage
test-python-coverage: ## Run Python tests with coverage report
	@echo "🧪 Running Python tests with coverage..."
	cd agent-sec-cli && uv sync
	uv run --project agent-sec-cli pytest tests/ \
		--ignore=tests/e2e/linux-sandbox \
		--ignore=tests/e2e/prompt-scanner \
		--ignore=tests/e2e/skill-ledger \
		--ignore=tests/e2e/skill-signing \
		-v --cov --cov-config=agent-sec-cli/pyproject.toml --cov-report=xml --cov-report=term-missing

.PHONY: test-rust
test-rust: ## Run Rust sandbox tests
	@echo "🧪 Running Rust tests..."
	cd linux-sandbox && cargo test -- --skip bwrap_seccomp

.PHONY: test-rust-coverage
test-rust-coverage: ## Run Rust sandbox tests with coverage report
	@echo "🧪 Running Rust tests with coverage..."
	cd linux-sandbox && cargo llvm-cov --cobertura --output-path ../rust-coverage.xml -- --skip bwrap_seccomp

.PHONY: test-openclaw-plugin
test-openclaw-plugin: ## Run openclaw-plugin tests
	@echo "🧪 Running openclaw-plugin tests..."
	cd openclaw-plugin && npm run test && npm run smoke

.PHONY: test-openclaw-plugin-coverage
test-openclaw-plugin-coverage: ## Run openclaw-plugin tests with coverage report
	@echo "🧪 Running openclaw-plugin tests with coverage..."
	cd openclaw-plugin && npm run test:coverage && npm run smoke

.PHONY: test
test: test-python test-rust test-openclaw-plugin ## Run all tests

# =============================================================================
# BUILD
# =============================================================================

# Build output directory — all artifacts are collected here after build.
# Override from outside: make build-all BUILD_DIR=/path/to/output
BUILD_DIR ?= target

.PHONY: build-sandbox
build-sandbox: ## Build linux-sandbox binary
	cd linux-sandbox && cargo build --release
	install -d -m 0755 $(BUILD_DIR)
	cp -p linux-sandbox/target/release/linux-sandbox $(BUILD_DIR)/

.PHONY: build-cli
build-cli: ## Build agent-sec-cli wheel with maturin (Rust + Python)
	cd agent-sec-cli && uv sync --only-group dev --no-install-project && \
		uv run --no-sync maturin build --release -i python3.11 --manylinux off
	install -d -m 0755 $(BUILD_DIR)/wheels
	cp -p agent-sec-cli/target/wheels/*.whl $(BUILD_DIR)/wheels/

.PHONY: setup
setup: ## Install all dependencies (including dev), create .venv
	cd agent-sec-cli && uv sync

.PHONY: build-openclaw-plugin
build-openclaw-plugin: ## Build openclaw-plugin TypeScript sources
	cd openclaw-plugin && npm install && npm run build
	install -d -m 0755 $(BUILD_DIR)/openclaw-plugin/dist
	install -d -m 0755 $(BUILD_DIR)/openclaw-plugin/scripts
	cp openclaw-plugin/openclaw.plugin.json $(BUILD_DIR)/openclaw-plugin/
	cp openclaw-plugin/package.json $(BUILD_DIR)/openclaw-plugin/
	cp -r openclaw-plugin/dist/* $(BUILD_DIR)/openclaw-plugin/dist/
	cp -r openclaw-plugin/scripts/* $(BUILD_DIR)/openclaw-plugin/scripts/

.PHONY: build-hermes-plugin
build-hermes-plugin: ## Stage hermes-plugin Python sources to BUILD_DIR
	install -d -m 0755 $(BUILD_DIR)/hermes-plugin/src
	install -d -m 0755 $(BUILD_DIR)/hermes-plugin/scripts
	cp -rp hermes-plugin/src/. $(BUILD_DIR)/hermes-plugin/src/
	cp -rp hermes-plugin/scripts/. $(BUILD_DIR)/hermes-plugin/scripts/

.PHONY: stage-cosh-extension
stage-cosh-extension: ## Stage cosh-extension hooks to BUILD_DIR
	install -d -m 0755 $(BUILD_DIR)/cosh-extension
	cp -rp cosh-extension/. $(BUILD_DIR)/cosh-extension/

.PHONY: stage-skills
stage-skills: ## Stage skill files to BUILD_DIR
	install -d -m 0755 $(BUILD_DIR)/skills
	cp -rp skills/. $(BUILD_DIR)/skills/

.PHONY: stage-tools
stage-tools: ## Stage tools (sign-skill.sh) to BUILD_DIR
	install -d -m 0755 $(BUILD_DIR)/tools
	cp -p tools/sign-skill.sh $(BUILD_DIR)/tools/

.PHONY: stage-adapter-manifest
stage-adapter-manifest: ## Stage adapter-manifest.json to BUILD_DIR
	install -d -m 0755 $(ADAPTER_STAGE_DIR)
	install -p -m 0644 adapters/adapter-manifest.json $(ADAPTER_STAGE_DIR)/manifest.json
	install -d -m 0755 $(ADAPTER_STAGE_DIR)/common
	install -p -m 0644 adapters/common/manifest.sh $(ADAPTER_STAGE_DIR)/common/
	install -d -m 0755 $(ADAPTER_STAGE_DIR)/openclaw/scripts
	install -p -m 0755 adapters/openclaw/scripts/detect.sh $(ADAPTER_STAGE_DIR)/openclaw/scripts/
	install -p -m 0755 adapters/openclaw/scripts/install.sh $(ADAPTER_STAGE_DIR)/openclaw/scripts/
	install -p -m 0755 adapters/openclaw/scripts/uninstall.sh $(ADAPTER_STAGE_DIR)/openclaw/scripts/
	install -d -m 0755 $(ADAPTER_STAGE_DIR)/hermes/scripts
	install -p -m 0755 adapters/hermes/scripts/detect.sh $(ADAPTER_STAGE_DIR)/hermes/scripts/
	install -p -m 0755 adapters/hermes/scripts/install.sh $(ADAPTER_STAGE_DIR)/hermes/scripts/
	install -p -m 0755 adapters/hermes/scripts/uninstall.sh $(ADAPTER_STAGE_DIR)/hermes/scripts/

.PHONY: build-all
build-all: build-sandbox build-cli build-openclaw-plugin build-hermes-plugin stage-cosh-extension stage-skills stage-adapter-manifest ## Build all components
	@echo "📦 All artifacts collected to $(BUILD_DIR)/"

.PHONY: export-requirements
export-requirements: ## Re-export agent-sec-cli/requirements.txt from uv.lock
	cd agent-sec-cli && uv export --frozen --no-dev --no-emit-project -o requirements.txt

.PHONY: download-deps
download-deps: ## Download ALL Python deps for agent-sec-cli (requires network)
	install -d -m 0755 $(BUILD_DIR)/wheels
	pip3 download --dest $(BUILD_DIR)/wheels/ --no-cache-dir \
		--python-version 3.11.6 --only-binary=:all: \
		--timeout 60 \
		--require-hashes \
		--index-url https://pypi.org/simple/ \
		--extra-index-url https://download.pytorch.org/whl/cpu \
		-r agent-sec-cli/requirements.txt

.PHONY: stage-cli
stage-cli: ## Install all wheels to local staging dir (requires uv)
	install -d -m 0755 $(CLI_STAGED_SITE)
	uv pip install --target $(CLI_STAGED_SITE) --no-deps --no-cache --link-mode copy \
		$(BUILD_DIR)/wheels/*.whl
	rm -f $(CLI_STAGED_SITE)/.lock

# =============================================================================
# INSTALL
# =============================================================================

# --- Install profile: 'system' (RPM) or 'user' (source build) ----------------
INSTALL_PROFILE ?= system

ifeq ($(INSTALL_PROFILE),user)
  PREFIX          ?= $(HOME)/.local
  ANOLISA_DATADIR ?= $(PREFIX)/share/anolisa
  EXTENSIONDIR    ?= $(HOME)/.copilot-shell/extensions/agent-sec-core
  SKILLDIR        ?= $(HOME)/.copilot-shell/skills
  OPENCLAW_PLUGIN_DIR ?= $(LIBDIR)/openclaw-plugin
  HERMES_PLUGIN_DIR   ?= $(LIBDIR)/hermes-plugin
else
  PREFIX          ?= /usr/local
  ANOLISA_DATADIR ?= /usr/share/anolisa
  EXTENSIONDIR    ?= $(ANOLISA_DATADIR)/extensions/agent-sec-core
  SKILLDIR        ?= $(ANOLISA_DATADIR)/skills
  OPENCLAW_PLUGIN_DIR ?= $(LIBDIR)/openclaw-plugin
  HERMES_PLUGIN_DIR   ?= $(LIBDIR)/hermes-plugin
endif

ADAPTER_DIR         ?= $(ANOLISA_DATADIR)/adapters/sec-core
ADAPTER_STAGE_DIR   ?= $(BUILD_DIR)/share/anolisa/adapters/sec-core

BINDIR              ?= $(PREFIX)/bin
LIBDIR              ?= $(PREFIX)/lib/anolisa/sec-core
LIBEXECDIR          ?= $(PREFIX)/libexec/anolisa/sec-core
VENV_DIR            ?= $(LIBDIR)/venv
WHEEL_DIR           ?= $(LIBDIR)/wheels
CLI_STAGED_SITE     ?= $(BUILD_DIR)/site-packages
CLI_PRIVATE_SITE    ?= /opt/agent-sec/lib/python3.11/site-packages
RPM_OPENCLAW_PLUGIN_DIR ?= /opt/agent-sec/openclaw-plugin
RPM_HERMES_PLUGIN_DIR ?= /opt/agent-sec/hermes-plugin

.PHONY: install-sandbox
install-sandbox: ## Install linux-sandbox binary only
	install -d -m 0755 $(DESTDIR)$(BINDIR)
	install -p -m 0755 $(BUILD_DIR)/linux-sandbox $(DESTDIR)$(BINDIR)/

.PHONY: install-tool
install-tool: ## Install sign-skill.sh to LIBEXECDIR
	install -d -m 0755 $(DESTDIR)$(LIBEXECDIR)
	install -p -m 0755 $(BUILD_DIR)/tools/sign-skill.sh $(DESTDIR)$(LIBEXECDIR)/

.PHONY: install
install: install-all ## Install all components (alias for install-all)

.PHONY: install-cli
install-cli: ## Install agent-sec-cli wheel (for dev/debug)
	pip3 install $(BUILD_DIR)/wheels/agent_sec_cli-*.whl

.PHONY: install-cli-site
install-cli-site: ## RPM: Copy staged site-packages to private dir + wrapper
	# 1. Copy all Python packages to private directory
	install -d -m 0755 $(DESTDIR)$(CLI_PRIVATE_SITE)
	cp -rp $(CLI_STAGED_SITE)/. $(DESTDIR)$(CLI_PRIVATE_SITE)/
	# Remove uv-generated bin/ and .lock from private site-packages
	rm -rf $(DESTDIR)$(CLI_PRIVATE_SITE)/bin
	rm -f  $(DESTDIR)$(CLI_PRIVATE_SITE)/.lock
	# 2. Install wrapper script as /usr/bin/agent-sec-cli
	install -d -m 0755 $(DESTDIR)/usr/bin
	install -p -m 0755 scripts/agent-sec-cli-wrapper.sh $(DESTDIR)/usr/bin/agent-sec-cli

.PHONY: install-cli-venv
install-cli-venv: ## User: Create venv, install deps from uv.lock, install wheel, symlink
	@echo "Creating Python venv at $(VENV_DIR) ..."
	install -d -m 0755 $(VENV_DIR)
	uv venv --python 3.11.6 --allow-existing $(VENV_DIR)
	@echo "Installing dependencies from uv.lock ..."
	cd agent-sec-cli && UV_PROJECT_ENVIRONMENT=$(VENV_DIR) uv sync --frozen --no-dev --no-install-project
	@echo "Installing agent-sec-cli wheel ..."
	uv pip install --python $(VENV_DIR)/bin/python --no-deps \
		$(BUILD_DIR)/wheels/agent_sec_cli-*.whl
	@echo "Creating symlink ..."
	install -d -m 0755 $(BINDIR)
	ln -sf $(VENV_DIR)/bin/agent-sec-cli $(BINDIR)/agent-sec-cli
	ln -sf $(VENV_DIR)/bin/agent-sec-daemon $(BINDIR)/agent-sec-daemon

.PHONY: install-skills
install-skills: ## Install skill files to SKILLDIR
	install -d -m 0755 $(DESTDIR)$(SKILLDIR)
	cp -rp $(BUILD_DIR)/skills/. $(DESTDIR)$(SKILLDIR)/
	find $(DESTDIR)$(SKILLDIR) -type f -name '*.sh' -exec chmod 0755 {} +
	find $(DESTDIR)$(SKILLDIR) -type f -name '*.py' -exec chmod 0755 {} +

.PHONY: install-openclaw-plugin
install-openclaw-plugin: ## Install openclaw-plugin to target directory
	install -d -m 0755 $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)
	install -d -m 0755 $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/dist
	install -d -m 0755 $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/scripts
	cp $(BUILD_DIR)/openclaw-plugin/openclaw.plugin.json $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/
	cp $(BUILD_DIR)/openclaw-plugin/package.json $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/
	cp -r $(BUILD_DIR)/openclaw-plugin/dist/* $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/dist/
	cp -r $(BUILD_DIR)/openclaw-plugin/scripts/* $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/scripts/
	chmod 0755 $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)/scripts/*.sh

.PHONY: install-hermes-plugin
install-hermes-plugin: ## Install hermes-plugin to target directory
	install -d -m 0755 $(DESTDIR)$(HERMES_PLUGIN_DIR)/src
	install -d -m 0755 $(DESTDIR)$(HERMES_PLUGIN_DIR)/scripts
	cp -rp $(BUILD_DIR)/hermes-plugin/src/. $(DESTDIR)$(HERMES_PLUGIN_DIR)/src/
	cp -rp $(BUILD_DIR)/hermes-plugin/scripts/. $(DESTDIR)$(HERMES_PLUGIN_DIR)/scripts/
	chmod 0755 $(DESTDIR)$(HERMES_PLUGIN_DIR)/scripts/*.sh

.PHONY: install-cosh-hook
install-cosh-hook: ## Install cosh hooks (linux-sandbox + extension)
	install -d -m 0755 $(DESTDIR)$(BINDIR)
	install -p -m 0755 $(BUILD_DIR)/linux-sandbox $(DESTDIR)$(BINDIR)/
	install -d -m 0755 $(DESTDIR)$(EXTENSIONDIR)
	cp -rp $(BUILD_DIR)/cosh-extension/. $(DESTDIR)$(EXTENSIONDIR)/

.PHONY: install-adapter-manifest
install-adapter-manifest: ## Install adapter-manifest.json from staged copy
	test -f $(ADAPTER_STAGE_DIR)/manifest.json
	install -d -m 0755 $(DESTDIR)$(ADAPTER_DIR)
	install -p -m 0644 $(ADAPTER_STAGE_DIR)/manifest.json $(DESTDIR)$(ADAPTER_DIR)/manifest.json
	install -d -m 0755 $(DESTDIR)$(ADAPTER_DIR)/common
	install -p -m 0644 $(ADAPTER_STAGE_DIR)/common/manifest.sh $(DESTDIR)$(ADAPTER_DIR)/common/
	install -d -m 0755 $(DESTDIR)$(ADAPTER_DIR)/openclaw/scripts
	install -p -m 0755 $(ADAPTER_STAGE_DIR)/openclaw/scripts/detect.sh $(DESTDIR)$(ADAPTER_DIR)/openclaw/scripts/
	install -p -m 0755 $(ADAPTER_STAGE_DIR)/openclaw/scripts/install.sh $(DESTDIR)$(ADAPTER_DIR)/openclaw/scripts/
	install -p -m 0755 $(ADAPTER_STAGE_DIR)/openclaw/scripts/uninstall.sh $(DESTDIR)$(ADAPTER_DIR)/openclaw/scripts/
	install -d -m 0755 $(DESTDIR)$(ADAPTER_DIR)/hermes/scripts
	install -p -m 0755 $(ADAPTER_STAGE_DIR)/hermes/scripts/detect.sh $(DESTDIR)$(ADAPTER_DIR)/hermes/scripts/
	install -p -m 0755 $(ADAPTER_STAGE_DIR)/hermes/scripts/install.sh $(DESTDIR)$(ADAPTER_DIR)/hermes/scripts/
	install -p -m 0755 $(ADAPTER_STAGE_DIR)/hermes/scripts/uninstall.sh $(DESTDIR)$(ADAPTER_DIR)/hermes/scripts/

.PHONY: install-all
install-all: install-cli-venv install-cosh-hook install-openclaw-plugin install-hermes-plugin install-skills install-adapter-manifest ## Install all (user source build)

.PHONY: install-all-for-rpmbuild
install-all-for-rpmbuild: OPENCLAW_PLUGIN_DIR := $(RPM_OPENCLAW_PLUGIN_DIR)
install-all-for-rpmbuild: HERMES_PLUGIN_DIR := $(RPM_HERMES_PLUGIN_DIR)
install-all-for-rpmbuild: install-cli-site install-cosh-hook install-openclaw-plugin install-hermes-plugin install-skills install-adapter-manifest ## Install all (RPM build)

# =============================================================================
# UNINSTALL
# =============================================================================
# Mirrors install-all (source build). RPM uninstall is handled by the package
# manager via the .spec %files manifests, not by `make uninstall`.

.PHONY: uninstall-cli-venv
uninstall-cli-venv: ## Remove venv and agent-sec-cli symlink
	rm -f $(DESTDIR)$(BINDIR)/agent-sec-cli
	rm -f $(DESTDIR)$(BINDIR)/agent-sec-daemon
	rm -rf $(DESTDIR)$(VENV_DIR)

.PHONY: uninstall-cli-site
uninstall-cli-site: ## Remove RPM-style private site-packages and wrapper
	rm -f $(DESTDIR)$(BINDIR)/agent-sec-cli
	rm -rf $(DESTDIR)$(CLI_PRIVATE_SITE)

.PHONY: uninstall-cosh-hook
uninstall-cosh-hook: ## Remove linux-sandbox and cosh extension
	rm -f $(DESTDIR)$(BINDIR)/linux-sandbox
	rm -rf $(DESTDIR)$(EXTENSIONDIR)

.PHONY: uninstall-openclaw-plugin
uninstall-openclaw-plugin: ## Remove openclaw plugin
	rm -rf $(DESTDIR)$(OPENCLAW_PLUGIN_DIR)

.PHONY: uninstall-hermes-plugin
uninstall-hermes-plugin: ## Remove hermes plugin
	rm -rf $(DESTDIR)$(HERMES_PLUGIN_DIR)

.PHONY: uninstall-skills
uninstall-skills: ## Remove sec-core's own skill subdirs from SKILLDIR
	@for d in skills/*/; do \
		[ -d "$$d" ] || continue; \
		name=$$(basename "$$d"); \
		rm -rf "$(DESTDIR)$(SKILLDIR)/$$name"; \
	done

.PHONY: uninstall-adapter-manifest
uninstall-adapter-manifest: ## Remove installed adapter manifest directory
	rm -rf $(DESTDIR)$(ADAPTER_DIR)

.PHONY: uninstall
uninstall: uninstall-cli-venv uninstall-cosh-hook uninstall-openclaw-plugin uninstall-hermes-plugin uninstall-skills uninstall-adapter-manifest ## Uninstall everything install-all installed

.PHONY: help
help: ## Show this help message
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

.DEFAULT_GOAL := help
