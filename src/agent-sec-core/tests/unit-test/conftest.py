"""Shared test infrastructure — add agent-sec-cli to sys.path."""

import os
import sys

# Add the agent-sec-cli source directory to sys.path
_CLI_SRC_DIR = os.path.join(
    os.path.dirname(__file__), "..", "..", "agent-sec-cli", "src"
)
sys.path.insert(0, os.path.abspath(_CLI_SRC_DIR))
