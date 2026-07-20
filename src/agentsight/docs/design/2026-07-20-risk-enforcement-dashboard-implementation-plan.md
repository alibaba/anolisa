# Risk-Enforcement Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a standalone AgentSight Dashboard page that creates, observes, and detaches the first ActPlane-backed sensitive-file blocking policy without requiring AgentSecCore.

**Architecture:** Add one product-level REST endpoint that converts a PID and canonical file path into the existing versioned `ApplyPolicy` contract. A React operations console consumes that endpoint plus the existing health, binding, violation, and detach APIs; the browser never handles ActPlane DSL or the privileged UDS.

**Tech Stack:** Rust 2024, Actix Web, SQLite/rusqlite, React 18, TypeScript, Tailwind CSS, Vitest, systemd, BPF-LSM, and ActPlane revision `a62e5d9d96f91101cda019519053e950d532380a`.

## Global Constraints

- The feature is Linux-only and must preserve AgentSight's documented kernel >= 5.8 support; real enforcement requires a host with BPF LSM enabled.
- AgentSecCore must not be installed, contacted, or required by `/enforcement`.
- The first release supports only `block open file` for one existing regular file and one live Agent process tree.
- Do not expose arbitrary ActPlane DSL, taint propagation, `unless`/`since`, network enforcement, or kill effects in the page.
- Only an ActPlane acknowledgement may produce `enforced`; only an observed kernel result may produce `blocked=true`.
- Do not add `mod.rs`, production `unwrap()`, production `expect()`, or a new third-party dependency.
- All code and comments are English; visible Dashboard copy remains Chinese to match the current navigation.
- Keep files under 500 implementation lines and the implementation diff below the AgentSight review limit by committing each task separately.
- Backend gates are `cargo fmt --all -- --check`, focused Clippy with `-D warnings`, and focused plus workspace tests on Linux.
- Dashboard gates are `npm test`, `npm run typecheck`, and `npm run build:embed` from `src/agentsight/dashboard`.
- Preserve the existing port 7396 database, Dashboard token, and installed service configuration during deployment.

## File Map

- Product REST boundary and policy template: `src/agentsight/src/server/enforcement.rs`.
- Route registration test: `src/agentsight/src/server/mod.rs`.
- Browser API types and calls: `src/agentsight/dashboard/src/utils/apiClient.ts`.
- API client tests: `src/agentsight/dashboard/src/test/apiClient.test.ts`.
- Operations console: `src/agentsight/dashboard/src/pages/RiskEnforcementPage.tsx`.
- Page behavior tests: `src/agentsight/dashboard/src/test/RiskEnforcementPage.test.tsx`.
- Navigation and routing: `src/agentsight/dashboard/src/components/NavBar.tsx` and `src/agentsight/dashboard/src/App.tsx`.
- Navigation tests: `src/agentsight/dashboard/src/test/NavBar.test.tsx` and `src/agentsight/dashboard/src/test/App.test.tsx`.
- Accepted design: `src/agentsight/docs/design/risk-enforcement-dashboard.md`.

---

### Task 1: Add the product-level file-binding API

**Files:**
- Modify: `src/agentsight/src/server/enforcement.rs`
- Modify: `src/agentsight/src/server/mod.rs`
- Test: `src/agentsight/src/server/enforcement.rs`
- Test: `src/agentsight/src/server/mod.rs`

**Interfaces:**
- Consumes: `agentsight_enforcement_protocol::ApplyPolicy` and `EnforcementCoordinator::apply`.
- Produces: `POST /api/enforcement/file-bindings`, `FileBindingRequest`, `build_file_binding`, and template revision `agentsight-file-open-v1`.

- [ ] **Step 1: Write failing request-conversion tests**

Add tests before production code. Use a live child process and an existing temporary regular file so the test exercises the same `/proc` and filesystem checks as production:

