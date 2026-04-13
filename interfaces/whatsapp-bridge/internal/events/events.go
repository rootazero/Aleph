package events

import (
	"encoding/json"
	"log"
)

type EventPusher struct {
	sendFunc func(event json.RawMessage) error
}

func NewEventPusher(sendFunc func(event json.RawMessage) error) *EventPusher {
	return &EventPusher{sendFunc: sendFunc}
}

func (ep *EventPusher) Push(event interface{}) error {
	data, err := json.Marshal(event)
	if err != nil {
		return err
	}

	envelope := map[string]interface{}{
		"jsonrpc": "2.0",
		"method":  "event.push",
		"params":  json.RawMessage(data),
	}

	envData, err := json.Marshal(envelope)
	if err != nil {
		return err
	}

	return ep.sendFunc(envData)
}

func (ep *EventPusher) PushQR(qrData string, expiresInSecs uint64) error {
	return ep.Push(map[string]interface{}{
		"type":            "qr",
		"qr_data":         qrData,
		"expires_in_secs": expiresInSecs,
	})
}

func (ep *EventPusher) PushConnected(deviceName, phoneNumber string) error {
	return ep.Push(map[string]interface{}{
		"type":         "connected",
		"device_name":  deviceName,
		"phone_number": phoneNumber,
	})
}

func (ep *EventPusher) PushDisconnected(reason string) error {
	return ep.Push(map[string]interface{}{
		"type":   "disconnected",
		"reason": reason,
	})
}

func (ep *EventPusher) PushMessage(msg map[string]interface{}) error {
	return ep.Push(msg)
}

func (ep *EventPusher) PushReaction(from, fromName, chatID, messageID, text string, timestamp int64, hasReaction bool) error {
	event := map[string]interface{}{
		"type":       "reaction",
		"from":       from,
		"from_name":  fromName,
		"chat_id":    chatID,
		"message_id": messageID,
		"text":       text,
		"timestamp":  timestamp,
	}

	if hasReaction {
		event["has_reaction"] = true
	} else {
		event["has_reaction"] = false
	}

	return ep.Push(event)
}

func (ep *EventPusher) PushPresence(jid, presence string) error {
	return ep.Push(map[string]interface{}{
		"type":     "presence",
		"jid":      jid,
		"presence": presence,
	})
}

func (ep *EventPusher) PushError(message string) error {
	return ep.Push(map[string]interface{}{
		"type":    "error",
		"message": message,
	})
}

func (ep *EventPusher) PushReady() error {
	return ep.Push(map[string]interface{}{
		"type": "ready",
	})
}

func (ep *EventPusher) PushSyncing(progress float32) error {
	return ep.Push(map[string]interface{}{
		"type":     "syncing",
		"progress": progress,
	})
}

func (ep *EventPusher) PushScanned() error {
	return ep.Push(map[string]interface{}{
		"type": "scanned",
	})
}

func (ep *EventPusher) PushQrExpired() error {
	return ep.Push(map[string]interface{}{
		"type": "qr_expired",
	})
}

type RawSender struct {
	conn interface {
		Write([]byte) (int, error)
	}
}

func (rs *RawSender) Send(event json.RawMessage) error {
	_, err := rs.conn.Write(event)
	return err
}

func NewRawSender(conn interface{ Write([]byte) (int, error) }) *RawSender {
	return &RawSender{conn: conn}
}

func Send(conn interface{ Write([]byte) (int, error) }, event interface{}) {
	data, err := json.Marshal(event)
	if err != nil {
		log.Printf("failed to marshal event: %v", err)
		return
	}

	envelope := map[string]interface{}{
		"jsonrpc": "2.0",
		"method":  "event.push",
		"params":  json.RawMessage(data),
	}

	envData, err := json.Marshal(envelope)
	if err != nil {
		log.Printf("failed to marshal envelope: %v", err)
		return
	}

	if _, err := conn.Write(append(envData, '\n')); err != nil {
		log.Printf("failed to write event: %v", err)
	}
}
