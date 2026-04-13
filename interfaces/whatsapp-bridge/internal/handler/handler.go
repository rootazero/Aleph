package handler

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"sync"

	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/types/events"
	waLog "go.mau.fi/whatsmeow/waLog"
	"google.golang.org/protobuf/proto"
)

type Handler struct {
	dataDir   string
	nextID    uint64
	mu        sync.RWMutex
	cli       *whatsmeow.Client
	connected bool
	events    *EventEmitter
}

func New(dataDir string) *Handler {
	h := &Handler{
		dataDir:   dataDir,
		nextID:    1,
		connected: false,
		events:    NewEventEmitter(),
	}
	return h
}

type RPCRequest struct {
	JSONRPC string          `json:"jsonrpc"`
	ID      interface{}     `json:"id"`
	Method  string          `json:"method"`
	Params  json.RawMessage `json:"params,omitempty"`
}

type RPCResponse struct {
	JSONRPC string      `json:"jsonrpc"`
	ID      interface{} `json:"id"`
	Result  interface{} `json:"result,omitempty"`
	Error   *RPCError   `json:"error,omitempty"`
}

type RPCError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

func (h *Handler) Handle(raw json.RawMessage) interface{} {
	var req RPCRequest
	if err := json.Unmarshal(raw, &req); err != nil {
		return &RPCResponse{
			JSONRPC: "2.0",
			ID:      nil,
			Error:   &RPCError{Code: -32700, Message: fmt.Sprintf("parse error: %v", err)},
		}
	}

	if req.JSONRPC != "2.0" {
		return &RPCResponse{
			JSONRPC: "2.0",
			ID:      req.ID,
			Error:   &RPCError{Code: -32600, Message: "invalid request"},
		}
	}

	var result interface{}
	var errMsg string

	switch req.Method {
	case "bridge.connect":
		result, errMsg = h.handleConnect(req.Params)
	case "bridge.disconnect":
		result, errMsg = h.handleDisconnect(req.Params)
	case "bridge.send":
		result, errMsg = h.handleSend(req.Params)
	case "bridge.send_reaction":
		result, errMsg = h.handleSendReaction(req.Params)
	case "bridge.ping":
		result, errMsg = h.handlePing(req.Params)
	case "bridge.status":
		result, errMsg = h.handleStatus(req.Params)
	default:
		errMsg = fmt.Sprintf("unknown method: %s", req.Method)
	}

	resp := &RPCResponse{
		JSONRPC: "2.0",
		ID:      req.ID,
	}

	if errMsg != "" {
		resp.Error = &RPCError{Code: -32601, Message: errMsg}
	} else {
		resp.Result = result
	}

	return resp
}

func (h *Handler) handleConnect(_ json.RawMessage) (interface{}, string) {
	h.mu.Lock()
	defer h.mu.Unlock()

	if h.connected {
		return map[string]interface{}{
			"ok":          true,
			"device_name": "Aleph",
			"phone":       "already_connected",
		}, ""
	}

	ctx := context.Background()
	dbLog := waLog.Stdout("DB", "DEBUG", true)
	container, err := sqlstore.New(ctx, "sqlite3", "file:"+h.dataDir+"/whatsmeow.db?_foreign_keys=on", dbLog)
	if err != nil {
		return nil, fmt.Sprintf("failed to create DB: %v", err)
	}

	device, err := container.GetFirstDevice(ctx)
	if err != nil {
		return nil, fmt.Sprintf("failed to get device: %v", err)
	}

	clientLog := waLog.Stdout("Client", "DEBUG", true)
	h.cli = whatsmeow.NewClient(device, clientLog)
	h.cli.AddEventHandler(h.handleWAMEvent)

	if h.cli.Store.ID == nil {
		qrChan, err := h.cli.GetQRChannel(ctx)
		if err != nil {
			return nil, fmt.Sprintf("failed to get QR channel: %v", err)
		}

		go h.handleQRChannel(qrChan)

		if err := h.cli.Connect(); err != nil {
			return nil, fmt.Sprintf("failed to connect: %v", err)
		}
	} else {
		if err := h.cli.Connect(); err != nil {
			return nil, fmt.Sprintf("failed to connect: %v", err)
		}
	}

	h.connected = true
	return map[string]interface{}{
		"ok":          true,
		"device_name": "Aleph",
		"phone":       "connecting",
	}, ""
}

