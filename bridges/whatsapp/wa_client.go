package main

import (
	"context"
	"encoding/base64"
	"fmt"
	"log"
	"path/filepath"
	"sync"
	"time"

	_ "github.com/mattn/go-sqlite3"
	"go.mau.fi/whatsmeow"
	"go.mau.fi/whatsmeow/proto/waE2E"
	"go.mau.fi/whatsmeow/store/sqlstore"
	"go.mau.fi/whatsmeow/types"
	"go.mau.fi/whatsmeow/types/events"
	waLog "go.mau.fi/whatsmeow/util/log"
	"google.golang.org/protobuf/proto"
)

// WAClient wraps a whatsmeow client for the JSON-RPC bridge.
type WAClient struct {
	client    *whatsmeow.Client
	container *sqlstore.Container
	eventCh   chan map[string]interface{} // events pushed to Rust via JSON-RPC notifications
	mu        sync.Mutex
	connected bool
	device    string // device name from PairSuccess
	phone     string // phone number from PairSuccess
}

// NewWAClient creates a new WhatsApp client backed by SQLite session storage.
func NewWAClient(dataDir string) (*WAClient, error) {
	dbPath := filepath.Join(dataDir, "whatsmeow.db")
	dbURI := fmt.Sprintf("file:%s?_foreign_keys=on", dbPath)

	dbLog := waLog.Stdout("DB", "WARN", true)
	container, err := sqlstore.New(context.Background(), "sqlite3", dbURI, dbLog)
	if err != nil {
		return nil, fmt.Errorf("failed to create sqlstore: %w", err)
	}

	deviceStore, err := container.GetFirstDevice(context.Background())
	if err != nil {
		return nil, fmt.Errorf("failed to get device store: %w", err)
	}

	clientLog := waLog.Stdout("Client", "WARN", true)
	client := whatsmeow.NewClient(deviceStore, clientLog)

	w := &WAClient{
		client:    client,
		container: container,
		eventCh:   make(chan map[string]interface{}, 128),
	}

	// Register the event handler
	client.AddEventHandler(w.handleEvent)

	return w, nil
}

// Connect initiates a connection. If paired, reconnects; otherwise starts QR pairing.
func (w *WAClient) Connect() error {
	w.mu.Lock()
	defer w.mu.Unlock()

	if w.connected {
		return nil // already connected
	}

	if w.client.Store.ID == nil {
		// No session data — need QR pairing
		return w.connectWithQR()
	}

	// Has session data — reconnect directly
	err := w.client.Connect()
	if err != nil {
		return fmt.Errorf("failed to connect: %w", err)
	}

	return nil
}

// connectWithQR starts the QR pairing flow. Must be called with w.mu held.
func (w *WAClient) connectWithQR() error {
	qrChan, err := w.client.GetQRChannel(context.Background())
	if err != nil {
		return fmt.Errorf("failed to get QR channel: %w", err)
	}

	// Connect must be called after GetQRChannel
	err = w.client.Connect()
	if err != nil {
		return fmt.Errorf("failed to connect for QR pairing: %w", err)
	}

	// Process QR events in a background goroutine
	go w.processQRChannel(qrChan)

	return nil
}

// processQRChannel forwards QR events from whatsmeow as bridge events.
func (w *WAClient) processQRChannel(qrChan <-chan whatsmeow.QRChannelItem) {
	for evt := range qrChan {
		switch evt.Event {
		case "code":
			// New QR code to display
			expiresIn := uint64(evt.Timeout.Seconds())
			if expiresIn == 0 {
				expiresIn = 60 // default 60 seconds
			}
			w.pushEvent(map[string]interface{}{
				"type":            "qr",
				"qr_data":         evt.Code,
				"expires_in_secs": expiresIn,
			})

		case "success":
			// QR scanning succeeded, connection will follow via event handler
			log.Printf("QR pairing successful")

		case "timeout":
			w.pushEvent(map[string]interface{}{
				"type": "qr_expired",
			})
			w.pushEvent(map[string]interface{}{
				"type":    "error",
				"message": "QR code timeout, please reconnect to get a new QR code",
			})

		default:
			// Handle error events
			errMsg := fmt.Sprintf("QR channel event: %s", evt.Event)
			if evt.Error != nil {
				errMsg = fmt.Sprintf("QR channel error: %s (%v)", evt.Event, evt.Error)
			}
			log.Printf("%s", errMsg)
			w.pushEvent(map[string]interface{}{
				"type":    "error",
				"message": errMsg,
			})
		}
	}
}

// Disconnect closes the WhatsApp connection.
func (w *WAClient) Disconnect() {
	w.mu.Lock()
	defer w.mu.Unlock()

	if w.client != nil {
		w.client.Disconnect()
	}
	w.connected = false
}

