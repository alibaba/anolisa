# Run SkillFS as a Kubernetes Sidecar

[中文版](../../zh/runtime/skillfs-kubernetes-sidecar.md)

Run SkillFS beside a Kubernetes workload so the workload reads the SkillFS
view without mounting the physical skill source. The SkillFS container owns the
FUSE mount; the workload stays non-privileged and receives the propagated view.

The repository does not currently publish a dedicated SkillFS sidecar image.
Build the image from the source revision you plan to deploy, verify it locally,
and push it to a registry that the cluster can pull from.

## Prerequisites

- Kubernetes 1.29 or later.
- Linux nodes with `/dev/fuse`.
- Permission to run the SkillFS sidecar as privileged.
- `docker buildx` and `kubectl`.
- A registry that the cluster can pull from.

## Choose the image base

SkillFS provides two equivalent sidecar image definitions. They use the same
entrypoint, probes, default paths, and Kubernetes manifest.

| Dockerfile | Runtime base | Use when |
| --- | --- | --- |
| `src/skillfs/container/Dockerfile` | Debian Bookworm | You want the general-purpose image with the pinned Rust 1.86 build toolchain |
| `src/skillfs/container/Dockerfile.alinux4` | Alibaba Cloud Linux 4 | Your deployment standardizes on Alibaba Cloud Linux 4 or builds through the public Aliyun RPM and Cargo mirrors |

The Alibaba Cloud Linux 4 build uses the Aliyun Cargo mirror by default. Pass
an empty `SKILLFS_CARGO_REGISTRY_INDEX` build argument when the build
environment should use crates.io directly. Both image definitions accept a
`BASE_IMAGE` build argument when production builds need a pinned base tag or
digest.

## Build, verify, and push the image

Run the following commands from the repository root. The example builds the
Debian image for AMD64 and tags it with the source commit so an operator can
trace the deployed image back to its inputs.

```bash
export REVISION="$(git rev-parse HEAD)"
export VERSION="$(git describe --tags --match 'skillfs/v*' --always)"
export IMAGE="registry.example.com/anolisa/skillfs-sidecar:$(git rev-parse --short=12 HEAD)"
export PLATFORM=linux/amd64
export DOCKERFILE=src/skillfs/container/Dockerfile

docker buildx build \
  --platform "$PLATFORM" \
  --build-arg VERSION="$VERSION" \
  --build-arg REVISION="$REVISION" \
  -f "$DOCKERFILE" \
  -t "$IMAGE" \
  --load \
  src/skillfs

docker run --rm --platform "$PLATFORM" "$IMAGE" skillfs --version
docker push "$IMAGE"
```

Set `DOCKERFILE=src/skillfs/container/Dockerfile.alinux4` to build the Alibaba
Cloud Linux 4 variant. Set `PLATFORM` to the target node architecture and run
the smoke check on every platform before publishing a multi-platform tag. The
repository CI does not currently build or test these dedicated sidecar images.

The `docker run` command above only verifies the binary and runtime libraries.
Serving a FUSE view still requires the device, privilege, volumes, and mount
propagation described below.

## How the image starts

With no command arguments, the image runs its preflight checks and then starts
this fixed container lifecycle:

```text
skillfs mount "$SKILLFS_SOURCE" "$SKILLFS_MOUNTPOINT" \
  --foreground --allow-other
```

`SKILLFS_DISCOVER_ROOT` and `SKILLFS_EXTRA_ARGS` add optional mount arguments.
Do not add `--managed`; the foreground SkillFS process must remain PID 1 so the
kubelet can restart it and deliver `SIGTERM` directly. Passing command arguments
to the image replaces the mount command completely, which is why the version
smoke check works without `/dev/fuse`.

## Required Pod topology

The reference Pod keeps the physical source away from the workload and shares
only the propagated FUSE view.

| Volume | Sidecar mount | Workload mount | Requirement |
| --- | --- | --- | --- |
| `skill-source` | `/var/lib/skillfs/source` | Not mounted | Must be writable; use a PVC when skill changes must survive Pod recreation |
| `skill-shared` | `/var/lib/skillfs/shared` with `Bidirectional` | The same path with `HostToContainer` | Keep the FUSE mountpoint in a subdirectory such as `shared/mount`, never over the volume root |
| `fuse-device` | `/dev/fuse` | Not mounted | Use a `/dev/fuse` `hostPath` with type `CharDevice` |

Only the SkillFS sidecar runs as root with `privileged` enabled. The workload
can run as a different non-root UID because the image always adds
`--allow-other`. The manifest uses a native Kubernetes sidecar by setting the
SkillFS init container's `restartPolicy` to `Always`, so Kubernetes waits for
the FUSE startup probe before starting the workload and stops the sidecar after
the workload exits.

## Deploy

The example uses a ConfigMap-backed skill source. Replace it with a PVC for
persistent workloads.

