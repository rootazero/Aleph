package main

import (
	"bufio"
	"encoding/json"
	"log"
	"net"
	"sync"
	"time"
)

// ─── JSON-RPC 2.0 Wire Types ───────────────────────────────────────────────

// RpcRequest represents a JSON-RPC 2.0 request from the Rust aleph server.
type RpcRequest struct {
	Jsonrpc string           `json:"jsonrpc"`
	ID      *uint64          `json:"id"`
	Method  string           `json:"method"`
	Params  json.RawMessage  `json:"params,omitempty"`
}

// RpcResponse represents a JSON-RPC 2.0 response or notification sent to Rust.
type RpcResponse struct {
	Jsonrpc string      `json:"jsonrpc"`
	ID      *uint64     `json:"id,omitempty"`
	Result  interface{} `json:"result,omitempty"`
	Error   *RpcError   `json:"error,omitempty"`
	Method  string      `json:"method,omitempty"`  // for push notifications
	Params  interface{} `json:"params,omitempty"`   // for push notifications
}

// RpcError represents a JSON-RPC 2.0 error object.
type RpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

// SendParams represents the parameters for the bridge.send RPC method.
// Matches the Rust SendRequest struct.
type SendParams struct {
	To      string        `json:"to"`
	Text    string        `json:"text"`
	Media   *MediaPayload `json:"media,omitempty"`
	ReplyTo string        `json:"reply_to,omitempty"`
}

// ─── Connection Handler ─────────────────────────────────────────────────────

// HandleConnection processes a single JSON-RPC connection from aleph.
// It reads newline-delimited JSON requests, dispatches them, and writes
// responses. A background goroutine forwards events from the WAClient as
// JSON-RPC notifications.
func HandleConnection(conn net.Conn, client *WAClient) {
	defer conn.Close()

	scanner := bufio.NewScanner(conn)
	// Increase scanner buffer for large media payloads (16MB)
	scanner.Buffer(make([]byte, 0, 64*1024), 16*1024*1024)

	writer := bufio.NewWriter(conn)
	var writeMu sync.Mutex

	// Done channel to signal the event forwarder to stop
	done := make(chan struct{})
	defer close(done)

	// Start the event forwarder goroutine.
	// This reads events from the WAClient and pushes them as
	// JSON-RPC notifications (no id, with method "event.push").
	go func() {
		for {
			select {
			case event, ok := <-client.eventCh:
				if !ok {
					return
				}
				notification := RpcResponse{
					Jsonrpc: "2.0",
					Method:  "event.push",
					Params:  event,
				}
				writeMu.Lock()
				writeResponse(writer, notification)
				writeMu.Unlock()

			case <-done:
				return
			}
		}
	}()

	// Read and process JSON-RPC requests
	for scanner.Scan() {
		line := scanner.Bytes()
		if len(line) == 0 {
			continue
		}

		var req RpcRequest
		if err := json.Unmarshal(line, &req); err != nil {
			log.Printf("Failed to parse JSON-RPC request: %v", err)
			continue
		}

		// Dispatch the request
		result, rpcErr := dispatch(client, &req)

		// Build and send the response
		resp := RpcResponse{
			Jsonrpc: "2.0",
			ID:      req.ID,
			Result:  result,
			Error:   rpcErr,
		}

		writeMu.Lock()
		writeResponse(writer, resp)
		writeMu.Unlock()
	}

	if err := scanner.Err(); err != nil {
		log.Printf("Scanner error: %v", err)
	}

	log.Printf("Connection closed")
}

// dispatch routes a JSON-RPC request to the appropriate handler.
func dispatch(client *WAClient, req *RpcRequest) (interface{}, *RpcError) {
	switch req.Method {

	case "bridge.connect":
		err := client.Connect()
		if err != nil {
			return nil, &RpcError{Code: -1, Message: err.Error()}
		}
		return map[string]bool{"ok": true}, nil

	case "bridge.disconnect":
		client.Disconnect()
		return map[string]bool{"ok": true}, nil

	case "bridge.send":
		return handleSend(client, req.Params)

	case "bridge.status":
		connected, device, phone := client.Status()
		result := map[string]interface{}{
			"connected": connected,
		}
		if device != "" {
			result["device_name"] = device
		}
		if phone != "" {
			result["phone_number"] = phone
		}
		return result, nil

	case "bridge.ping":
		return map[string]interface{}{
			"pong":   true,
			"rtt_ms": nil,
		}, nil

	default:
		return nil, &RpcError{
			Code:    -32601,
			Message: "Method not found: " + req.Method,
		}
	}
}

// handleSend parses the send request parameters and dispatches the message.
func handleSend(client *WAClient, params json.RawMessage) (interface{}, *RpcError) {
	if params == nil {
		return nil, &RpcError{Code: -32602, Message: "Missing params for bridge.send"}
	}

	var p SendParams
	if err := json.Unmarshal(params, &p); err != nil {
		return nil, &RpcError{
			Code:    -32602,
			Message: "Invalid params for bridge.send: " + err.Error(),
		}
	}

	if p.To == "" {
		return nil, &RpcError{Code: -32602, Message: "Missing 'to' in bridge.send params"}
	}

	start := time.Now()
	msgID, err := client.Send(p.To, p.Text, p.Media, p.ReplyTo)
	if err != nil {
		return nil, &RpcError{Code: -1, Message: err.Error()}
	}

	log.Printf("Sent message %s to %s in %v", msgID, p.To, time.Since(start))

	return map[string]string{"id": msgID}, nil
}

// writeResponse marshals a response and writes it as a newline-delimited JSON line.
func writeResponse(writer *bufio.Writer, resp RpcResponse) {
	data, err := json.Marshal(resp)
	if err != nil {
		log.Printf("Failed to marshal response: %v", err)
		return
	}
	if _, err := writer.Write(data); err != nil {
		log.Printf("Failed to write response: %v", err)
		return
	}
	if err := writer.WriteByte('\n'); err != nil {
		log.Printf("Failed to write newline: %v", err)
		return
	}
	if err := writer.Flush(); err != nil {
		log.Printf("Failed to flush writer: %v", err)
	}
}