```rust
#[test]
fn builds_file_binding_from_product_fields() {
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("fixture process should start");
    let path = std::env::temp_dir().join(format!("agentsight-secret-{}", Uuid::new_v4()));
    fs::write(&path, b"fixture").expect("fixture file should exist");

    let binding = build_file_binding(FileBindingRequest {
        agent_id: " qoder ".into(),
        session_id: Some(" session-1 ".into()),
        root_pid: child.id() as i32,
        path: path.clone(),
    })
    .expect("valid request should build");

    assert_eq!(binding.agent_id, "qoder");
    assert_eq!(binding.session_id.as_deref(), Some("session-1"));
    assert_eq!(binding.root_pid, child.id() as i32);
    assert!(binding.process_start_time > 0);
    assert_eq!(binding.policy_revision, "agentsight-file-open-v1");
    assert!(binding.policy_id.starts_with("agentsight-file-open:"));
    assert!(binding.policy_dsl.contains("source AGENT = exec \"**\""));
    assert!(binding.policy_dsl.contains("block open file"));
    assert!(binding.policy_dsl.contains(path.canonicalize().unwrap().to_str().unwrap()));

    child.kill().expect("fixture process should stop");
    child.wait().expect("fixture process should exit");
    fs::remove_file(path).expect("fixture file should be removed");
}

#[test]
fn rejects_unsafe_file_binding_inputs() {
    let directory = std::env::temp_dir();
    assert!(build_file_binding(FileBindingRequest {
        agent_id: "".into(),
        session_id: None,
        root_pid: 1,
        path: directory,
    })
    .is_err());

    assert!(validate_policy_path(std::path::Path::new("relative/secret")).is_err());
    assert!(validate_policy_path(std::path::Path::new("/tmp/quote\"secret")).is_err());
}
```

- [ ] **Step 2: Run the focused test and verify the red state**

Run on Linux:

```bash
cd src/agentsight
cargo test --lib server::enforcement::tests::builds_file_binding_from_product_fields
```

Expected: compilation fails because `FileBindingRequest`, `build_file_binding`, and `validate_policy_path` do not exist.

- [ ] **Step 3: Implement the minimal request DTO and policy builder**

Add the product request and keep all low-level fields server-owned:

```rust
const FILE_POLICY_REVISION: &str = "agentsight-file-open-v1";

#[derive(Debug, Deserialize)]
pub(super) struct FileBindingRequest {
    agent_id: String,
    session_id: Option<String>,
    root_pid: i32,
    path: std::path::PathBuf,
}

fn build_file_binding(request: FileBindingRequest) -> Result<ApplyPolicy, String> {
    let agent_id = request.agent_id.trim();
    if agent_id.is_empty() || agent_id.len() > 128 {
        return Err("agent_id must contain 1 to 128 characters".into());
    }
    let path = validate_policy_path(&request.path)?;
    let process_start_time = read_target_start_time(request.root_pid)?;
    let binding_id = Uuid::new_v4();
    let path = path
        .to_str()
        .ok_or_else(|| "path must be valid UTF-8".to_string())?;
    let policy_dsl = format!(
        "source AGENT = exec \"**\"\n\
         rule agentsight-file-open:\n\
           block open file \"{path}\" if AGENT\n\
           because \"AgentSight sensitive file policy\"\n"
    );
    Ok(ApplyPolicy {
        binding_id,
        agent_id: agent_id.into(),
        session_id: request
            .session_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        root_pid: request.root_pid,
        process_start_time,
        policy_id: format!("agentsight-file-open:{binding_id}"),
        policy_revision: FILE_POLICY_REVISION.into(),
        policy_dsl,
    })
}
```

`validate_policy_path` requires an absolute existing regular file, canonicalizes it, and rejects non-UTF-8 paths, NUL bytes, double quotes, carriage returns, and newlines because the pinned ActPlane lexer does not implement quoted-string escapes. Refactor the existing process-stat parser into `read_target_start_time(pid) -> Result<u64, String>` and make `validate_target_identity` call it, preserving the self-target and PID-reuse checks.