// Send sends a text message to the specified JID. Returns the message ID.
func (w *WAClient) Send(to, text string, media *MediaPayload, replyTo string) (string, error) {
	w.mu.Lock()
	if !w.connected {
		w.mu.Unlock()
		return "", fmt.Errorf("not connected to WhatsApp")
	}
	w.mu.Unlock()

	jid, err := types.ParseJID(to)
	if err != nil {
		return "", fmt.Errorf("invalid JID %q: %w", to, err)
	}

	// Build the protobuf message
	msg := &waE2E.Message{}

	if media != nil {
		// TODO: implement media upload via client.Upload() + ImageMessage/DocumentMessage
		log.Printf("WARNING: media sending not yet implemented, sending text only")
	}

	// Build text message with optional reply context
	if replyTo != "" {
		msg.ExtendedTextMessage = &waE2E.ExtendedTextMessage{
			Text: proto.String(text),
			ContextInfo: &waE2E.ContextInfo{
				StanzaID: proto.String(replyTo),
			},
		}
	} else {
		msg.Conversation = proto.String(text)
	}

	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	resp, err := w.client.SendMessage(ctx, jid, msg)
	if err != nil {
		return "", fmt.Errorf("failed to send message: %w", err)
	}

	return resp.ID, nil
}

// Status returns the current connection state.
func (w *WAClient) Status() (connected bool, deviceName string, phoneNumber string) {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.connected, w.device, w.phone
}

// handleEvent converts whatsmeow events to bridge events for the Rust side.
func (w *WAClient) handleEvent(evt interface{}) {
	switch v := evt.(type) {
	case *events.Connected:
		w.mu.Lock()
		w.connected = true
		w.mu.Unlock()

		// Try to get device info from store
		deviceName := w.device
		phoneNumber := w.phone
		if w.client.Store.ID != nil {
			if phoneNumber == "" {
				phoneNumber = w.client.Store.ID.User
			}
		}

		w.pushEvent(map[string]interface{}{
			"type":         "connected",
			"device_name":  deviceName,
			"phone_number": phoneNumber,
		})

		// Push ready event after connected
		w.pushEvent(map[string]interface{}{
			"type": "ready",
		})

	case *events.Disconnected:
		w.mu.Lock()
		w.connected = false
		w.mu.Unlock()

		w.pushEvent(map[string]interface{}{
			"type":   "disconnected",
			"reason": "server disconnected",
		})

	case *events.LoggedOut:
		w.mu.Lock()
		w.connected = false
		w.mu.Unlock()

		w.pushEvent(map[string]interface{}{
			"type":   "disconnected",
			"reason": fmt.Sprintf("logged out: %v", v.Reason),
		})

	case *events.PairSuccess:
		w.mu.Lock()
		w.device = v.Platform
		w.phone = v.ID.User
		w.mu.Unlock()

		log.Printf("Pair success: platform=%s, phone=%s", v.Platform, v.ID.User)

		// Push scanned event (QR was scanned successfully)
		w.pushEvent(map[string]interface{}{
			"type": "scanned",
		})

	case *events.HistorySync:
		// Push syncing event with progress indication
		// whatsmeow doesn't provide granular progress, so we push 0.5 as a placeholder
		w.pushEvent(map[string]interface{}{
			"type":     "syncing",
			"progress": 0.5,
		})

	case *events.Message:
		w.handleMessage(v)

	case *events.Receipt:
		w.handleReceipt(v)
	}
}

// handleMessage converts a whatsmeow Message event to a bridge Message event.
func (w *WAClient) handleMessage(v *events.Message) {
	// Skip messages from ourselves
	if v.Info.IsFromMe {
		return
	}

	// Extract text content from the message
	text := extractText(v.Message)

	// Extract sender info
	from := v.Info.Sender.String()
	chatID := v.Info.Chat.String()
	isGroup := v.Info.IsGroup
	msgID := v.Info.ID
	timestamp := v.Info.Timestamp.Unix()
	pushName := v.Info.PushName

	// Build the bridge event
	event := map[string]interface{}{
		"type":       "message",
		"from":       from,
		"chat_id":    chatID,
		"text":       text,
		"timestamp":  timestamp,
		"message_id": msgID,
		"is_group":   isGroup,
	}

	// Optional fields
	if pushName != "" {
		event["from_name"] = pushName
	}

	// Extract reply context
	if ci := extractContextInfo(v.Message); ci != nil && ci.StanzaID != nil {
		event["reply_to"] = *ci.StanzaID
	}

	// Extract and download media if present
	if mediaPayload := w.extractAndDownloadMedia(v); mediaPayload != nil {
		event["media"] = mediaPayload
	}

	w.pushEvent(event)
}

