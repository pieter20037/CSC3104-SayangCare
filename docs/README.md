# Project Documentation

Start here to find the project guides and implementation notes.

## Windows and macOS

The root [README](../README.md) has separate Windows PowerShell and macOS Terminal setup instructions. Commands in a `powershell` block use PowerShell syntax; commands in `bash` or `sh` blocks use POSIX shell syntax and work in macOS Terminal, Linux shells, Git Bash, or WSL. Docker, Cargo, `kubectl`, HTTP URLs, and SQL examples are otherwise the same on both systems. Phase guides identify shell-specific steps and provide alternatives where needed.

## Running and Testing

- [Phase 1: Session lifecycle and recovery](PHASE_1_TEST_GUIDE.md)
- [Phase 2: Raft volunteer queue run guide](PHASE_2.md)
- [Phase 3: Resilience and live telephony](PHASE_3.md)
- [Phase 4: Kubernetes deployment and HA validation](PHASE_4_KUBERNETES.md)
- [Raft leader election demo](RAFT_DEMO_GUIDE.md)
- [Test execution quick reference](TEST_EXECUTION_QUICK_REFERENCE.md)

## Architecture and Implementation

- [Implementation summary](IMPLEMENTATION_SUMMARY.md)
- [Merged implementations guide](MERGED_IMPLEMENTATIONS_GUIDE.md)
- [Flow diagrams](FLOW_DIAGRAMS.md)
- [Circuit breaker and session-store comparison](COMPARISON_TABLE.md)
- [Review index](REVIEW_INDEX.md)

## Report

- [Group 25 report source](GROUP25.md)

The main project overview and local development instructions remain in the root [README](../README.md).