- [ ] **Step 4: Add the HTTP handler and route test**

Add the handler:

```rust
#[post("/enforcement/file-bindings")]
pub(super) async fn apply_file_binding(
    data: web::Data<AppState>,
    body: web::Json<FileBindingRequest>,
) -> HttpResponse {
    let Some(coordinator) = data.enforcement.clone() else {
        return unavailable();
    };
    let request = match build_file_binding(body.into_inner()) {
        Ok(request) => request,
        Err(message) => {
            return error_response(
                actix_web::http::StatusCode::BAD_REQUEST,
                "invalid_file_binding",
                &message,
                false,
            );
        }
    };
    run_blocking(move || coordinator.apply(request)).await
}
```

Register `.service(enforcement::apply_file_binding)` before the static fallback. Extend `configure_routes_registers_enforcement_routes` with an invalid JSON request to `/api/enforcement/file-bindings` and assert it resolves to the API handler rather than the frontend fallback.

- [ ] **Step 5: Run backend gates**

```bash
cd src/agentsight
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all tests pass. If the repository's already-recorded unrelated Clippy baseline still fails, run the documented baseline allowances and separately run strict Clippy for the changed AgentSight targets; do not add new `#[allow]` attributes.

- [ ] **Step 6: Commit the backend API**

```bash
git add src/agentsight/src/server/enforcement.rs src/agentsight/src/server/mod.rs
git commit -m "feat(sight): add file enforcement endpoint"
```

---

### Task 2: Add typed Dashboard enforcement API calls

**Files:**
- Modify: `src/agentsight/dashboard/src/utils/apiClient.ts`
- Modify: `src/agentsight/dashboard/src/test/apiClient.test.ts`

**Interfaces:**
- Consumes: Task 1 REST endpoint and the existing health, binding, violation, and detach endpoints.
- Produces: `EnforcementHealth`, `EnforcementBinding`, `EnforcementViolation`, `FileBindingInput`, `EnforcementApiError`, `fetchEnforcementHealth`, `fetchEnforcementBindings`, `fetchEnforcementViolations`, `createFileBinding`, and `detachEnforcementBinding`.

- [ ] **Step 1: Write failing API client tests**

```typescript
describe('Enforcement APIs', () => {
  it('creates a product-level file binding', async () => {
    mockFetch.mockResolvedValueOnce(mockJsonResponse({ state: 'enforced' }));
    await createFileBinding({
      agent_id: 'qoder',
      session_id: 'session-1',
      root_pid: 45231,
      path: '/root/.ssh/id_rsa',
    });
    expect(mockFetch).toHaveBeenCalledWith(
      expect.stringContaining('/api/enforcement/file-bindings'),
      expect.objectContaining({
        method: 'POST',
        credentials: 'same-origin',
        body: JSON.stringify({
          agent_id: 'qoder',
          session_id: 'session-1',
          root_pid: 45231,
          path: '/root/.ssh/id_rsa',
        }),
      }),
    );
  });

  it('preserves structured enforcement errors', async () => {
    mockFetch.mockResolvedValueOnce(mockJsonResponse({
      error: { code: 'invalid_file_binding', message: 'path must be absolute', retryable: false },
    }, 400));
    await expect(createFileBinding({
      agent_id: 'qoder', root_pid: 2, path: 'relative',
    })).rejects.toMatchObject({ status: 400, code: 'invalid_file_binding', retryable: false });
  });

  it('detaches the selected binding', async () => {
    mockFetch.mockResolvedValueOnce({ ok: true, status: 204, text: async () => '' });
    await detachEnforcementBinding('binding-1');
    expect(mockFetch.mock.calls[0][0]).toContain('/api/enforcement/bindings/binding-1');
    expect(mockFetch.mock.calls[0][1]).toMatchObject({ method: 'DELETE' });
  });
});
```

- [ ] **Step 2: Verify the API tests fail for missing exports**

