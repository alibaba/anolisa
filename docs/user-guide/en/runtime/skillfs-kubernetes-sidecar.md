# Run SkillFS as a Kubernetes Sidecar

[中文版](../../zh/runtime/skillfs-kubernetes-sidecar.md)

Run SkillFS beside a Kubernetes workload so the workload reads the SkillFS
view without mounting the physical skill source. The SkillFS container owns the
FUSE mount; the workload stays non-privileged and receives the propagated view.

## Prerequisites

- Kubernetes 1.29 or later.
- Linux nodes with `/dev/fuse`.
- Permission to run the SkillFS sidecar as privileged.
- `docker buildx` and `kubectl`.
- A registry that the cluster can pull from.

## Build and push the image

Build for the target node architecture:

```bash
export IMAGE=registry.example.com/anolisa/skillfs-sidecar:0.4.0
export PLATFORM=linux/amd64

docker buildx build \
  --platform "$PLATFORM" \
  -f src/skillfs/container/Dockerfile \
  -t "$IMAGE" \
  --push \
  src/skillfs
```

Use `linux/arm64` for ARM64 nodes.

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
3. set `SKILLFS_PROBE_FILE` to a stable file in the mounted view;
4. replace the `agent` image and command;
5. keep `Bidirectional` on the SkillFS mount and `HostToContainer` on the
   workload mount.

The workload readiness probe should read meaningful SkillFS content, not only
check the directory or run `skillfs --version`.

## Troubleshoot

```bash
kubectl -n "$NS" describe pod "$POD"
kubectl -n "$NS" logs "$POD" -c skillfs
kubectl -n "$NS" logs "$POD" -c skillfs --previous
kubectl -n "$NS" get events --sort-by=.lastTimestamp
```

Common causes are blocked privileged containers, missing `/dev/fuse`, incorrect
mount propagation, an unreadable probe file, or a read-only source volume.

## Cleanup

```bash
kubectl delete namespace "$NS" --wait=true
```

`emptyDir` does not survive Pod recreation. Use a PVC when skill changes must
persist across Pods.