// handleReceipt converts a whatsmeow Receipt event to a bridge Receipt event.
func (w *WAClient) handleReceipt(v *events.Receipt) {
	receiptType := "delivered"
	switch v.Type {
	case types.ReceiptTypeRead, types.ReceiptTypeReadSelf:
		receiptType = "read"
	case types.ReceiptTypePlayed, types.ReceiptTypePlayedSelf:
		receiptType = "played"
	case types.ReceiptTypeDelivered:
		receiptType = "delivered"
	default:
		// Skip receipt types we don't care about (sender, retry, etc.)
		return
	}

	for _, msgID := range v.MessageIDs {
		w.pushEvent(map[string]interface{}{
			"type":         "receipt",
			"message_id":   msgID,
			"receipt_type": receiptType,
		})
	}
}

// pushEvent sends an event to the channel. Non-blocking; drops if full.
func (w *WAClient) pushEvent(event map[string]interface{}) {
	select {
	case w.eventCh <- event:
	default:
		log.Printf("WARNING: event channel full, dropping event: %v", event["type"])
	}
}

// extractText extracts text content from a whatsmeow message.
func extractText(msg *waE2E.Message) string {
	if msg == nil {
		return ""
	}

	if msg.Conversation != nil {
		return *msg.Conversation
	}
	if msg.ExtendedTextMessage != nil && msg.ExtendedTextMessage.Text != nil {
		return *msg.ExtendedTextMessage.Text
	}
	if msg.ImageMessage != nil && msg.ImageMessage.Caption != nil {
		return *msg.ImageMessage.Caption
	}
	if msg.VideoMessage != nil && msg.VideoMessage.Caption != nil {
		return *msg.VideoMessage.Caption
	}
	if msg.DocumentMessage != nil && msg.DocumentMessage.Caption != nil {
		return *msg.DocumentMessage.Caption
	}

	return ""
}

// extractContextInfo extracts ContextInfo from a message for reply detection.
func extractContextInfo(msg *waE2E.Message) *waE2E.ContextInfo {
	if msg == nil {
		return nil
	}

	if msg.ExtendedTextMessage != nil {
		return msg.ExtendedTextMessage.ContextInfo
	}
	if msg.ImageMessage != nil {
		return msg.ImageMessage.ContextInfo
	}
	if msg.VideoMessage != nil {
		return msg.VideoMessage.ContextInfo
	}
	if msg.DocumentMessage != nil {
		return msg.DocumentMessage.ContextInfo
	}
	if msg.AudioMessage != nil {
		return msg.AudioMessage.ContextInfo
	}

	return nil
}

// MediaPayload matches the Rust bridge_protocol::MediaPayload struct.
type MediaPayload struct {
	MimeType string  `json:"mime_type"`
	Data     string  `json:"data"` // base64-encoded
	Filename *string `json:"filename,omitempty"`
}

// extractAndDownloadMedia downloads media from an incoming message.
// Returns nil if no media is present.
func (w *WAClient) extractAndDownloadMedia(v *events.Message) *MediaPayload {
	msg := v.Message
	if msg == nil {
		return nil
	}

	var (
		mimeType string
		filename *string
		mediaMsg whatsmeow.DownloadableMessage
	)

	switch {
	case msg.ImageMessage != nil:
		im := msg.ImageMessage
		mimeType = im.GetMimetype()
		mediaMsg = im
	case msg.AudioMessage != nil:
		am := msg.AudioMessage
		mimeType = am.GetMimetype()
		mediaMsg = am
	case msg.VideoMessage != nil:
		vm := msg.VideoMessage
		mimeType = vm.GetMimetype()
		mediaMsg = vm
	case msg.DocumentMessage != nil:
		dm := msg.DocumentMessage
		mimeType = dm.GetMimetype()
		if dm.FileName != nil {
			fn := *dm.FileName
			filename = &fn
		}
		mediaMsg = dm
	case msg.StickerMessage != nil:
		sm := msg.StickerMessage
		mimeType = sm.GetMimetype()
		mediaMsg = sm
	default:
		return nil
	}

	if mediaMsg == nil {
		return nil
	}

	// Download the media bytes from WhatsApp servers
	data, err := w.client.Download(mediaMsg)
	if err != nil {
		log.Printf("WARNING: failed to download media (%s): %v", mimeType, err)
		// Return metadata without data so the Rust side knows media was present
		return &MediaPayload{
			MimeType: mimeType,
			Data:     "",
			Filename: filename,
		}
	}

	return &MediaPayload{
		MimeType: mimeType,
		Data:     base64.StdEncoding.EncodeToString(data),
		Filename: filename,
	}
}