```bash
cd src/agentsight/dashboard
npm test -- --run src/test/apiClient.test.ts
```

Expected: FAIL because the enforcement exports do not exist.

- [ ] **Step 3: Implement typed requests and structured errors**

Define protocol-mirroring types with string unions for binding state and effect. Add one private request helper that always uses `credentials: 'same-origin'`, redirects `401` to `#/login`, accepts `204` without JSON, and parses `{ error: { code, message, retryable } }` into:

```typescript
export class EnforcementApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string,
    message: string,
    public readonly retryable: boolean,
  ) {
    super(message);
    this.name = 'EnforcementApiError';
  }
}
```

Expose exactly these functions:

```typescript
export const fetchEnforcementHealth = () =>
  enforcementRequest<EnforcementHealth>('/api/enforcement/health');

export const fetchEnforcementBindings = () =>
  enforcementRequest<{ bindings: EnforcementBinding[] }>('/api/enforcement/bindings');

export const fetchEnforcementViolations = (limit = 100) =>
  enforcementRequest<{ violations: EnforcementViolation[] }>(
    `/api/enforcement/violations?limit=${Math.min(1000, Math.max(1, limit))}`,
  );

export const createFileBinding = (input: FileBindingInput) =>
  enforcementRequest<EnforcementBinding>('/api/enforcement/file-bindings', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(input),
  });

export const detachEnforcementBinding = (bindingId: string) =>
  enforcementRequest<void>(
    `/api/enforcement/bindings/${encodeURIComponent(bindingId)}`,
    { method: 'DELETE' },
  );
```

- [ ] **Step 4: Run API client tests and type checking**

```bash
cd src/agentsight/dashboard
npm test -- --run src/test/apiClient.test.ts
npm run typecheck
```

Expected: both commands pass with no TypeScript errors.

- [ ] **Step 5: Commit the typed client**

```bash
git add src/agentsight/dashboard/src/utils/apiClient.ts src/agentsight/dashboard/src/test/apiClient.test.ts
git commit -m "feat(sight): add enforcement dashboard client"
```

---

### Task 3: Build the risk-enforcement operations console

**Files:**
- Create: `src/agentsight/dashboard/src/pages/RiskEnforcementPage.tsx`
- Create: `src/agentsight/dashboard/src/test/RiskEnforcementPage.test.tsx`
- Modify: `src/agentsight/dashboard/src/components/NavBar.tsx`
- Modify: `src/agentsight/dashboard/src/App.tsx`
- Modify: `src/agentsight/dashboard/src/test/NavBar.test.tsx`
- Modify: `src/agentsight/dashboard/src/test/App.test.tsx`

**Interfaces:**
- Consumes: Task 2 typed API functions.
- Produces: top-level `/enforcement` route and `RiskEnforcementPage` operations console.

- [ ] **Step 1: Write failing navigation and route tests**

Add `expect(screen.getByText('风险拦截')).toBeInTheDocument()` to the navigation test and assert that `/enforcement` highlights that link. Mock the page in `App.test.tsx` and add:

```typescript
vi.mock('../pages/RiskEnforcementPage', () => ({
  RiskEnforcementPage: () => <div data-testid="page-enforcement">RiskEnforcementPage</div>,
}));

it('renders RiskEnforcementPage on enforcement path', () => {
  window.location.hash = '#/enforcement';
  render(<App />);
  expect(screen.getByTestId('page-enforcement')).toBeInTheDocument();
});
```

- [ ] **Step 2: Run navigation tests and verify they fail**

```bash
cd src/agentsight/dashboard
npm test -- --run src/test/NavBar.test.tsx src/test/App.test.tsx
```

Expected: FAIL because the navigation item, route, and page export are missing.

- [ ] **Step 3: Add the route and navigation item**

Add `{ path: '/enforcement', label: '风险拦截', icon: '⛔' }` immediately after the existing security item. Import `RiskEnforcementPage` in `App.tsx` and register `<Route path="/enforcement" element={<RiskEnforcementPage />} />`.