func (h *Handler) handleQRChannel(qrChan <-chan whatsmeow.QRChannelItem) {
	for evt := range qrChan {
		switch evt.Event {
		case whatsmeow.QRChannelEventCode:
			h.events.Emit(map[string]interface{}{
				"type":             "qr",
				"qr_data":          evt.Code,
				"expires_in_secs":   int64(evt.Timeout.Seconds()),
			})
		case whatsmeow.QRChannelEventError:
			h.events.Emit(map[string]interface{}{
				"type":    "error",
				"message": evt.Error.Error(),
			})
		default:
			h.events.Emit(map[string]interface{}{
				"type": evt.Event,
			})
		}
	}
}

func (h *Handler) handleWAMEvent(evt interface{}) {
	switch e := evt.(type) {
	case *events.Connected:
		info := h.cli.GetInfo()
		h.events.Emit(map[string]interface{}{
			"type":         "connected",
			"device_name":  info.DeviceName,
			"phone_number": info.PhoneNumber,
		})

	case *events.Disconnected:
		h.events.Emit(map[string]interface{}{
			"type":   "disconnected",
			"reason": "server_disconnect",
		})

	case *events.Message:
		h.handleMessage(e)

	case *events.Receipt:
		h.handleReceipt(e)

	case *events.Presence:
		h.handlePresence(e)

	case *events.ChatPresence:
		h.handleChatPresence(e)
	}
}

func (h *Handler) handleMessage(e *events.Message) {
	info := e.Info
	chatID := info.Chat.String()
	senderID := info.Sender.String()
	msgID := info.ID
	isGroup := info.IsGroup

	var text string
	if e.Message.GetConversation() != "" {
		text = e.Message.GetConversation()
	} else if e.Message.GetExtendedTextMessage() != nil {
		text = e.Message.GetExtendedTextMessage().GetText()
	}

	h.events.Emit(map[string]interface{}{
		"type":       "message",
		"from":       senderID,
		"from_name":  info.PushName,
		"chat_id":    chatID,
		"text":       text,
		"timestamp":  info.Timestamp.Unix(),
		"message_id": msgID,
		"is_group":   isGroup,
	})
}

func (h *Handler) handleReceipt(e *events.Receipt) {
	if len(e.MessageIDs) == 0 {
		return
	}

	receiptType := "read"
	if e.Type == events.ReceiptTypeDelivered {
		receiptType = "delivered"
	} else if e.Type == events.ReceiptTypePlayed {
		receiptType = "played"
	}

	h.events.Emit(map[string]interface{}{
		"type":         "receipt",
		"message_id":   e.MessageIDs[0],
		"receipt_type": receiptType,
	})
}

func (h *Handler) handlePresence(e *events.Presence) {
	presence := "unknown"
	if !e.Unavailable {
		presence = "available"
	}

	h.events.Emit(map[string]interface{}{
		"type":      "presence",
		"jid":       e.From.String(),
		"presence":  presence,
		"last_seen": e.LastSeen.Unix(),
	})
}

func (h *Handler) handleChatPresence(e *events.ChatPresence) {
	presence := "unknown"
	switch e.State {
	case types.ChatPresenceComposing:
		presence = "typing"
	case types.ChatPresencePaused:
		presence = "paused"
	}

	h.events.Emit(map[string]interface{}{
		"type":     "presence",
		"jid":      e.JID.String(),
		"presence": presence,
	})
}