```bash
export NS=skillfs-container-example

kubectl apply -f src/skillfs/deploy/kubernetes/00-namespace.yaml
kubectl apply -f src/skillfs/deploy/kubernetes/10-example-configmap.yaml
sed "s|skillfs-sidecar:dev|$IMAGE|g" \
  src/skillfs/deploy/kubernetes/20-pod.yaml | kubectl apply -f -

kubectl -n "$NS" wait \
  --for=condition=Ready pod/skillfs-sidecar-example \
  --timeout=300s
```

## Verify the mounted view

Read the view from the non-privileged workload container:

```bash
export POD=skillfs-sidecar-example
export VIEW=/var/lib/skillfs/shared/mount/skills

kubectl -n "$NS" exec "$POD" -c agent -- ls -1 "$VIEW"
kubectl -n "$NS" exec "$POD" -c agent -- \
  cat "$VIEW/skillfs-container-example/SKILL.md"
kubectl -n "$NS" exec "$POD" -c agent -- \
  cat "$VIEW/skill-discover/SKILL.md"
kubectl -n "$NS" exec "$POD" -c agent -- \
  cat "$VIEW/skillfs-container-reserve/SKILL.md"
```

The listing must contain `skillfs-container-example` and `skill-discover`, but
not `skillfs-container-reserve`. The `skill-discover` output must contain the
`reserve` view and the absolute path used by the last command. Secondary
skills are hidden from directory listings, while their advertised paths remain
readable.

## Verify sidecar restart

```bash
kubectl -n "$NS" exec "$POD" -c skillfs -- \
  /bin/bash -c 'kill -TERM 1'
kubectl -n "$NS" wait \
  --for=condition=Ready pod/skillfs-sidecar-example \
  --timeout=300s
```

Run the mounted-view commands again after the Pod returns to Ready.

## Use your own workload

Edit `src/skillfs/deploy/kubernetes/20-pod.yaml`:

1. replace `skill-source` with your PVC;
2. remove the example ConfigMap and `seed-example` init container;
3. set `SKILLFS_PROBE_FILE` to a stable, non-empty file that remains visible
   for the lifetime of the mount;
4. replace the `agent` image and command;
5. keep `Bidirectional` on the SkillFS mount and `HostToContainer` on the
   workload mount.

The workload readiness probe should read meaningful SkillFS content, not only
check the directory or run `skillfs --version`.

The reference manifest marks the Pod unready after one failed FUSE read and
restarts only the SkillFS sidecar after two consecutive liveness failures. The
single-failure threshold means that a transient probe timeout also immediately
marks the Pod unready, which may shift traffic under high concurrency. The
workload intentionally has no liveness probe. After an `EIO` or `ENOTCONN`, a
consumer must close the failed file descriptor and reopen the file after the
Pod becomes Ready again.

## Image configuration

The image defaults match the reference manifest. Override them in the Pod when
your volume paths or probe skill differ.

| Variable | Default | Purpose |
| --- | --- | --- |
| `SKILLFS_SOURCE` | `/var/lib/skillfs/source` | Writable physical skill source root |
| `SKILLFS_MOUNTPOINT` | `/var/lib/skillfs/shared/mount` | FUSE mountpoint inside the shared volume |
| `SKILLFS_DISCOVER_ROOT` | `/var/lib/skillfs/shared/mount/skills` | Reader-visible root advertised by `skill-discover` |
| `SKILLFS_EXTRA_ARGS` | Empty | Additional whitespace-separated `skillfs mount` arguments |
| `SKILLFS_PROBE_FILE` | `skills/skillfs-container-example/SKILL.md` | Stable, non-empty file read through FUSE by the health probe |
| `SKILLFS_PROBE_TIMEOUT` | `5` | Per-read health probe timeout in seconds |
| `RUST_LOG` | `info` | SkillFS log filter |

The preflight check requires distinct absolute source and mountpoint paths, a
writable source, an openable `/dev/fuse`, `fusermount3`, and
`user_allow_other` in `/etc/fuse.conf`. `SKILLFS_SKIP_PREFLIGHT=1` is a debugging
escape hatch and should not be used in a normal deployment.

On shutdown, SkillFS receives `SIGTERM` as PID 1 and unmounts the FUSE view. If
a previous process was killed before cleanup, the next preflight removes a
residual FUSE mount at the configured mountpoint. It refuses to unmount any
non-FUSE filesystem found there, which protects a misconfigured volume path.

## Troubleshoot

```bash
kubectl -n "$NS" describe pod "$POD"
kubectl -n "$NS" logs "$POD" -c skillfs
kubectl -n "$NS" logs "$POD" -c skillfs --previous
kubectl -n "$NS" get events --sort-by=.lastTimestamp
```

Common causes are blocked privileged containers, missing `/dev/fuse`, incorrect
mount propagation, an unreadable probe file, or a read-only source volume.

Preflight failures include a stable numeric code in the container log. Codes
10 through 16 cover invalid configuration, the FUSE device, `fusermount3`, the
source root, the mountpoint, `fuse.conf`, and residual mount cleanup in that
order.

## Cleanup

```bash
kubectl delete namespace "$NS" --wait=true
```

`emptyDir` does not survive Pod recreation. Use a PVC when skill changes must
persist across Pods.