- [ ] **Step 4: Write failing page behavior tests**

Mock Task 2 API calls. The default fixture has `health.ready=true`, one enforced binding, and one blocked violation. Cover these behaviors as separate tests:

```typescript
it('loads health bindings and violations independently', async () => {
  render(<RiskEnforcementPage />);
  expect(await screen.findByText('运行中')).toBeInTheDocument();
  expect(screen.getByText('/root/.ssh/id_rsa')).toBeInTheDocument();
  expect(screen.getByText('已拦截')).toBeInTheDocument();
});

it('keeps history visible when the enforcer is unavailable', async () => {
  mockFetchEnforcementHealth.mockRejectedValueOnce(
    new EnforcementApiError(503, 'enforcer_unavailable', 'socket unavailable', true),
  );
  render(<RiskEnforcementPage />);
  expect(await screen.findByText('socket unavailable')).toBeInTheDocument();
  expect(screen.getByText('/root/.ssh/id_rsa')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: '下发策略' })).toBeDisabled();
});

it('creates a file binding and refreshes the console', async () => {
  render(<RiskEnforcementPage />);
  fireEvent.change(await screen.findByLabelText('Agent ID'), { target: { value: 'qoder' } });
  fireEvent.change(screen.getByLabelText('PID'), { target: { value: '45231' } });
  fireEvent.change(screen.getByLabelText('敏感文件'), { target: { value: '/root/.ssh/id_rsa' } });
  fireEvent.click(screen.getByRole('button', { name: '下发策略' }));
  await waitFor(() => expect(mockCreateFileBinding).toHaveBeenCalledWith({
    agent_id: 'qoder', root_pid: 45231, path: '/root/.ssh/id_rsa', session_id: undefined,
  }));
  expect(await screen.findByText('策略已生效')).toBeInTheDocument();
});

it('confirms before detaching a binding', async () => {
  vi.spyOn(window, 'confirm').mockReturnValue(true);
  render(<RiskEnforcementPage />);
  fireEvent.click(await screen.findByRole('button', { name: '解除策略' }));
  await waitFor(() => expect(mockDetachEnforcementBinding).toHaveBeenCalledWith('binding-1'));
});
```

- [ ] **Step 5: Verify the page tests fail before implementation**

```bash
cd src/agentsight/dashboard
npm test -- --run src/test/RiskEnforcementPage.test.tsx
```

Expected: FAIL because `RiskEnforcementPage` does not exist.

- [ ] **Step 6: Implement the single-screen operations console**

Keep `RiskEnforcementPage.tsx` under 500 lines. Use one `loadAll()` function with `Promise.allSettled` so a health failure does not clear bindings or violations. Derive:

```typescript
const activeBindings = bindings.filter((binding) => binding.state !== 'detached');
const blockedViolations = violations.filter((event) => event.blocked).length;
const canMutate = health?.ready === true && !submitting && !detachingId;
```

Render, in order:

1. title, explanatory subtitle, readiness pill, and refresh button;
2. four summary cards for readiness, backend, active bindings, and blocked violations;
3. responsive two-column area with the binding table on the left and create form on the right;
4. newest-first violation table below.

Show individual panel errors without hiding successful panel data. Preserve form values on failure, clear them after success, use `aria-live="polite"` for operation results, and render nanosecond timestamps by converting to milliseconds before `Intl.DateTimeFormat`. The binding table uses `request.path` only if the product API adds it to the protocol; with the current contract, derive the displayed path from the single `block open file` line in `request.policy_dsl` using a narrowly tested formatter and display `—` if parsing fails.

- [ ] **Step 7: Run all Dashboard gates**

```bash
cd src/agentsight/dashboard
npm test
npm run typecheck
npm run build:embed
```

Expected: all tests, type checking, and embedded production build pass.

- [ ] **Step 8: Commit the page**

