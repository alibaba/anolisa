#!/bin/zsh
# Script to complete ebpf-ifc-engine crate creation and deploy to remote
# Run from project root: /Users/xzw/Program/anolisa

set -euo pipefail

CRATE_DIR="/Users/xzw/Program/anolisa/.worktrees/deploy-121-199-33-125/src/agentsight/crates/ebpf-ifc-engine"
BPF_SRC="/Users/xzw/Program/anolisa/oslevel-harness/actplane/bpf"
CARGO_CHECKOUT="/Users/xzw/.cargo/git/checkouts/actplane-cfcc2854c9bfeb3f/a62e5d9/bpf"

echo "=== Part 1: Creating local ebpf-ifc-engine crate ==="

# Ensure directories exist
mkdir -p "$CRATE_DIR/src" "$CRATE_DIR/prebuilt"

# Copy BPF C/H source files (with TTL modifications) from local bpf/
echo "Copying BPF source files from local bpf/ (with TTL)..."
cp "$BPF_SRC/taint.h" "$CRATE_DIR/"
cp "$BPF_SRC/taint_engine.bpf.h" "$CRATE_DIR/"
cp "$BPF_SRC/process.bpf.c" "$CRATE_DIR/"
cp "$BPF_SRC/process.c" "$CRATE_DIR/"
cp "$BPF_SRC/process.h" "$CRATE_DIR/"
cp "$BPF_SRC/capability.bpf.h" "$CRATE_DIR/"
cp "$BPF_SRC/channel.bpf.h" "$CRATE_DIR/"
cp "$BPF_SRC/test_taint.c" "$CRATE_DIR/"
cp "$BPF_SRC/Makefile" "$CRATE_DIR/"

# Copy prebuilt .o (will be rebuilt on remote with ACTPLANE_REBUILD_BPF=1)
echo "Copying prebuilt process.bpf.o..."
cp "$BPF_SRC/prebuilt/process.bpf.o" "$CRATE_DIR/prebuilt/"

# Copy Rust source files from LOCAL bpf/src (which has the latest TTL version)
echo "Copying Rust source files from local bpf/src..."
cp "$BPF_SRC/src/lib.rs" "$CRATE_DIR/src/"
cp "$BPF_SRC/src/capability.rs" "$CRATE_DIR/src/"

# Cargo.toml and build.rs already created by the agent
echo "Cargo.toml and build.rs already exist."

# Verify crate structure
echo ""
echo "=== Verifying crate structure ==="
ls -la "$CRATE_DIR/"
echo ""
ls -la "$CRATE_DIR/src/"
echo ""
ls -la "$CRATE_DIR/prebuilt/"

echo ""
echo "=== Part 1 complete! ==="
echo ""

# === Part 2: Deploy to remote ===
echo "=== Part 2: Deploying to 121.199.33.125 ==="

# Step 2.1: Sync source to remote
echo "Step 2.1: rsync source to remote..."
rsync -az --delete \
  /Users/xzw/Program/anolisa/.worktrees/deploy-121-199-33-125/src/agentsight/ \
  root@121.199.33.125:/root/agentsight-sensitive-file-build/core/_thirdparty/coolbpf/src/agentsight/

echo "rsync complete."

# Step 2.2: Check for clang on remote
echo "Step 2.2: Checking for clang on remote..."
ssh root@121.199.33.125 'which clang && clang --version | head -2' || {
  echo "clang not found, installing..."
  ssh root@121.199.33.125 'yum install -y clang llvm || apt-get install -y clang llvm'
}

# Step 2.3: Remote compile
echo "Step 2.3: Compiling on remote..."
ssh root@121.199.33.125 'cd /root/agentsight-sensitive-file-build/core/_thirdparty/coolbpf/src/agentsight && ACTPLANE_REBUILD_BPF=1 cargo build --release --features actplane 2>&1 | tail -50'

# Step 2.4: Build frontend locally
echo "Step 2.4: Building frontend locally..."
cd /Users/xzw/Program/anolisa/.worktrees/deploy-121-199-33-125/src/agentsight/dashboard
AGENTSIGHT_EMBED=1 npx webpack --mode production

# Sync frontend dist to remote
echo "Syncing frontend to remote..."
rsync -az \
  /Users/xzw/Program/anolisa/.worktrees/deploy-121-199-33-125/src/agentsight/frontend-dist/ \
  root@121.199.33.125:/root/agentsight-sensitive-file-build/core/_thirdparty/coolbpf/src/agentsight/frontend-dist/

# Step 2.5: Install binaries + restart
echo "Step 2.5: Installing binaries and restarting services..."
ssh root@121.199.33.125 'systemctl stop agentsight agentsight-enforcer || true'
ssh root@121.199.33.125 'cd /root/agentsight-sensitive-file-build/core/_thirdparty/coolbpf/src/agentsight && cp target/release/agentsight /usr/local/bin/agentsight && cp target/release/agentsight-enforcer /usr/local/bin/agentsight-enforcer'
ssh root@121.199.33.125 'setcap cap_sys_admin,cap_bpf,cap_perfmon,cap_sys_ptrace,cap_net_admin,cap_dac_read_search+ep /usr/local/bin/agentsight-enforcer'

# Clear old BPF pin (ABI changed)
echo "Clearing old BPF pin..."
ssh root@121.199.33.125 'rm -rf /sys/fs/bpf/actplane'

# Restart services
echo "Restarting services..."
ssh root@121.199.33.125 'systemctl restart agentsight-enforcer && systemctl restart agentsight && sleep 3'

# Step 2.6: Verify
echo ""
echo "=== Step 2.6: Verification ==="
ssh root@121.199.33.125 'systemctl status agentsight agentsight-enforcer --no-pager' || true
echo ""
ssh root@121.199.33.125 'curl -s http://localhost:7396/health || curl -s http://localhost:9097/health' || true
echo ""

echo "=== Deployment complete! ==="