func (h *Handler) handleDisconnect(_ json.RawMessage) (interface{}, string) {
	h.mu.Lock()
	defer h.mu.Unlock()

	if h.cli != nil {
		h.cli.Disconnect()
	}
	h.connected = false

	return map[string]interface{}{"ok": true}, ""
}

type SendParams struct {
	To    string `json:"to"`
	Text  string `json:"text"`
	Media *struct {
		MimeType string  `json:"mime_type"`
		Data     string  `json:"data"`
		Filename *string `json:"filename,omitempty"`
	} `json:"media,omitempty"`
	ReplyTo *string `json:"reply_to,omitempty"`
}

func (h *Handler) handleSend(raw json.RawMessage) (interface{}, string) {
	h.mu.RLock()
	defer h.mu.RUnlock()

	if !h.connected || h.cli == nil {
		return nil, "not connected"
	}

	var params SendParams
	if err := json.Unmarshal(raw, &params); err != nil {
		return nil, fmt.Sprintf("invalid params: %v", err)
	}

	recvJID, err := types.ParseJID(params.To)
	if err != nil {
		return nil, fmt.Sprintf("invalid recipient JID: %v", err)
	}

	msg := &waE2E.Message{
		Conversation: proto.String(params.Text),
	}

	resp, err := h.cli.SendMessage(context.Background(), recvJID, msg, nil)
	if err != nil {
		return nil, fmt.Sprintf("send failed: %v", err)
	}

	return map[string]interface{}{
		"id": resp.ID,
	}, ""
}

type SendReactionParams struct {
	To        string `json:"to"`
	MessageID string `json:"message_id"`
	Reaction  string `json:"reaction"`
}

func (h *Handler) handleSendReaction(raw json.RawMessage) (interface{}, string) {
	h.mu.RLock()
	defer h.mu.RUnlock()

	if !h.connected || h.cli == nil {
		return nil, "not connected"
	}

	var params SendReactionParams
	if err := json.Unmarshal(raw, &params); err != nil {
		return nil, fmt.Sprintf("invalid params: %v", err)
	}

	recvJID, err := types.ParseJID(params.To)
	if err != nil {
		return nil, fmt.Sprintf("invalid recipient JID: %v", err)
	}

	reaction := &waE2E.Message{
		ReactionMessage: &waE2E.ReactionMessage{
			Text: proto.String(params.Reaction),
		},
	}

	_, err = h.cli.SendMessage(context.Background(), recvJID, reaction, nil)
	if err != nil {
		return nil, fmt.Sprintf("send reaction failed: %v", err)
	}

	log.Printf("send_reaction: to=%s msg=%s reaction=%s", params.To, params.MessageID, params.Reaction)

	return map[string]interface{}{"ok": true}, ""
}

func (h *Handler) handlePing(_ json.RawMessage) (interface{}, string) {
	return map[string]interface{}{
		"pong":   true,
		"rtt_ms": nil,
	}, ""
}

func (h *Handler) handleStatus(_ json.RawMessage) (interface{}, string) {
	h.mu.RLock()
	defer h.mu.RUnlock()

	var deviceName, phoneNumber string
	if h.cli != nil {
		info := h.cli.GetInfo()
		deviceName = info.DeviceName
		phoneNumber = info.PhoneNumber
	}

	return map[string]interface{}{
		"connected":    h.connected,
		"device_name":  deviceName,
		"phone_number": phoneNumber,
	}, ""
}

type EventEmitter struct {
	mu        sync.RWMutex
	listeners []chan<- map[string]interface{}
}

func NewEventEmitter() *EventEmitter {
	return &EventEmitter{
		listeners: make([]chan<- map[string]interface{}, 0),
	}
}

func (e *EventEmitter) AddListener(ch chan<- map[string]interface{}) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.listeners = append(e.listeners, ch)
}

func (e *EventEmitter) Emit(event map[string]interface{}) {
	e.mu.RLock()
	defer e.mu.RUnlock()
	for _, ch := range e.listeners {
		select {
		case ch <- event:
		default:
		}
	}
}
