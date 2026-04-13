package main

import (
	"flag"
	"log"
	"os"
	"os/signal"
	"syscall"

	"github.com/rootazero/aleph/interfaces/whatsapp-bridge/internal/handler"
	"github.com/rootazero/aleph/interfaces/whatsapp-bridge/internal/socket"
)

func main() {
	socketPath := flag.String("socket", "", "Unix domain socket path")
	dataDir := flag.String("data-dir", "", "Data directory path")
	flag.Parse()

	if *socketPath == "" {
		log.Fatal("--socket is required")
	}
	if *dataDir == "" {
		log.Fatal("--data-dir is required")
	}

	log.Printf("whatsapp-bridge starting...")
	log.Printf("  socket: %s", *socketPath)
	log.Printf("  data-dir: %s", *dataDir)

	if err := os.RemoveAll(*socketPath); err != nil {
		log.Fatalf("failed to remove stale socket: %v", err)
	}

	h := handler.New(*dataDir)

	srv, err := socket.NewServer(*socketPath, h)
	if err != nil {
		log.Fatalf("failed to create socket server: %v", err)
	}
	defer srv.Close()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)

	go func() {
		<-sigCh
		log.Println("shutting down...")
		srv.Close()
		os.Exit(0)
	}()

	log.Printf("socket server listening on %s", *socketPath)
	if err := srv.Serve(); err != nil {
		log.Fatalf("socket server error: %v", err)
	}
}