```bash
git add src/agentsight/dashboard/src/pages/RiskEnforcementPage.tsx \
  src/agentsight/dashboard/src/test/RiskEnforcementPage.test.tsx \
  src/agentsight/dashboard/src/components/NavBar.tsx \
  src/agentsight/dashboard/src/App.tsx \
  src/agentsight/dashboard/src/test/NavBar.test.tsx \
  src/agentsight/dashboard/src/test/App.test.tsx
git commit -m "feat(sight): add risk enforcement console"
```

---

### Task 4: Build, deploy, and verify the real Linux closure

**Files:**
- Verify: `src/agentsight/scripts/agentsight.service`
- Verify: `src/agentsight/scripts/agentsight-enforcer.service`
- Verify: `src/agentsight/scripts/build-enforcer.sh`
- No repository file changes are expected in this task unless verification exposes a defect; any defect starts a new red-green-refactor cycle and receives its own commit.

**Interfaces:**
- Consumes: Tasks 1-3, the pinned ActPlane patch queue, systemd units, and host `47.110.39.158`.
- Produces: a live Dashboard `/enforcement` page and evidence for create, deny, violation, detach, and restored access.

- [ ] **Step 1: Run final local and remote build gates**

```bash
cd src/agentsight/dashboard
npm test
npm run typecheck
npm run build:embed

cd ..
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
make build
make build-enforcer
```

Expected: all changed targets pass. Linux-only Rust and BPF commands run on the remote build host when the local host is macOS.

- [ ] **Step 2: Back up and install artifacts without touching databases**

On the remote host, copy the current `/usr/local/bin/agentsight`, optional `/usr/local/bin/agentsight-enforcer`, and both unit files into a timestamped `/root/agentsight-backup-<timestamp>/`. Install the newly built binaries with mode `0755`, install the reviewed units, run `systemctl daemon-reload`, then restart in this order:

```bash
systemctl enable --now agentsight-enforcer
systemctl restart agentsight
systemctl is-active agentsight-enforcer
systemctl is-active agentsight
curl -fsS http://127.0.0.1:7396/api/enforcement/health
```

Expected: both services are `active`, and health returns `ready=true` with backend `actplane`.

- [ ] **Step 3: Verify the page assets and route**

```bash
curl -fsS http://127.0.0.1:7396/ | grep -q 'AgentSight'
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:7396/
```

Expected: HTTP 200. Through the existing SSH tunnel, open `http://127.0.0.1:7396/#/enforcement` and verify the top-level `风险拦截` item, status cards, binding form, binding table, and violation table render.

- [ ] **Step 4: Run the real file-denial sequence**

Create `/tmp/agentsight-risk-demo/secret.txt` and start a long-lived non-AgentSight test process that can be instructed to open that file repeatedly. From the page, submit the test process PID, `demo-agent`, and the canonical file path. Verify:

```text
page binding state = enforced
test process open result = EPERM
page violation blocked = true
violation target = /tmp/agentsight-risk-demo/secret.txt
violation binding_id = created binding
```

Use the page's `解除策略` action, confirm HTTP 204, and instruct the same live process to open the file again. Expected: the read succeeds and the binding no longer counts as active.

- [ ] **Step 5: Verify service and resource stability**

```bash
systemctl --no-pager --full status agentsight agentsight-enforcer
journalctl -u agentsight -u agentsight-enforcer --since '15 minutes ago' --no-pager
ss -lntp | grep ':7396'
```

Expected: no restart loop, verifier error, UDS permission error, or unexpected second listener. Confirm that the pre-deployment AgentSight database files still exist and that historical Dashboard data remains queryable.

- [ ] **Step 6: Record final evidence**

Report the deployed commit, kernel version, ActPlane revision, service status, API health body, binding ID, denied errno, normalized violation identity, detach result, restored read result, and the Dashboard URL. Do not include the Dashboard token or sensitive file contents in logs or the report.
