// Package parity is the harness that compares Go output with the Python
// oracle (legacy/pulse.py) on the /tmp/pulse-parity snapshot: Oracle(t, home,
// args...) runs the oracle and Snapshot(t) locates the snapshot, both skipping
// when they are unavailable.
package parity
