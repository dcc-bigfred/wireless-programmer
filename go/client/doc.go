// Package client talks to a running wireless-programmer daemon over its
// Unix control socket.
//
// Protocol: 4-byte little-endian length prefix + UTF-8 JSON frame (max 1 MiB),
// matching the microinit IPC convention.
package client
