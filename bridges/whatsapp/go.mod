module github.com/anthropic/aleph/bridges/whatsapp

go 1.23.0

require (
	github.com/mattn/go-sqlite3 v1.14.24
	go.mau.fi/whatsmeow v0.0.0-20250101000000-000000000000
	google.golang.org/protobuf v1.36.11
)

// Note: The whatsmeow version above is a placeholder.
// Run the following to resolve all dependencies:
//
//   go get go.mau.fi/whatsmeow@latest
//   go mod tidy
