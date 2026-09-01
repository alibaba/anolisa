# Policy CRUD process E2E

This package validates the current Policy control-plane boundary with the real
`asc-daemon` and `asc-cli` executables, a Unix socket, a generated management
credential, and a file-backed SQLite database.

The happy path covers:

1. Starting the daemon and waiting for its socket.
2. Creating, reading, updating, and listing a Policy Template and Scope.
3. Creating, reading, updating, and listing a Binding that snapshots exact
   Policy and Scope revisions.
4. Stopping the daemon with `SIGTERM`, checking socket cleanup, restarting it
   against the same database, and reading the persisted Binding.
5. Deleting the exact updated Policy and Scope revisions. Binding deletion is
   verified using its protocol meaning: a new `ABSENT` desired revision.

The error path covers:

1. A validly shaped but incorrect management credential.
2. A Policy Template file outside the shared DTO.
3. Invalid Scope selector input.
4. Updates of unknown Policy and Scope identities.
5. Binding creation with a missing Policy or Scope revision and deletion of an
   unknown Binding.
6. A successful authenticated query after the rejected requests, proving that
   errors remain request-local.

The production Adapter and PEP are intentionally outside this suite. The
current acceptance boundary is durable Binding desired state, not downstream
enforcement effectiveness.

Run only this suite:

```bash
cargo test -p asc-e2e-tests --test policy_crud
```

The suite builds the two product binaries once before launching them. It is
also included in `cargo test --workspace`.
