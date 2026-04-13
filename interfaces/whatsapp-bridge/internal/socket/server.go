package socket

import (
	"encoding/json"
	"fmt"
	"log"
	"net"
	"sync"
	"time"

	"github.com/rootazero/aleph/interfaces/whatsapp-bridge/internal/handler"
)

type Server struct {
	path    string
	handler *handler.Handler
	mu      sync.Mutex
	done    chan struct{}
}

func NewServer(path string, h *handler.Handler) (*Server, error) {
	return &Server{
		path:    path,
		handler: h,
		done:    make(chan struct{}),
	}, nil
}

func (s *Server) Serve() error {
	ln, err := net.ListenUnix("unix", &net.UnixAddr{Name: s.path})
	if err != nil {
		return fmt.Errorf("listen: %w", err)
	}

	for {
		select {
		case <-s.done:
			return nil
		default:
		}

		ln.SetDeadline(time.Now().Add(1 * time.Second))

		conn, err := ln.Accept()
		if err != nil {
			if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
				continue
			}
			return fmt.Errorf("accept: %w", err)
		}

		go s.handleConn(conn)
	}
}

func (s *Server) handleConn(conn net.Conn) {
	defer conn.Close()

	dec := json.NewDecoder(conn)
	enc := json.NewEncoder(conn)

	for {
		var raw json.RawMessage
		if err := dec.Decode(&raw); err != nil {
			if err.Error() != "EOF" {
				log.Printf("decode error: %v", err)
			}
			return
		}

		resp := s.handler.Handle(raw)
		if resp == nil {
			continue
		}

		if err := enc.Encode(resp); err != nil {
			log.Printf("encode error: %v", err)
			return
		}

		if _, err := conn.Write([]byte("\n")); err != nil {
			log.Printf("write newline error: %v", err)
			return
		}
	}
}

func (s *Server) Close() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	close(s.done)
	return nil
}
