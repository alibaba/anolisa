# SkillFS Kubernetes Sidecar Manifests

These manifests run SkillFS as a privileged FUSE sidecar and expose its view to
a non-privileged workload container.

See the
[Kubernetes sidecar user guide](../../../../docs/user-guide/en/runtime/skillfs-kubernetes-sidecar.md)
([中文](../../../../docs/user-guide/zh/runtime/skillfs-kubernetes-sidecar.md))
for the complete workflow.

## Prerequisites

- Kubernetes 1.29 or later.
- Privileged containers and the `/dev/fuse` hostPath are allowed.
- The target nodes provide `/dev/fuse`.
- The SkillFS image is available to the cluster.

## Files

| File | Purpose |
| --- | --- |
| `00-namespace.yaml` | Isolated example namespace |
| `10-example-configmap.yaml` | Example default and secondary skills |
| `20-pod.yaml` | SkillFS sidecar and workload Pod |

## Deploy

```bash
export IMAGE=registry.example.com/anolisa/skillfs-sidecar:0.4.0
export NS=skillfs-container-example

kubectl apply -f 00-namespace.yaml
kubectl apply -f 10-example-configmap.yaml
sed "s|skillfs-sidecar:dev|$IMAGE|g" 20-pod.yaml | kubectl apply -f -
kubectl -n "$NS" wait \
  --for=condition=Ready pod/skillfs-sidecar-example \
  --timeout=300s
```

The workload readiness probe reads the transformed default skill and the
virtual `skill-discover/SKILL.md`. It also opens a secondary skill through the
path advertised by `skill-discover`. Secondary skills stay out of the directory
listing but remain readable through their advertised paths.

## Use your own skills

Before using the manifest for a workload:

1. replace `skill-source` with a PVC;
2. remove the example ConfigMap and `seed-example` init container;
3. update `SKILLFS_PROBE_FILE`;
4. replace the `agent` image and command;
5. keep the shared-volume mount propagation settings unchanged.
