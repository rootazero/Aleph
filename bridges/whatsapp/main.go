// Package main implements the whatsapp-bridge binary, a thin sidecar
// that wraps the whatsmeow library and exposes a JSON-RPC 2.0 interface
// over a Unix domain socket. It is managed as a child process by the
// Aleph server's BridgeManager.
//
// Usage:
//
//	whatsapp-bridge --socket /tmp/aleph-wa.sock --data-dir ~/.aleph/whatsapp/data
package main

import (
	"flag"
	"log"
	"net"
	"os"
	"os/signal"
	"syscall"
)

func main() {
	socketPath := flag.String("socket", "/tmp/aleph-wa.sock", "Unix domain socket path for JSON-RPC IPC")
	dataDir := flag.String("data-dir", "", "Directory for WhatsApp session data (SQLite)")
	flag.Parse()

	if *dataDir == "" {
		homeDir, err := os.UserHomeDir()
		if err != nil {
			log.Fatal("Failed to determine home directory: ", err)
		}
		defaultDir := homeDir + "/.aleph/whatsapp/data"
		dataDir = &defaultDir
	}

	// Ensure data directory exists
	if err := os.MkdirAll(*dataDir, 0700); err != nil {
		log.Fatalf("Failed to create data directory %s: %v", *dataDir, err)
	}

	// Remove stale socket file from a previous run
	os.Remove(*socketPath)

	// Listen on Unix domain socket
	listener, err := net.Listen("unix", *socketPath)
	if err != nil {
		log.Fatalf("Failed to listen on %s: %v", *socketPath, err)
	}
	defer listener.Close()
	defer os.Remove(*socketPath)

	// Create the WhatsApp client wrapper
	client, err := NewWAClient(*dataDir)
	if err != nil {
		log.Fatalf("Failed to create WhatsApp client: %v", err)
	}

	// Handle shutdown signals (SIGINT, SIGTERM)
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		sig := <-sigCh
		log.Printf("Received signal %s, shutting down...", sig)
		client.Disconnect()
		listener.Close()
		os.Remove(*socketPath)
		os.Exit(0)
	}()

	log.Printf("whatsapp-bridge listening on %s (data-dir: %s)", *socketPath, *dataDir)

	// Accept connections. Typically one persistent connection from aleph-server,
	// but we allow multiple for robustness during reconnects.
	for {
		conn, err := listener.Accept()
		if err != nil {
			// listener.Close() in the signal handler will cause Accept to return an error
			log.Printf("Accept error (shutting down?): %v", err)
			break
		}
		log.Printf("New connection accepted")
		go HandleConnection(conn, client)
	}
}
